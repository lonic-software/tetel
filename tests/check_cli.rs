//! End-to-end tests: run the actual `tetel check` binary against small
//! hand-written fixtures and assert on exit code and, where useful, on
//! specific output content. Each of the four checks gets a fixture that
//! fails it and one that passes, plus the no-rows-found fixture whose
//! exit code must differ from a clean run's.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
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
fn no_rows_found_exit_code_differs_from_clean() {
    let (code, out) = run_check("no_rows.md");
    assert_eq!(code, 2, "no-rows state must use its own exit code:\n{out}");
    assert_ne!(code, 0, "no-rows must never be confusable with clean");
    assert!(out.contains("no tetel rows found"));
}

#[test]
fn grammar_check_fails_on_unknown_field() {
    let (code, out) = run_check("grammar_fail.md");
    assert_eq!(code, 1, "unknown field must redden:\n{out}");
    assert!(out.contains("unknown field"), "output was:\n{out}");
}

#[test]
fn grammar_check_passes_on_well_formed_row() {
    let (code, out) = run_check("grammar_pass.md");
    assert_eq!(code, 0, "well-formed row must be clean:\n{out}");
    assert!(out.contains("machine-checked: clean"));
}

#[test]
fn subset_check_fails_when_domain_wider_than_extent() {
    let (code, out) = run_check("subset_fail.md");
    assert_eq!(code, 1, "widened domain must redden:\n{out}");
    assert!(out.contains("domain not covered by extent"), "output was:\n{out}");
}

#[test]
fn subset_check_passes_when_extent_covers_domain() {
    let (code, out) = run_check("subset_pass.md");
    assert_eq!(code, 0, "covered domain must be clean:\n{out}");
    assert!(out.contains("machine-checked: clean"));
}

#[test]
fn abutting_literal_check_fails_on_mismatch() {
    let (code, out) = run_check("abutting_fail.md");
    assert_eq!(code, 1, "mismatched abutting literal must redden:\n{out}");
    assert!(out.contains("abutting-literal"), "output was:\n{out}");
    assert!(out.contains("29s"));
    assert!(out.contains("31s"));
}

#[test]
fn abutting_literal_check_passes_on_match() {
    let (code, out) = run_check("abutting_pass.md");
    assert_eq!(code, 0, "matching abutting literal must be clean:\n{out}");
    assert!(out.contains("machine-checked: clean"));
}

#[test]
fn abutting_literal_check_passes_on_multitoken_value_trailing_word_match() {
    // Row M-1's value is the two-token "elapsed 29s"; the prose cites it
    // with the abutting word "29s", which is the value's own trailing
    // word but not the whole value. A comparison against the whole
    // field (the old behavior) would mismatch "29s" != "elapsed 29s"
    // and redden; a comparison against the value's first word would
    // also mismatch ("elapsed" != "29s"). Only comparing against the
    // trailing word passes here, so this fixture distinguishes the
    // correct fix from both plausible wrong ones.
    let (code, out) = run_check("abutting_multitoken_pass.md");
    assert_eq!(code, 0, "trailing-word match on a multi-token value must be clean:\n{out}");
    assert!(out.contains("machine-checked: clean"));
}

#[test]
fn abutting_literal_check_downgrades_bare_integer_to_candidate() {
    // "appendix 2" is an ordinary cross-reference, not an asserted
    // value; row BI-1's value ("31s") never matches the bare integer
    // "2". Before the fix this hard-failed; now it must show up only as
    // an informational candidate.
    let (code, out) = run_check("abutting_bare_integer_pass.md");
    assert_eq!(code, 0, "a bare integer cross-reference must never fail:\n{out}");
    assert!(out.contains("machine-checked: clean"));
    assert!(out.contains("is near citation [BI-1] but not at abutting distance"));
}

#[test]
fn abutting_literal_check_never_treats_a_citation_as_a_literal() {
    // "[CL-1] [CL-2]" — the citation immediately before [CL-2] must
    // never be read as CL-2's asserted literal value, no matter how
    // digit-heavy CL-1's id is.
    let (code, out) = run_check("abutting_citation_as_literal_pass.md");
    assert_eq!(code, 0, "a citation abutting another citation must never fail:\n{out}");
    assert!(out.contains("machine-checked: clean"));
}

