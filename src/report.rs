//! Renders the output contract: exactly two labelled partitions, each
//! stating its own scope on the same line, no standalone document-level
//! verdict anywhere. See the crate README / design memo for the contract
//! this implements.

use crate::checks::Findings;
use crate::parse::Document;

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

pub fn render(display_path: &str, doc: &Document, findings: &Findings) -> (i32, String) {
    if doc.row_groups_found == 0 && findings.ledger_claims_found == 0 {
        let msg = format!(
            "no tetel rows found in {display_path} — out of scope, nothing was checked. \
This is a distinct state from a clean run, not a weaker way of spelling it (exit {EXIT_NO_ROWS}).\n"
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
        + findings.verdict_disagreements.len();
    let scope = "grammar, subset (enumerated rows only), abutting literals, unsettled citations, \
dependency cascades, evidence-ledger import, verdict disagreement";
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
by attested (ingested) evidence, evidence sources that do not resolve, and tetel's own \
standing non-coverage \u{2014} none of this is settled by a passing check\n",
    );
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
    for item in NON_COVERAGE {
        out.push_str(&format!("  - tetel does not catch: {item}\n"));
    }

    let code = if failing { EXIT_CHECK_FAILED } else { EXIT_CLEAN };
    (code, out)
}
