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

/// TET-73: `analyze_ledger` used to push one row per *stale record*, so a
/// claim revised several times before its first reprove read as that many
/// separate findings — on this crate's own real memos, up to 216 rows on
/// one memo for a fraction of that many claims. `stale_evidence_aggregation.md`
/// carries a claim with three stale records and a fresh one (C1, which
/// belongs in `superseded`) and a claim with two stale records and no fresh
/// one (C2, which belongs in `out-of-proof`) — each must print as exactly
/// one row, aggregating the record count rather than enumerating them.
/// Finds the "Retrieve …with: <cmd>" line printed for `claim_id` and runs
/// `<cmd>` for real through `sh -c`, returning its stdout lines. Executing
/// the command rather than pattern-matching its text is the point: a
/// printed pointer that resolves to nothing (TET-73 code review, F1 — the
/// first version of this used the Rust-side field name `claim_id` instead
/// of the on-disk JSON key `name`, and matched zero lines against every
/// real ledger) would still have satisfied a `contains()` assertion.
fn run_retrieval_command(out: &str, claim_id: &str) -> Vec<String> {
    let marker = format!("\"name\":\"{claim_id}\"");
    let line = out
        .lines()
        .find(|l| l.contains("Retrieve") && l.contains(&marker))
        .unwrap_or_else(|| panic!("no retrieval line for {claim_id} in:\n{out}"));
    let cmd = line.split("with: ").nth(1).unwrap_or_else(|| panic!("no command on line: {line}")).trim();
    let result = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .unwrap_or_else(|e| panic!("failed to run `{cmd}`: {e}"));
    assert!(
        result.status.success(),
        "the printed command must actually succeed: `{cmd}`\nstderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8_lossy(&result.stdout).lines().map(str::to_string).collect()
}

#[test]
fn several_stale_records_on_one_claim_collapse_to_one_row_each() {
    let (code, out) = run_check("stale_evidence_aggregation.md");
    assert_eq!(code, 1, "C2 has nothing grading its current wording:\n{out}");

    // C2: out of proof, one row, counting both of its stale records.
    assert_eq!(
        out.matches("[out-of-proof]").count(),
        1,
        "one row for C2, not one per stale record:\n{out}"
    );
    assert!(
        out.contains("[out-of-proof] C2 — 2 record(s) grade wording this claim no longer carries, \
and nothing grades what it says today."),
        "output was:\n{out}"
    );

    // C1: superseded, one row, counting three stale against one fresh.
    assert_eq!(
        out.matches("superseded evidence:").count(),
        1,
        "one row for C1, not one per stale record:\n{out}"
    );
    assert!(
        out.contains("superseded evidence: C1 — 3 record(s) grade wording this claim no longer \
carries; 1 record(s) grade the current wording."),
        "output was:\n{out}"
    );

    // The retrieval commands must actually retrieve the records the rows
    // claim — run for real, not pattern-matched. C1 has 4 total records
    // (3 stale + 1 fresh, all returned by the pointer); C2 has 2 (both
    // stale, nothing fresh).
    assert_eq!(
        run_retrieval_command(&out, "C1").len(),
        4,
        "C1's pointer must return all 4 of its records:\n{out}"
    );
    assert_eq!(
        run_retrieval_command(&out, "C2").len(),
        2,
        "C2's pointer must return both of its stale records:\n{out}"
    );

    // The pointer names the real fixture ledger path, not a placeholder.
    let evidence_path = format!("{}.evidence.jsonl", fixture("stale_evidence_aggregation.md").display());
    assert!(
        out.contains(&evidence_path),
        "the pointer must name this fixture's real ledger path:\n{out}"
    );
}

/// TET-73 code review, F2: the memo path is interpolated straight from
/// argv into the printed pointer. `tests/fixtures/dir with space/memo.md`
/// pins the case an unquoted path breaks: `grep '"name":"C1"'
/// tests/fixtures/dir with space/memo.md.evidence.jsonl` (no quotes)
/// would split on the space and grep two files, neither of which is the
/// ledger. Run the printed command for real, not a reconstruction of it.
#[test]
fn a_memo_path_containing_a_space_prints_a_pointer_that_actually_runs() {
    let (code, out) = run_check("dir with space/memo.md");
    assert_eq!(code, 0, "nothing here is a machine failure:\n{out}");
    assert!(out.contains("superseded evidence: C1"), "output was:\n{out}");

    let lines = run_retrieval_command(&out, "C1");
    assert_eq!(
        lines.len(),
        2,
        "the pointer must survive the space in its own path and return both of C1's records:\n{out}"
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
