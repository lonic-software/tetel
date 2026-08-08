//! `tetel render` — assemble the workspace's current prose state into
//! markdown on stdout, followed by an evidence ledger built from the
//! workspace's claims. Mirrors `trender`, plus fix 1 from this port's own
//! design memo: the prototype's `trender` had nothing downstream that
//! read its output back, so a rendered document and `tetel check` never
//! connected — every check reported "no evidence ledger found" against
//! a document this same tool had just produced. Appending a ledger
//! table in the shape `ledger::import` already reads closes that loop
//! without inventing a third evidence shape.
//!
//! Headings render at their own declared depth (`#` * level), not a
//! single fixed depth — see `prose.rs`'s doc comment on why that's a
//! fix, not a stylistic choice. A revised block renders only its
//! current text; superseded text lives in `prose.jsonl`'s history but
//! never here.
//!
//! The ledger is strictly an addition after the prose: nothing here can
//! change a single byte the prose loop already wrote, so the document a
//! human reads stays exactly what was authored — only `tetel check`
//! sees the appended table as evidence to grade.

use std::io;
use std::path::Path;

use crate::claims;
use crate::prose;

pub fn render(workspace_dir: &Path) -> io::Result<String> {
    let blocks = prose::load_all(workspace_dir)?;
    let mut out = String::new();
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if b.heading {
            let level = b.level.unwrap_or(2).clamp(1, 6);
            out.push_str(&"#".repeat(level as usize));
            out.push(' ');
            out.push_str(&b.text);
            out.push('\n');
        } else {
            out.push_str(&b.text);
            out.push('\n');
            if !b.cite.is_empty() {
                out.push('\n');
                out.push_str(&format!("*cites: {}*\n", b.cite.join(", ")));
            }
        }
    }
    let claim_list = claims::load_all(workspace_dir)?;
    out.push_str(&render_targets(&crate::targets::load_all(workspace_dir)?, &claim_list));
    out.push_str(&render_transplants(
        &crate::transplants::load_all(workspace_dir)?,
        &crate::targets::load_all(workspace_dir)?,
        &crate::facts::load_all(workspace_dir)?,
        &claim_list,
    ));
    out.push_str(&render_ledger(&claim_list));
    out.push_str(&render_facts(
        &crate::facts::load_all(workspace_dir)?,
        &claim_list,
    ));
    Ok(out)
}

/// Appends the modification-target section — the symbols this design
/// tells an implementer to modify, each with the fact that censuses it.
///
/// # Why it renders when it is empty
///
/// Because the hole is the finding. A declared target is refused without
/// a census at declaration time, so a *declared* target is always
/// censused; what no refusal can reach is a target the author never
/// declared, since deciding that a memo recommended something it never
/// declared is the heuristic read of prose the whole design rejects.
///
/// Visibility is the answer instead. Once a document has any claim at
/// all, this section renders — as "none declared" when it is empty — so
/// a memo full of tell-the-implementer language with an empty section
/// shows its own gap to every reader. An absent section would make the
/// same omission invisible, which is the failure mode, not the fix.
///
/// Withdrawn targets are dropped rather than struck through: a target is
/// withdrawn precisely when it stopped being a target, and the log keeps
/// the history for anyone auditing the workspace.
fn render_targets(targets: &[crate::targets::Target], claims: &[claims::Claim]) -> String {
    let live: Vec<&crate::targets::Target> = targets.iter().filter(|t| !t.withdrawn).collect();
    // Nothing to be silent about yet: a document with no claims is not a
    // document making recommendations.
    if live.is_empty() && claims.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n## Modification targets\n\n");
    if live.is_empty() {
        out.push_str(
            "None declared. A target is a symbol this design tells an implementer to modify; \
             declaring one requires a search of the whole worktree for it. Nothing forces a \
             recommendation to be declared, so an empty section is not evidence that none exist.\n",
        );
        return out;
    }
    out.push_str("| target | censused by |\n|---|---|\n");
    for t in live {
        out.push_str(&format!("| `{}` | {} |\n", t.symbol, t.from));
    }
    out
}

