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
