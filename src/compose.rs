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
use crate::ledger;
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
    out.push_str(&render_ledger(&claim_list));
    out.push_str(&render_facts(
        &crate::facts::load_all(workspace_dir)?,
        &claim_list,
    ));
    Ok(out)
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
/// Domain and Extent are always [`ledger::NO_SCOPE_DECLARED`], never a
/// value derived from the facts a claim rests on: a claim minted by
/// `tetel claim` (see `claims.rs`) has no scope/domain field of its own
/// at all, and deriving one from its facts' extents would recreate
/// exactly the vacuity this project measured and rejected once
/// already — a field and its own check answered by one act.
/// `checks::claims_without_declared_scope` turns that sentinel into a
/// plain, human-owed line rather than a silent claim of full coverage.
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
        "Claims recorded with `tetel claim`. Domain/Extent below are not declared — the \
authoring model has no such field on a claim — so no coverage claim of any strength is \
made for any row; `tetel check` reports this plainly rather than treating an empty-seeming \
cell as full coverage.\n\n",
    );
    out.push_str("| ID | Proposition | Domain | Extent | Status |\n");
    out.push_str("|---|---|---|---|---|\n");
    for c in rows {
        out.push_str(&format!(
            "| {} | {} | {} | {} | OWED |\n",
            ledger_cell(&c.id),
            ledger_cell(&c.prop),
            ledger::NO_SCOPE_DECLARED,
            ledger::NO_SCOPE_DECLARED,
        ));
    }
    out
}