/// Appends the transplant section — the mechanisms this design takes
/// from somewhere else, each donor premise in the donor's own words with
/// the claim that answers it at the destination.
///
/// # Why the premise text is inlined when captured output never is
///
/// [`render_facts`] deliberately keeps captured output out of the
/// document: it is unbounded, it lives in the snapshot, and inlining it
/// would bury the prose. A premise inverts every one of those. It is an
/// author-bounded selection rather than a whole capture, and putting the
/// donor's clause in front of the reader *is* the mechanism — a section
/// that printed only ids would hide the one sentence the inventory
/// exists to make someone read.
///
/// Fenced blocks rather than table cells, because a premise legitimately
/// spans lines and a markdown cell cannot carry one without escaping the
/// newlines away — which would break the byte fidelity the refusal is
/// built on. The fence is widened past any backtick run inside the text,
/// since donor comments in this very language routinely contain fenced
/// examples of their own.
fn render_transplants(
    transplants: &[crate::transplants::Transplant],
    targets: &[crate::targets::Target],
    facts: &[crate::facts::Fact],
    claims: &[claims::Claim],
) -> String {
    let live: Vec<&crate::transplants::Transplant> = transplants.iter().filter(|t| !t.withdrawn).collect();
    if live.is_empty() && claims.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n## Transplants\n\n");
    if live.is_empty() {
        out.push_str(
            "None declared. A transplant installs a mechanism taken from another site; declaring \
             one requires selecting the donor's stated premises from captured output and answering \
             each at the destination. Nothing forces a transplant to be declared, so an empty \
             section is not evidence that none exist.\n",
        );
        return out;
    }
    out.push_str(
        "Mechanisms this design takes from elsewhere. Every premise below is the donor's own \
         words, selected from captured output rather than typed — `tetel transplant` refuses a \
         premise that is not a verbatim substring of one observation in the cited donor fact. \
         Whether the answering claim is *true* is graded by grounding, not here.\n",
    );
    for t in live {
        let symbol = targets
            .iter()
            .find(|x| x.id == t.into)
            .map(|x| format!("`{}`", x.symbol))
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!("\n### {} — {} into {} ({})\n\n", t.id, t.from, symbol, t.into));
        if let Some(f) = facts.iter().find(|f| f.id == t.from) {
            let extent = f.extent.iter().map(|e| e.label.as_str()).collect::<Vec<_>>().join("; ");
            out.push_str(&format!("Donor extent (captured): {extent}\n"));
        }
        let premises: Vec<&crate::transplants::Premise> = t.live_premises().collect();
        if premises.is_empty() {
            out.push_str("\nNo premises selected from the donor.\n");
            continue;
        }
        for p in premises {
            let answered = match p.discharged_by.as_deref() {
                Some(c) if claims.iter().any(|x| x.id == c && !x.withdrawn) => {
                    format!("holds at the destination per {c}")
                }
                // Renderable only on a preview: `render --out` refuses a
                // document with an unanswered premise.
                _ => "**not yet answered at the destination**".to_string(),
            };
            out.push_str(&format!("\n**{}** — {}\n\n", p.id, answered));
            out.push_str(&fenced(&p.text));
        }
    }
    out
}

/// Wrap text in a code fence long enough that nothing inside it can end
/// the block early — the text is a verbatim quotation and must survive
/// rendering byte for byte.
fn fenced(text: &str) -> String {
    let longest_run = text
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest_run.max(2) + 1);
    format!("{fence}text\n{text}\n{fence}\n")
}

