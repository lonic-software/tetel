//! End-to-end tests for `tetel brief`: the grounding brief emitted from a
//! memo's evidence ledger table. `tetel check`'s ledger checks have their
//! own test file (`check_ledger_cli.rs`); `tetel record`'s is
//! `record_cli.rs`.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn run_brief(name: &str) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_tetel"))
        .arg("brief")
        .arg(fixture(name))
        .output()
        .expect("failed to run tetel binary");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    (out.status.code().expect("process should exit normally"), stdout)
}

fn run_brief_json(name: &str) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_tetel"))
        .arg("brief")
        .arg("--json")
        .arg(fixture(name))
        .output()
        .expect("failed to run tetel binary");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    (out.status.code().expect("process should exit normally"), stdout)
}

#[test]
fn a_floor_of_zero_is_refused_because_it_would_be_the_flag_that_does_not_exist() {
    let out = Command::new(env!("CARGO_BIN_EXE_tetel"))
        .arg("brief")
        .arg("--confirm")
        .arg("0")
        .arg(fixture("ledger_check.md"))
        .output()
        .expect("failed to run tetel binary");
    assert_ne!(out.status.code(), Some(0), "a floor of 0 must be refused");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("at least 1"), "stderr was:\n{err}");
}

#[test]
fn every_claim_is_owed_when_authorship_cannot_be_determined() {
    // This fixture ships no snapshot beside it, so nothing can say whether
    // a grading workspace was the author's own. Counting anyway would
    // score self-grading as independent confirmation, so the rule errs
    // toward scheduling — and it is keyed on whether the file is there,
    // not on a guess about what it would have said.
    let (code, out) = run_brief("ledger_check.md");
    assert_eq!(code, 0, "output was:\n{out}");
    assert!(out.contains("owed (floor 1): L-1, L-2"), "output was:\n{out}");

    // Additive, never a truncation: every claim is still briefed in full.
    assert!(out.contains("id: L-1"), "output was:\n{out}");
    assert!(out.contains("id: L-2"), "output was:\n{out}");
    assert_eq!(out.matches("scope: WITHHELD").count(), 2, "output was:\n{out}");

    // And the schedule says which ids, never why any claim is absent.
    assert!(!out.contains("already graded"), "output was:\n{out}");
}

#[test]
fn brief_reports_no_evidence_ledger_for_a_memo_without_one() {
    // no_rows.md (from check_cli.rs's fixtures) has neither tetel fences
    // nor a ledger table.
    let (code, out) = run_brief("no_rows.md");
    assert_eq!(code, 2, "no ledger must use its own exit code:\n{out}");
    assert!(out.contains("no evidence ledger found"), "output was:\n{out}");
}

#[test]
fn brief_lists_every_claim_with_byte_identical_proposition_and_withholds_scope() {
    let (code, out) = run_brief("ledger_check.md");
    assert_eq!(code, 0, "a clean ledger must brief cleanly:\n{out}");
    assert!(out.contains("2 claim(s)"), "output was:\n{out}");
    assert!(out.contains("id: L-1"));
    // Byte-identical to the source cell — bold and backticks intact,
    // nothing paraphrased.
    assert!(
        out.contains("proposition: `foo` always returns **4**"),
        "output was:\n{out}"
    );
    assert!(out.contains("pin: abc1234"), "the document-level pin must be attached to each claim:\n{out}");
    assert!(out.contains("scope: WITHHELD"));
    // The domain/extent cell text must never appear.
    assert!(!out.contains("foo's body"), "domain must be withheld:\n{out}");
    assert!(!out.contains("opened in full"), "extent must be withheld:\n{out}");
}

#[test]
fn brief_json_output_is_parseable_and_withholds_scope() {
    let (code, out) = run_brief_json("ledger_check.md");
    assert_eq!(code, 0);
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("--json output must parse as JSON");

    // The top level was a bare array and is now an object. That change is
    // deliberate and this assertion was rewritten rather than relaxed: the
    // owed schedule depends on the floor in force, an array has nowhere to
    // put a document-level value, and a floor printed in text but absent
    // from JSON would leave the schedule unattributable under exactly the
    // mode a program consumes.
    assert!(parsed.get("floor").is_some(), "the floor must be visible under --json too");
    assert!(parsed.get("owed").is_some(), "the schedule must be visible under --json too");

    let items = parsed["claims"].as_array().expect("claims must be an array");
    // Every claim is still emitted. The owed list is additive; it never
    // truncates the brief, because a claim may not be dropped on account
    // of what its evidence ledger already holds.
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], "L-1");
    assert_eq!(items[0]["proposition"], "`foo` always returns **4**");
    assert_eq!(items[0]["scope"], "WITHHELD");
    assert!(items[0].get("domain").is_none());
    assert!(items[0].get("extent").is_none());
}

#[test]
fn brief_reports_malformed_ledger_rows_without_dropping_them() {
    let (code, out) = run_brief("ledger_malformed_row.md");
    assert_eq!(code, 1, "an unparsable row must not exit clean:\n{out}");
    assert!(out.contains("ledger import errors"), "output was:\n{out}");
    assert!(out.contains("5 cell"), "output was:\n{out}");
    // The well-formed sibling row must still come through.
    assert!(out.contains("id: M-2"), "a bad row must not sink the whole import:\n{out}");
}

#[test]
fn brief_still_lists_a_claim_grounded_only_by_ingested_evidence() {
    // ledger_attested_grounded.md's only claim (A-1) already has a
    // recorded, attested-derived evidence record on disk (see its sibling
    // `.evidence.jsonl`, used by check_ledger_cli.rs's own test on this
    // fixture). Ingestion enrolls a claim in the independent-grounding
    // loop; it does not excuse it — `brief` must keep listing it exactly
    // as if nothing had been recorded yet, until a witnessed grounding
    // lands.
    let (code, out) = run_brief("ledger_attested_grounded.md");
    assert_eq!(code, 0, "output was:\n{out}");
    assert!(out.contains("id: A-1"), "output was:\n{out}");
    assert!(
        out.contains("proposition: `lib` exposes the check/brief/record entry points"),
        "output was:\n{out}"
    );
}