#[test]
fn abutting_literal_check_strips_backtick_and_quote_cruft_before_comparing() {
    // The abutting token is backtick-wrapped (`` `31s` ``); it must be
    // stripped down to "31s" before comparison and before display, not
    // compared (or printed) with the backticks still attached.
    let (code, out) = run_check("abutting_cruft_stripped_pass.md");
    assert_eq!(code, 0, "a backtick-wrapped literal must be stripped before comparing:\n{out}");
    assert!(out.contains("machine-checked: clean"));
}

#[test]
fn checker_reads_its_own_design_memos() {
    // scratch-memo-A.md and scratch-memo-B.md (copies of the design
    // memos this fix was built from) exercise the checker against real,
    // hand-written prose rather than a minimal fixture.
    let (code_a, out_a) = run_check("scratch-memo-A.md");
    assert_eq!(code_a, 0, "memo A must check clean:\n{out_a}");

    let (code_b, out_b) = run_check("scratch-memo-B.md");
    // The two abutting-literal symptoms this fix targets are gone:
    // a citation is never misread as a literal (no more "[E-4]"
    // reported as a literal value abutting [E-5]), and markdown cruft
    // around a literal token is stripped before comparison (no more
    // raw "'31s'`" garbage in the message).
    assert!(
        !out_b.contains("literal '[E-4]'"),
        "a citation must never be treated as a literal:\n{out_b}"
    );
    assert!(
        !out_b.contains("'31s'`'"),
        "backtick/quote cruft must be stripped before comparing:\n{out_b}"
    );
    // One failure remains, and it is a genuine, different-class one:
    // markdown inline code-spans (single backticks around a quoted
    // sentence) aren't excluded from citation/literal scanning the way
    // fenced ```blocks are, so the closing backtick of a quoted
    // transcript can still land at abutting distance from a real
    // citation. That gap lives in parse.rs's body-blanking logic, not
    // in the classification/comparison logic this fix touches — out of
    // scope here, and asserted explicitly so it isn't silently masked
    // or silently regressed by a future change.
    assert_eq!(code_b, 1, "memo B has one known, out-of-scope residual failure:\n{out_b}");
    assert!(
        out_b.contains("literal '31s' abuts [E-4] but row E-4's value is '\"1\"'"),
        "residual failure changed shape unexpectedly:\n{out_b}"
    );
}

#[test]
fn unsettled_citation_check_fails_on_bare_citation() {
    let (code, out) = run_check("unsettled_fail.md");
    assert_eq!(code, 1, "bare citation of an OWED row must redden:\n{out}");
    assert!(out.contains("unsettled-citation"), "output was:\n{out}");
}

#[test]
fn unsettled_citation_check_passes_with_stance_marker() {
    let (code, out) = run_check("unsettled_pass.md");
    assert_eq!(code, 0, "stance-marked citation must be clean:\n{out}");
    assert!(out.contains("machine-checked: clean"));
}

#[test]
fn output_never_prints_a_standalone_document_level_verdict() {
    // Every run — pass, fail, or mixed — must show two scoped partitions
    // and nothing that reads as a bare document-wide verdict word.
    for name in [
        "grammar_pass.md",
        "grammar_fail.md",
        "subset_fail.md",
        "abutting_fail.md",
        "unsettled_fail.md",
        "demo.md",
    ] {
        let (_, out) = run_check(name);
        assert!(out.contains("machine-checked:"), "{name} missing machine-checked partition:\n{out}");
        assert!(out.contains("human-owed:"), "{name} missing human-owed partition:\n{out}");
    }
}

#[test]
fn human_owed_partition_is_never_silently_empty() {
    for name in ["grammar_pass.md", "subset_pass.md", "abutting_pass.md", "unsettled_pass.md"] {
        let (_, out) = run_check(name);
        // Standing non-coverage is printed every run regardless of file content.
        assert!(out.contains("tetel does not catch:"), "{name} output:\n{out}");
    }
}

#[test]
fn demo_fixture_shows_the_full_output_contract() {
    let (code, out) = run_check("demo.md");
    assert_eq!(code, 1, "the kitchen-sink fixture has deliberate failures:\n{out}");
    // Claims print verbatim, not as bare counts.
    assert!(out.contains("The entire module is covered by this single symbol's inspection."));
    // Coverage-not-machine-checked rows are named explicitly.
    assert!(out.contains("coverage not machine-checked"));
    // Cited-but-undefined and defined-but-uncited both surface.
    assert!(out.contains("T-9"));
    assert!(out.contains("default disposition is delete"));
}
