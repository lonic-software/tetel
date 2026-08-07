//! Renders the output contract: exactly two labelled partitions, each
//! stating its own scope on the same line, no standalone document-level
//! verdict anywhere. See the crate README / design memo for the contract
//! this implements.

use crate::checks::Findings;
use crate::parse::Document;
use crate::snapshot::Provenance;

/// Exit codes. 0 and 1 are the conventional pass/fail; 2 is the D8 state —
/// "no tetel rows found" — which must never be confusable with clean.
pub const EXIT_CLEAN: i32 = 0;
pub const EXIT_CHECK_FAILED: i32 = 1;
pub const EXIT_NO_ROWS: i32 = 2;

const NON_COVERAGE: &[&str] = &[
    "dependents that never declared themselves",
    "deleted premises",
    "one word used in two senses",
    "a command that runs green without establishing its proposition",
    "unfalsifiable-shaped claim phrasing",
];

/// `build` names the binary that produced this report (see `buildid.rs`).
/// It is passed in rather than read here so this function stays a pure
/// function of the document it grades — and it is *always* printed,
/// including on the no-rows path, because the whole point is that two
/// outputs which disagree can be told apart by their checker. A report
/// that does not name its build cannot be disbelieved.
pub fn render(display_path: &str, doc: &Document, findings: &Findings, build: &str) -> (i32, String) {
    if doc.row_groups_found == 0 && findings.ledger_claims_found == 0 {
        let msg = format!(
            "no tetel rows found in {display_path} — out of scope, nothing was checked. \
This is a distinct state from a clean run, not a weaker way of spelling it (exit {EXIT_NO_ROWS}).\n\
\nchecked by {build}\n"
        );
        return (EXIT_NO_ROWS, msg);
    }

    let mut out = String::new();

    // --- machine-checked partition -----------------------------------
    let failing = findings.machine_check_failed();
    let total_failures = findings.grammar_errors.len()
        + findings.subset_failures.len()
        + findings.abutting_failures.len()
        + findings.unsettled_failures.len()
        + findings.cascade_failures.len()
        + findings.ledger_errors.len()
        + findings.verdict_disagreements.len()
        + findings.stale_evidence.len()
        + usize::from(findings.provenance_failed());
    let scope = "grammar, subset (enumerated rows only), abutting literals, unsettled citations, \
dependency cascades, evidence-ledger import, verdict disagreement, stale evidence, provenance drift";
    if failing {
        out.push_str(&format!(
            "machine-checked: {total_failures} failing — {scope}\n"
        ));
        for e in &findings.grammar_errors {
            out.push_str(&format!("  - [grammar] {e}\n"));
        }
        for e in &findings.subset_failures {
            out.push_str(&format!("  - [subset] {e}\n"));
        }
        for e in &findings.abutting_failures {
            out.push_str(&format!("  - [abutting-literal] {e}\n"));
        }
        for e in &findings.unsettled_failures {
            out.push_str(&format!("  - [unsettled-citation] {e}\n"));
        }
        for e in &findings.cascade_failures {
            out.push_str(&format!("  - [cascade] {e}\n"));
        }
        for e in &findings.ledger_errors {
            out.push_str(&format!("  - [ledger] {e}\n"));
        }
        for e in &findings.verdict_disagreements {
            out.push_str(&format!("  - [verdict-disagreement] {e}\n"));
        }
        for e in &findings.stale_evidence {
            out.push_str(&format!("  - [stale-evidence] {e}\n"));
        }
        match &findings.provenance {
            Provenance::Drifted { first_diff_line, snapshot_lines, memo_lines } => {
                let where_ = match first_diff_line {
                    Some(n) => format!("first difference at line {n}"),
                    None => "identical line-for-line but different lengths".to_string(),
                };
                out.push_str(&format!(
                    "  - [provenance-drift] this document is not what its own snapshot renders \
({where_}; snapshot {snapshot_lines} lines, document {memo_lines}). Either the document was \
edited by hand after rendering, or the workspace moved on without a re-render — a reader \
following a citation would land somewhere this text was never produced from. Re-render, or \
recover the workspace the text really came from.\n"
                ));
            }
            Provenance::Unreadable(e) => {
                out.push_str(&format!(
                    "  - [provenance-drift] a snapshot exists beside this document but could not \
be rendered from ({e}) — reported rather than passed over, because an unreadable record is not \
a matching one.\n"
                ));
            }
            Provenance::Missing | Provenance::Matches => {}
        }
    } else {
        out.push_str(&format!("machine-checked: clean — {scope}\n"));
    }
    if !findings.abutting_candidates.is_empty() {
        out.push_str("  informational, not checked (never a failure, at any distance looser than abutting):\n");
        for c in &findings.abutting_candidates {
            out.push_str(&format!("    - {c}\n"));
        }
    }

    out.push('\n');

    // --- human-owed partition -----------------------------------------
    out.push_str(
        "human-owed: every READING/OBSERVED/ATTESTED row, every row whose domain or extent \
contains a proc:/external designator, the RUN command\u{2194}proposition correspondence, \
cited-but-undefined and defined-but-uncited ids, ungrounded ledger claims, claims grounded only \
by attested (ingested) evidence, evidence sources that do not resolve, ledger claims with no \
declared scope at all, qualified verdicts, superseded evidence, facts whose note names a location outside their own \
 captured extent, refusals recorded in a fact's own mint window, \
and tetel's own standing non-coverage \u{2014} none of this is settled by a passing check\n",
    );
    for e in &findings.superseded_evidence {
        out.push_str(&format!("  - superseded evidence: {e}\n"));
    }
    for (id, pass, note) in &findings.qualified_claims {
        out.push_str(&format!(
            "  - {id}: QUALIFIED by pass {pass} — not a plain confirmation. \"{note}\"\n"
        ));
    }
    for line in &findings.grounding_provenance {
        out.push_str(&format!("  - {line}\n"));
    }
    for w in &findings.mint_windows {
        let window = if w.is_first {
            "before this fact, the first minted in its workspace"
        } else {
            "between this fact and the one before it"
        };
        out.push_str(&format!(
            "  - {}: {} refusal(s) recorded {window} — what the author tried and could not do in \
the window that produced it. Frequently innocent; worth a reader's eye when a note reaches past \
its extent, because a refused `look` leaves the pending buffer untouched and the next mint folds \
whatever was already there:\n",
            w.fact_id,
            w.refusals.len()
        ));
        for line in &w.refusals {
            out.push_str(&format!("      {line}\n"));
        }
        if w.straddles_a_boundary {
            out.push_str(
                "      (a refusal here shares a second with a mint, so which side of it the \
refusal fell on cannot be recovered — it is listed under both adjacent facts rather than \
guessed at)\n",
            );
        }
    }
    for o in &findings.notes_outside_extent {
        out.push_str(&format!(
            "  - {}: its note names {}, which this fact's extent does not cover (extent: {}) — \
the extent was captured by the tool, the note was written, so the two disagreeing is worth a \
reader's eye. A note may name a location as context; only you can tell that from a conclusion \
drawn about code this fact never opened\n",
            o.fact_id,
            o.mentioned,
            o.extent_labels.join("; ")
        ));
    }
    if matches!(findings.provenance, Provenance::Missing) && findings.cites_something {
        out.push_str(
            "  - no workspace snapshot beside this document: its citation ids are \
workspace-relative, so without the workspace that minted them every citation here is a pointer \
this repository cannot resolve. Re-render with `tetel render --out <this file>` to write one.\n",
        );
    }
    for (id, kind_status, claim) in &findings.human_owed_rows {
        out.push_str(&format!("  - {id} [{kind_status}]: {claim}\n"));
    }
    for (id, claim) in &findings.coverage_skipped {
        out.push_str(&format!(
            "  - {id}: coverage not machine-checked (domain or extent contains a proc:/external designator, so no coverage claim of any strength is made) — {claim}\n"
        ));
    }
    if !findings.run_row_ids.is_empty() {
        out.push_str(&format!(
            "  - RUN rows [{}]: a matching re-run establishes only that the command reproduces its stored value, never that the value establishes the claim\n",
            findings.run_row_ids.join(", ")
        ));
    }
    if !findings.cited_undefined.is_empty() {
        out.push_str(&format!(
            "  - cited but undefined: [{}]\n",
            findings.cited_undefined.join(", ")
        ));
    }
    for (id, claim) in &findings.defined_uncited {
        out.push_str(&format!(
            "  - {id}: defined but never cited; default disposition is delete, not hunting for a citation — {claim}\n"
        ));
    }
    for (id, proposition) in &findings.ungrounded_claims {
        out.push_str(&format!(
            "  - {id}: ungrounded — no evidence record on file — {proposition}\n"
        ));
    }
    for (id, proposition) in &findings.attested_grounded_claims {
        out.push_str(&format!(
            "  - {id}: grounded only by attested evidence — someone looked, off-instrument; \
distinct from no evidence at all, but never enough on its own to move past vouched — {proposition}\n"
        ));
    }
    for line in &findings.unresolved_evidence_sources {
        out.push_str(&format!("  - {line}\n"));
    }
    for (id, proposition) in &findings.no_scope_claims {
        out.push_str(&format!(
            "  - {id}: no scope declared (tetel's authoring model has no domain/extent field on a claim) — no coverage claim of any strength is made — {proposition}\n"
        ));
    }
    // Once, not once per claim. This is a fact about the authoring model,
    // identical for every claim in the document and dischargeable by
    // nobody; repeating it per row buried the findings that are about
    // *this* document under a constant.
    if findings.ledger_has_no_scope_columns {
        out.push_str(
            "  - no claim in this document declares a scope: `tetel claim` has no such field, \
so no coverage claim of any strength is made by any row. What each claim rests on is in the \
Facts table; whether it rests on enough is yours to judge\n",
        );
    }
    for item in NON_COVERAGE {
        out.push_str(&format!("  - tetel does not catch: {item}\n"));
    }

    out.push_str(&format!("\nchecked by {build}\n"));

    let code = if failing { EXIT_CHECK_FAILED } else { EXIT_CLEAN };
    (code, out)
}