/// Appends a facts table after the evidence ledger.
///
/// # Why the document has to carry this
///
/// Before it did, `prose --cites` accepted fact ids as readily as claim
/// ids and `render` wrote them into the document, but no fact appeared
/// anywhere in the output — so `tetel check` reported `cited but
/// undefined: [F5]` on the first real memo, correctly, and would have on
/// every memo that ever cited a fact from prose.
///
/// The deeper reason is what a reader loses without it. A fact's captured
/// extent is the one field in this system no author can type, and reading
/// notes against extents is where review value has actually shown up —
/// the FORK-94 review's sharpest finding was a note concluding about a
/// file its extent never covered. Without this table that comparison is
/// possible only by opening the workspace, which a reviewer of a
/// committed document does not have.
///
/// # What is deliberately not here
///
/// The `key` is an absolute machine-local path; the `label` is
/// repo-relative. Only the label is rendered — a committed document
/// should not carry `/Users/<someone>/...`, and the label is what a
/// reader can act on anyway.
///
/// Captured output is not rendered either. It is unbounded (a `run` can
/// capture megabytes), it is in the snapshot for anyone who needs it, and
/// a document that inlined it would bury the prose it exists to support.
fn render_facts(facts: &[crate::facts::Fact], claims: &[claims::Claim]) -> String {
    if facts.is_empty() {
        return String::new();
    }
    // Which claims rest on each fact — so a reader landing on `F5` from
    // prose can see what was built on it without scanning every row.
    let cited_by = |id: &str| -> String {
        let names: Vec<&str> = claims
            .iter()
            .filter(|c| !c.withdrawn && c.from.iter().any(|f| f == id))
            .map(|c| c.id.as_str())
            .collect();
        if names.is_empty() {
            "—".to_string()
        } else {
            names.join(", ")
        }
    };

    let mut out = String::new();
    out.push_str("\n## Facts\n\n");
    out.push_str(
        "Observations recorded with `tetel look`/`tetel run`, then minted with `tetel fact`. \
Extent is captured by the tool and cannot be typed by an author; the note is written. Where \
a note names a location the extent does not cover, `tetel check` says so — reading those two \
columns against each other is the point of this table.\n\n",
    );
    out.push_str("| ID | Note | Extent (captured) | Rests under |\n");
    out.push_str("|---|---|---|---|\n");
    for f in facts {
        let extent = f
            .extent
            .iter()
            .map(|e| e.label.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            ledger_cell(&f.id),
            ledger_cell(&f.note),
            ledger_cell(&extent),
            cited_by(&f.id),
        ));
    }
    out
}

/// Escape a claim proposition for safe embedding in one evidence-ledger
/// table cell: a bare `|` would otherwise be read as a cell separator
/// (see `ledger::split_row_cells`), and an embedded newline would end
/// the table row early, since every row of a markdown table is exactly
/// one physical line. A plain backslash is left alone — the ledger
/// importer only treats a backslash specially when it precedes `|`, so
/// an unrelated `\` round-trips unchanged.
fn ledger_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// Appends, after the prose, one evidence-ledger row per non-withdrawn
/// claim — the shape `ledger::import` already reads (see its module doc
/// comment), reused rather than a third shape invented for this. Empty
/// when there are no claims: a document with nothing to cite has nothing
/// to check either.
///
/// There are no Domain/Extent columns. A claim minted by `tetel claim`
/// (see `claims.rs`) has no scope field of its own, because v1's
/// `Domain ⊆ Extent` was retired on evidence — and deriving one from its
/// facts' extents would recreate exactly the vacuity that measurement
/// rejected: a field and its own check answered by one act.
///
/// Earlier this table carried the two columns anyway, filled with a
/// sentinel, which put two permanently unfillable cells in front of every
/// reader and made `check` print one undischargeable line per claim. The
/// absence is now stated once, in the paragraph above the table and once
/// in `check`'s human-owed list. What a claim rests on lives in the Facts
/// table, where the extent beside it was captured rather than typed.
///
/// Status is always `OWED`: a claim just authored has not been
/// independently graded by anything this crate can see yet (that is
/// what `tetel record` is for), and never `VERIFIED`/`REFUTED` — either
/// would assert a verdict nobody gave.
fn render_ledger(claims: &[claims::Claim]) -> String {
    let rows: Vec<&claims::Claim> = claims.iter().filter(|c| !c.withdrawn).collect();
    if rows.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("\n## Evidence ledger\n\n");
    out.push_str(
        "Claims recorded with `tetel claim`. No claim here declares a scope — the authoring \
model has no such field, and no coverage claim of any strength is made for any row. What each \
claim rests on is in the Facts table below.\n\n",
    );
    out.push_str("| ID | Proposition | Status |\n");
    out.push_str("|---|---|---|\n");
    for c in rows {
        out.push_str(&format!(
            "| {} | {} | OWED |\n",
            ledger_cell(&c.id),
            ledger_cell(&c.prop),
        ));
    }
    out
}
