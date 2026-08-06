//! End-to-end tests for `tetel check`'s two ledger checks: ungrounded
//! claims (human-owed) and verdict disagreement (a machine failure,
//! whether between two grounding passes or against the author's own
//! `Status` cell). The five pre-existing checks have their own untouched
//! test file, `check_cli.rs`; this file only adds coverage for what this
//! slice adds on top of it.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn run_check(name: &str) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_tetel"))
        .arg("check")
        .arg(fixture(name))
        .output()
        .expect("failed to run tetel binary");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    (out.status.code().expect("process should exit normally"), stdout)
}

#[test]
fn check_reports_ungrounded_claims_as_human_owed_not_a_failure() {
    let (code, out) = run_check("ledger_check.md");
    assert_eq!(code, 0, "an ungrounded claim must never fail the run:\n{out}");
    assert!(out.contains("human-owed:"));
    assert!(out.contains("L-1: ungrounded"), "output was:\n{out}");
    assert!(out.contains("L-2: ungrounded"), "output was:\n{out}");
    assert!(
        out.contains("`foo` always returns **4**"),
        "the proposition must print verbatim, not a bare id:\n{out}"
    );
}

#[test]
fn check_reports_verdict_disagreement_between_two_passes_as_a_machine_failure() {
    let (code, out) = run_check("ledger_two_pass_disagreement.md");
    assert_eq!(code, 1, "two disagreeing passes must fail the run:\n{out}");
    assert!(out.contains("[verdict-disagreement]"), "output was:\n{out}");
    assert!(out.contains("agent-alpha"), "both pass identities must print:\n{out}");
    assert!(out.contains("agent-beta"), "both pass identities must print:\n{out}");
    assert!(out.contains("supports"), "output was:\n{out}");
    assert!(out.contains("refutes"), "output was:\n{out}");
    assert!(
        out.contains("returns 4 on every branch"),
        "the first pass's note must print verbatim:\n{out}"
    );
    assert!(
        out.contains("returns 5 under overflow"),
        "the second pass's note must print verbatim:\n{out}"
    );
}

#[test]
fn check_reports_verdict_disagreement_against_author_status_as_a_machine_failure() {
    let (code, out) = run_check("ledger_status_conflict.md");
    assert_eq!(code, 1, "a pass refuting a VERIFIED status must fail the run:\n{out}");
    assert!(out.contains("[verdict-disagreement]"), "output was:\n{out}");
    assert!(out.contains("author (Status cell)"), "output was:\n{out}");
    assert!(out.contains("agent-gamma"), "output was:\n{out}");
    assert!(
        out.contains("contradicting the author's VERIFIED status"),
        "the pass's note must print verbatim:\n{out}"
    );
}

#[test]
fn check_reports_a_claim_grounded_only_by_attested_evidence_as_its_own_human_owed_line() {
    // A-1 has exactly one recorded evidence record, so it's grounded —
    // "ungrounded" must not fire for it. But that record derives to
    // Attested for standing (every ingested record does, regardless of
    // its `reported_kind`, which here even claims "run"), so the new
    // line — distinct from "ungrounded" — must fire instead.
    let (code, out) = run_check("ledger_attested_grounded.md");
    assert_eq!(code, 0, "grounded-only-by-attested-evidence must never fail the run:\n{out}");
    assert!(out.contains("human-owed:"));
    assert!(
        out.contains("A-1: grounded only by attested evidence"),
        "output was:\n{out}"
    );
    assert!(
        !out.contains("A-1: ungrounded"),
        "a claim with a recorded evidence record must never also print as ungrounded:\n{out}"
    );
    assert!(
        out.contains("`lib` exposes the check/brief/record entry points"),
        "the proposition must print verbatim, not a bare id:\n{out}"
    );
}

#[test]
fn check_reports_an_unresolved_evidence_source_as_human_owed_not_a_failure() {
    // U-1's evidence record names `src/does-not-exist.rs` as its source —
    // nothing on disk backs the reported act. This must surface, but
    // never redden the run: fabricating an attested fact should require
    // fabricating a preserved artifact, and a missing one is residue for
    // a human, mirroring cited-but-undefined's non-failing disposition.
    let (code, out) = run_check("ledger_unresolved_source.md");
    assert_eq!(code, 0, "an unresolved evidence source must never fail the run:\n{out}");
    assert!(
        out.contains("evidence source `src/does-not-exist.rs` does not resolve"),
        "output was:\n{out}"
    );
    assert!(out.contains("U-1"), "output was:\n{out}");
}

#[test]
fn check_runs_the_five_existing_checks_and_the_two_ledger_checks_together() {
    let (code, out) = run_check("ledger_and_tetel_rows.md");
    // TR-1 is a well-formed tetel row (clean); L-1 is ungrounded
    // (human-owed, not a failure). Nothing here should fail.
    assert_eq!(code, 0, "output was:\n{out}");
    assert!(out.contains("machine-checked: clean"), "output was:\n{out}");
    assert!(out.contains("TR-1"), "the five existing checks must still see the tetel row:\n{out}");
    assert!(out.contains("L-1: ungrounded"), "the ledger check must still see the ledger claim:\n{out}");
}

#[test]
fn a_citation_resolving_to_a_ledger_claim_is_not_cited_but_undefined() {
    // L-1 is a ledger claim, not a fenced row, and is cited bare (no
    // stance marker) with an OWED status — exactly the shape `tetel
    // render` produces, and exactly the shape that used to report
    // `cited but undefined: [L-1]` on every memo this tool authors
    // itself. GHOST-1 is genuinely undefined and must still surface: the
    // fix must not weaken the check, only stop it being blind to the
    // ledger.
    let (code, out) = run_check("ledger_citation_resolves.md");
    assert_eq!(code, 0, "a ledger-defined citation must never fail the run:\n{out}");
    assert!(
        !out.contains("cited but undefined: [L-1]") && !out.contains("[L-1, GHOST-1]") && !out.contains("[GHOST-1, L-1]"),
        "L-1 is defined by the ledger and must not be reported cited-but-undefined:\n{out}"
    );
    assert!(
        out.contains("cited but undefined: [GHOST-1]"),
        "a genuinely undefined citation must still be reported:\n{out}"
    );
    assert!(
        !out.contains("[unsettled-citation]"),
        "check 4 has no ledger-claim equivalent to run against and must not fabricate one:\n{out}"
    );
}

#[test]
fn a_ledger_claim_never_cited_by_prose_still_surfaces_as_defined_uncited() {
    // The inverse of the fix above: teaching the citation check to see
    // ledger claims must not also make an *uncited* one invisible. L-1 is
    // cited and must not print as uncited; L-2 is never cited and must.
    let (code, out) = run_check("ledger_uncited_claim.md");
    assert_eq!(code, 0, "an uncited ledger claim must never fail the run:\n{out}");
    assert!(
        out.contains("L-2: defined but never cited; default disposition is delete"),
        "an uncited ledger claim must surface exactly like an uncited row:\n{out}"
    );
    assert!(
        !out.contains("L-1: defined but never cited"),
        "L-1 is cited and must not print as uncited:\n{out}"
    );
}
