//! End-to-end tests for tetel's authoring commands (`look`, `run`,
//! `fact`, `claim`, `prose`, `render`, `query`). Each test gets a
//! private sandbox directory used both as the child process's working
//! directory (so `look`/`run` see files that exist only for this test)
//! and, via `TETEL_STATE_HOME`, as the root its workspace state lives
//! under — so tests never share state and never touch a real user's
//! `~/.local/state/tetel`.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tetel-authoring-cli-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Sandbox { dir }
    }

    fn state_home(&self) -> PathBuf {
        self.dir.join("state-home")
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
        path
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tetel"));
        cmd.args(args);
        cmd.current_dir(&self.dir);
        cmd.env("TETEL_STATE_HOME", self.state_home());
        cmd
    }

    /// Run with no stdin.
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = self.command(args).output().expect("failed to run tetel binary");
        (
            out.status.code().expect("process should exit normally"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// Run, feeding `input` on stdin.
    fn run_stdin(&self, args: &[&str], input: &str) -> (i32, String, String) {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn tetel binary");
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (
            out.status.code().expect("process should exit normally"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn facts_jsonl(&self) -> String {
        std::fs::read_to_string(self.state_home().join("workspaces/default/facts.jsonl")).unwrap_or_default()
    }

    fn claims_jsonl(&self) -> String {
        std::fs::read_to_string(self.state_home().join("workspaces/default/claims.jsonl")).unwrap_or_default()
    }

    fn prose_jsonl(&self) -> String {
        std::fs::read_to_string(self.state_home().join("workspaces/default/prose.jsonl")).unwrap_or_default()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn fact_is_refused_on_an_empty_buffer() {
    let sb = Sandbox::new("empty-buffer");
    let (code, _out, err) = sb.run(&["fact", "--note", "nothing was looked at"]);
    assert_ne!(code, 0, "minting a fact with no prior look/run must be refused");
    assert!(err.contains("pending observation buffer is empty"), "stderr was:\n{err}");
    assert!(sb.facts_jsonl().is_empty(), "a refused fact must not be logged");
}

#[test]
fn fact_has_no_flag_to_supply_extent_or_output_directly() {
    // There is no code path by which a caller can supply the extent
    // (or the captured output) themselves — clap must reject the flag
    // outright, not merely ignore it.
    let sb = Sandbox::new("no-extent-flag");
    let (code, _out, err) = sb.run(&["fact", "--extent", "src/lib.rs", "--note", "trying to fake an extent"]);
    assert_ne!(code, 0);
    assert!(err.contains("unexpected argument") || err.contains("unrecognized"), "stderr was:\n{err}");
    assert!(sb.facts_jsonl().is_empty());
}

#[test]
fn claim_is_refused_without_from_and_on_unknown_fact_ids() {
    let sb = Sandbox::new("claim-refusals");

    let (code, _out, err) = sb.run(&["claim", "--proposition", "a claim resting on nothing"]);
    assert_ne!(code, 0, "a claim with no --from must be refused");
    assert!(err.contains("--cites"), "stderr was:\n{err}");

    let (code, _out, err) = sb.run(&["claim", "--proposition", "x", "--cites", "F999"]);
    assert_ne!(code, 0, "a claim citing an unknown fact id must be refused");
    assert!(err.contains("F999"), "stderr was:\n{err}");
    assert!(sb.claims_jsonl().is_empty(), "no claim must be logged from either refusal");
}

#[test]
fn overlap_fires_across_two_different_line_ranges_of_one_file() {
    // Fix 2: the prototype keyed overlap on the literal command string,
    // so different `sed -n` ranges of the same file never overlapped
    // each other. `look --lines` must key on the resolved path instead,
    // so two facts taken from different ranges of one file do overlap.
    let sb = Sandbox::new("overlap-lines");
    sb.write("src/lib.rs", &(1..=40).map(|n| format!("line {n}\n")).collect::<String>());

    let (code, _, err) = sb.run(&["look", "src/lib.rs", "--lines", "1:5"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    let (code, _, err) = sb.run(&["fact", "--note", "first five lines"]);
    assert_eq!(code, 0, "stderr:\n{err}");

    let (code, _, err) = sb.run(&["look", "src/lib.rs", "--lines", "20:25"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    let (code, _, err) = sb.run(&["fact", "--note", "a different range of the same file"]);
    assert_eq!(code, 0, "stderr:\n{err}");

    // A third fact from an unrelated file must never show up as overlap.
    sb.write("src/other.rs", "unrelated content\n");
    let (code, _, err) = sb.run(&["look", "src/other.rs"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    let (code, _, err) = sb.run(&["fact", "--note", "an unrelated file"]);
    assert_eq!(code, 0, "stderr:\n{err}");

    let (code, out, err) = sb.run(&["claim", "--proposition", "lib.rs starts with `line 1`", "--cites", "F1"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(out.contains("F2"), "the second range of the SAME file must overlap:\n{out}");
    assert!(!out.contains("F3"), "an unrelated file's fact must never overlap:\n{out}");
}

#[test]
fn zero_match_grep_is_recorded_as_an_explicit_observation() {
    let sb = Sandbox::new("zero-match-grep");
    sb.write("src/lib.rs", "nothing interesting here\n");

    let (code, out, _err) = sb.run(&["look", "--grep", "DOES_NOT_OCCUR", "src"]);
    assert_eq!(code, 0);
    assert!(out.contains("no matches"), "output was:\n{out}");

    // The no-match observation must be fact-worthy: minting must succeed.
    let (code, _out, err) = sb.run(&["fact", "--note", "confirmed DOES_NOT_OCCUR is absent from src"]);
    assert_eq!(code, 0, "a zero-match grep must still leave a trace in the buffer:\n{err}");

    let (code, out, _err) = sb.run(&["query", "facts"]);
    assert_eq!(code, 0);
    assert!(out.contains("no-match: DOES_NOT_OCCUR in src"), "output was:\n{out}");
}

#[test]
fn fact_revision_keeps_the_old_note_verbatim() {
    let sb = Sandbox::new("fact-revise");
    sb.write("src/lib.rs", "content\n");
    sb.run(&["look", "src/lib.rs"]);
    sb.run(&["fact", "--note", "the original note"]);

    let (code, _out, err) = sb.run(&["fact", "--revise", "F1", "--note", "the corrected note", "--why", "the original was imprecise"]);
    assert_eq!(code, 0, "stderr:\n{err}");

    let log = sb.facts_jsonl();
    assert!(log.contains("the original note"), "the superseded note must survive verbatim in the log:\n{log}");
    assert!(log.contains("the corrected note"));
    assert!(log.contains("the original was imprecise"));

    let (_, out, _) = sb.run(&["query", "facts"]);
    assert!(out.contains("the corrected note"), "the current view must show the new note:\n{out}");
    assert!(!out.contains("the original note"), "the current view must not show the superseded note:\n{out}");
}

#[test]
fn fact_revision_without_why_is_refused() {
    let sb = Sandbox::new("fact-revise-no-why");
    sb.write("src/lib.rs", "content\n");
    sb.run(&["look", "src/lib.rs"]);
    sb.run(&["fact", "--note", "original"]);

    let (code, _out, err) = sb.run(&["fact", "--revise", "F1", "--note", "new"]);
    assert_ne!(code, 0, "a fact revision without --why must be refused");
    assert!(err.contains("--why"), "stderr was:\n{err}");
}

#[test]
fn claim_revision_keeps_the_old_proposition_verbatim() {
    let sb = Sandbox::new("claim-revise");
    sb.write("src/lib.rs", "content\n");
    sb.run(&["look", "src/lib.rs"]);
    sb.run(&["fact", "--note", "a fact"]);
    sb.run(&["claim", "--proposition", "the original proposition", "--cites", "F1"]);

    let (code, _out, err) = sb.run(&["claim", "--revise", "C1", "--proposition", "the revised proposition", "--why", "narrowed after review"]);
    assert_eq!(code, 0, "stderr:\n{err}");

    let log = sb.claims_jsonl();
    assert!(log.contains("the original proposition"), "log:\n{log}");
    assert!(log.contains("the revised proposition"));
    assert!(log.contains("narrowed after review"));
}

#[test]
fn render_omits_superseded_prose_text() {
    let sb = Sandbox::new("render-supersede");
    let (code, _out, err) = sb.run_stdin(&["prose"], "the original wording");
    assert_eq!(code, 0, "stderr:\n{err}");

    let (code, _out, err) = sb.run(&["prose", "--revise", "P1", "--why", "clumsy phrasing", "--text", "the improved wording"]);
    assert_eq!(code, 0, "stderr:\n{err}");

    let (code, out, err) = sb.run(&["render"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(out.contains("the improved wording"), "output was:\n{out}");
    assert!(!out.contains("the original wording"), "a superseded revision must not render:\n{out}");

    // The superseded text must still exist in the append-only log, just
    // never in the rendered document.
    let log = sb.prose_jsonl();
    assert!(log.contains("the original wording"), "log:\n{log}");
}

#[test]
fn heading_levels_are_not_flattened_to_one_depth() {
    let sb = Sandbox::new("heading-levels");
    let (code, _out, err) = sb.run(&["prose", "--heading", "Top", "--level", "1"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    let (code, _out, err) = sb.run(&["prose", "--heading", "Deep", "--level", "4"]);
    assert_eq!(code, 0, "stderr:\n{err}");

    let (code, out, err) = sb.run(&["render"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(out.contains("# Top\n"), "output was:\n{out}");
    assert!(out.contains("#### Deep\n"), "output was:\n{out}");
    assert!(!out.contains("## Top"), "must not flatten every heading to one fixed depth:\n{out}");
}

#[test]
fn heading_level_out_of_range_is_refused() {
    let sb = Sandbox::new("heading-bad-level");
    let (code, _out, err) = sb.run(&["prose", "--heading", "Broken", "--level", "9"]);
    assert_ne!(code, 0, "a heading level outside 1..=6 must be refused");
    assert!(err.contains("1..=6") || err.contains("level"), "stderr was:\n{err}");
}

#[test]
fn note_round_trips_byte_exact_through_stdin_with_backticks_quotes_and_newlines() {
    let sb = Sandbox::new("byte-exact");
    sb.write("src/lib.rs", "content\n");
    sb.run(&["look", "src/lib.rs"]);

    let note = "line one\nline two with `backticks` and a \"quote\"\nline three";
    let (code, _out, err) = sb.run_stdin(&["fact", "--note", "-"], note);
    assert_eq!(code, 0, "stderr:\n{err}");

    let log = sb.facts_jsonl();
    let parsed: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(parsed["note"], note, "the note must decode back byte-exact");
}

#[test]
fn prose_text_round_trips_byte_exact_through_stdin() {
    let sb = Sandbox::new("prose-byte-exact");
    let text = "a paragraph with `code`, a \"quoted phrase\", and\nan embedded newline";
    let (code, _out, err) = sb.run_stdin(&["prose"], text);
    assert_eq!(code, 0, "stderr:\n{err}");

    let log = sb.prose_jsonl();
    let parsed: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(parsed["text"], text);

    let (_, out, _) = sb.run(&["render"]);
    assert!(out.contains(text), "output was:\n{out}");
}

#[test]
fn look_refuses_a_nonexistent_path_and_a_directory() {
    let sb = Sandbox::new("look-refusals");
    let (code, _out, err) = sb.run(&["look", "no-such-file.rs"]);
    assert_ne!(code, 0);
    assert!(err.contains("no such path"), "stderr was:\n{err}");

    sb.write("src/mod.rs", "x\n");
    let (code, _out, err) = sb.run(&["look", "src"]);
    assert_ne!(code, 0, "look must refuse a directory, pointing at --grep instead");
    assert!(err.contains("--grep"), "stderr was:\n{err}");
}

#[test]
fn run_captures_output_and_mirrors_exit_code() {
    let sb = Sandbox::new("run-basic");
    let (code, out, _err) = sb.run(&["run", "echo", "hello from run"]);
    assert_eq!(code, 0);
    assert!(out.contains("hello from run"), "output was:\n{out}");

    let (code, _out, _err) = sb.run(&["fact", "--note", "ran echo successfully"]);
    assert_eq!(code, 0, "a run observation must be fact-worthy");
}

#[test]
fn query_deps_reports_what_a_fact_is_cited_by_and_what_a_claim_rests_on() {
    let sb = Sandbox::new("query-deps");
    sb.write("src/lib.rs", "content\n");
    sb.run(&["look", "src/lib.rs"]);
    sb.run(&["fact", "--note", "a fact"]);
    sb.run(&["claim", "--proposition", "a claim", "--cites", "F1"]);

    let (_, out, _) = sb.run(&["query", "deps", "F1"]);
    assert!(out.contains("C1"), "output was:\n{out}");

    let (_, out, _) = sb.run(&["query", "deps", "C1"]);
    assert!(out.contains("F1"), "output was:\n{out}");
}

#[test]
fn render_appends_a_checkable_evidence_ledger_without_altering_prose_bytes() {
    // Fix 1: `tetel check` on a document `tetel render` just produced
    // used to report "no tetel rows found" forever — the authoring half
    // and the verification half never connected. `render` now appends
    // an evidence-ledger table in the shape `ledger::import` already
    // reads; this test drives the CLI end to end (`look`/`fact`/`claim`/
    // `prose`/`render`/`check`) to prove the loop actually closes.
    let sb = Sandbox::new("render-ledger");
    sb.write("src/lib.rs", "fn foo() {}\n");
    sb.run(&["look", "src/lib.rs"]);
    sb.run(&["fact", "--note", "foo is defined in lib.rs"]);
    sb.run(&["claim", "--proposition", "foo exists", "--cites", "F1"]);
    sb.run_stdin(&["prose", "--cites", "C1"], "Foo exists in the codebase.");

    let (code, out, err) = sb.run(&["render"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(
        out.starts_with("Foo exists in the codebase.\n"),
        "prose must render first and unchanged, ledger only appended after it:\n{out}"
    );
    assert!(out.contains("## Evidence ledger"), "output was:\n{out}");
    assert!(out.contains("| C1 | foo exists |"), "output was:\n{out}");

    let memo_path = sb.write("rendered-memo.md", &out);
    let mut check_cmd = std::process::Command::new(env!("CARGO_BIN_EXE_tetel"));
    check_cmd.arg("check").arg(&memo_path);
    let check_out = check_cmd.output().expect("failed to run tetel binary");
    let check_stdout = String::from_utf8_lossy(&check_out.stdout).into_owned();
    assert_eq!(check_out.status.code(), Some(0), "stdout:\n{check_stdout}");
    assert!(
        !check_stdout.contains("no tetel rows found"),
        "a rendered memo must connect to `check`, not read as empty:\n{check_stdout}"
    );
    assert!(
        !check_stdout.contains("no evidence ledger found"),
        "output was:\n{check_stdout}"
    );
    assert!(
        check_stdout.contains("C1: ungrounded"),
        "an unrecorded claim must show up as a real, human-owed finding:\n{check_stdout}"
    );
    // The absent scope field must still be named plainly rather than
    // silently passed — but exactly once. It is a fact about tetel's
    // authoring model, identical for every claim and dischargeable by
    // nobody; one line per claim buried the findings that are about this
    // document under a constant.
    assert!(
        check_stdout.contains("no claim in this document declares a scope"),
        "the absent scope field must be named plainly, not silently passed:\n{check_stdout}"
    );
    assert_eq!(
        check_stdout.matches("declares a scope").count(),
        1,
        "said once, not once per claim:\n{check_stdout}"
    );
}

#[test]
fn render_with_no_claims_appends_no_evidence_ledger() {
    let sb = Sandbox::new("render-no-claims");
    sb.run_stdin(&["prose"], "just a paragraph, nothing cited");
    let (code, out, err) = sb.run(&["render"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(!out.contains("Evidence ledger"), "no claims exist; nothing to append:\n{out}");
}

#[test]
fn inline_backtick_warns_on_stderr_but_at_file_and_stdin_do_not() {
    // Real first-use evidence: every substantial `--note`/`--prop` on
    // this project hit shell command substitution on the very first
    // inline attempt, always worked around afterward by falling back to
    // `@file`. The warning must fire exactly on that inline path, and
    // stay silent for the two paths that never touch a shell at all.
    let sb = Sandbox::new("backtick-warning");
    sb.write("src/lib.rs", "content\n");

    sb.run(&["look", "src/lib.rs"]);
    let (code, _out, err) = sb.run(&["fact", "--note", "uses `backticks` inline"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(err.contains("backtick"), "an inline backtick must warn on stderr:\n{err}");

    sb.run(&["look", "src/lib.rs"]);
    let note_path = sb.write("note.txt", "uses `backticks` via file");
    let (code, _out, err) = sb.run(&["fact", "--note", &format!("@{}", note_path.display())]);
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(!err.contains("backtick"), "`@file` must never warn:\n{err}");

    sb.run(&["look", "src/lib.rs"]);
    let (code, _out, err) = sb.run_stdin(&["fact", "--note", "-"], "uses `backticks` via stdin");
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(!err.contains("backtick"), "stdin must never warn:\n{err}");
}

#[test]
fn brief_authoring_mode_needs_no_memo_and_is_self_contained() {
    let sb = Sandbox::new("brief-authoring");
    let (code, out, err) = sb.run(&["brief", "--authoring"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(out.contains("tetel claim --revise"), "output was:\n{out}");
    assert!(!out.contains("/Volumes/"), "output was:\n{out}");
    assert!(!out.contains("/Users/"), "output was:\n{out}");
}

/// `tetel workspaces` is the one question that cannot be answered from
/// inside a workspace, so it takes no `--workspace` of its own.
#[test]
fn workspaces_lists_every_workspace_with_its_counts() {
    let sb = Sandbox::new("workspaces-list");
    sb.write("a.txt", "alpha\n");

    sb.run(&["--workspace", "one", "look", "a.txt"]);
    sb.run(&["--workspace", "one", "fact", "--note", "a note"]);
    sb.run(&["--workspace", "one", "claim", "--proposition", "a claim", "--cites", "F1"]);
    sb.run_stdin(&["--workspace", "one", "prose", "--cites", "C1"], "Some prose.");

    // A second workspace with strictly less in it, to prove the counts
    // are per-workspace rather than global.
    sb.run(&["--workspace", "two", "look", "a.txt"]);
    sb.run(&["--workspace", "two", "fact", "--note", "another note"]);

    let (code, out, err) = sb.run(&["workspaces"]);
    assert_eq!(code, 0, "stderr was:\n{err}");

    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "expected exactly two workspaces, got:\n{out}");
    // Name-sorted, so `one` precedes `two` regardless of creation order.
    assert!(lines[0].starts_with("one\t"), "got: {}", lines[0]);
    assert!(lines[0].contains("1 facts") && lines[0].contains("1 claims") && lines[0].contains("1 prose"), "got: {}", lines[0]);
    assert!(lines[1].starts_with("two\t"), "got: {}", lines[1]);
    assert!(lines[1].contains("1 facts") && lines[1].contains("0 claims") && lines[1].contains("0 prose"), "got: {}", lines[1]);
}

/// An empty list is an ordinary state, not an error — and it must say
/// where it looked, so it is never mistaken for looking in the wrong
/// place. It must also not create the root as a side effect of asking.
#[test]
fn workspaces_on_a_fresh_machine_reports_empty_without_creating_anything() {
    let sb = Sandbox::new("workspaces-empty");
    let (code, out, err) = sb.run(&["workspaces"]);
    assert_eq!(code, 0, "stderr was:\n{err}");
    assert!(out.contains("no workspaces yet"), "got: {out}");
    assert!(
        !sb.state_home().join("workspaces").exists(),
        "asking what exists must not create the root"
    );
}

/// The regression behind the citation-scanner fix, end to end: a
/// document authored through `prose --cites` and produced by `render`
/// must not have every one of its claims reported "defined but never
/// cited", whose stated default disposition is to delete them.
#[test]
fn a_rendered_documents_own_citations_are_not_reported_as_uncited() {
    let sb = Sandbox::new("rendered-citations-scan");
    sb.write("a.txt", "alpha\n");
    sb.run(&["look", "a.txt"]);
    sb.run(&["fact", "--note", "a.txt begins with alpha"]);
    sb.run(&["claim", "--proposition", "the file begins with alpha", "--cites", "F1"]);
    sb.run_stdin(&["prose", "--cites", "C1"], "The file begins with alpha.");

    let (_code, rendered, _err) = sb.run(&["render"]);
    assert!(rendered.contains("*cites: C1*"), "render should emit the trailer:\n{rendered}");
    sb.write("memo.md", &rendered);

    let (_code, report, err) = sb.run(&["check", "memo.md"]);
    let combined = format!("{report}{err}");
    assert!(
        !combined.contains("C1: defined but never cited"),
        "check must read render's own citation syntax; report was:\n{combined}"
    );
}

/// `render --out` must write the document and the snapshot its citations
/// point into as one act, and the result must check clean.
#[test]
fn render_out_writes_a_snapshot_that_checks_clean() {
    let sb = Sandbox::new("render-out");
    sb.write("a.txt", "alpha\n");
    sb.run(&["look", "a.txt"]);
    sb.run(&["fact", "--note", "a.txt begins with alpha"]);
    sb.run(&["claim", "--proposition", "the file begins with alpha", "--cites", "F1"]);
    sb.run_stdin(&["prose", "--cites", "C1"], "The file begins with alpha.");

    let memo = sb.dir.join("memo.md");
    let (code, out, err) = sb.run(&["render", "--out", memo.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr was:\n{err}");
    assert!(out.contains("snapshot in"), "got: {out}");

    // The snapshot carries the whole workspace, not just what render read.
    let snap = sb.dir.join("memo.md.tetel");
    for f in ["facts.jsonl", "claims.jsonl", "prose.jsonl", "counters.json"] {
        assert!(snap.join(f).exists(), "snapshot missing {f}");
    }

    // Writing the document must not change what it renders to.
    let (_c, stdout_render, _e) = sb.run(&["render"]);
    assert_eq!(
        std::fs::read_to_string(&memo).unwrap(),
        stdout_render,
        "--out must write exactly what bare render prints"
    );

    let (_code, report, err) = sb.run(&["check", memo.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    assert!(!combined.contains("provenance-drift"), "should not drift:\n{combined}");
    assert!(!combined.contains("no workspace snapshot"), "snapshot exists:\n{combined}");
}

/// Hand-editing a rendered memo makes it stop matching its own record.
/// That is a machine failure, not a human-owed note: it is decidable
/// without a human, and a reader following a citation would land
/// somewhere the text was never produced from.
#[test]
fn hand_editing_a_rendered_memo_is_caught_as_drift() {
    let sb = Sandbox::new("drift");
    sb.write("a.txt", "alpha\n");
    sb.run(&["look", "a.txt"]);
    sb.run(&["fact", "--note", "a.txt begins with alpha"]);
    sb.run(&["claim", "--proposition", "the file begins with alpha", "--cites", "F1"]);
    sb.run_stdin(&["prose", "--cites", "C1"], "The file begins with alpha.");

    let memo = sb.dir.join("memo.md");
    sb.run(&["render", "--out", memo.to_str().unwrap()]);

    // The edit a human would actually make: strengthening a sentence in
    // the document without touching the record behind it.
    let text = std::fs::read_to_string(&memo).unwrap();
    std::fs::write(&memo, text.replace("begins with alpha", "always begins with alpha")).unwrap();

    let (code, report, err) = sb.run(&["check", memo.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    assert_eq!(code, 1, "drift must fail the check; report was:\n{combined}");
    assert!(combined.contains("provenance-drift"), "got:\n{combined}");
    assert!(combined.contains("first difference at line"), "got:\n{combined}");
}

/// A memo that cites nothing owes no snapshot; one that cites something
/// and has none is reported, but not failed — every memo authored before
/// `render --out` existed lacks one, and failing those would grade the
/// tooling's history rather than the document.
#[test]
fn a_missing_snapshot_is_reported_only_when_the_memo_cites_something() {
    let sb = Sandbox::new("missing-snapshot");
    sb.write("a.txt", "alpha\n");
    sb.run(&["look", "a.txt"]);
    sb.run(&["fact", "--note", "a.txt begins with alpha"]);
    sb.run(&["claim", "--proposition", "the file begins with alpha", "--cites", "F1"]);
    sb.run_stdin(&["prose", "--cites", "C1"], "The file begins with alpha.");

    // The real missing-snapshot case: a rendered memo saved by shell
    // redirect, so tetel never learned where it landed and wrote no
    // record. This is every memo authored before `render --out` existed.
    let (_c, rendered, _e) = sb.run(&["render"]);
    sb.write("redirected.md", &rendered);
    let (code, report, err) = sb.run(&["check", "redirected.md"]);
    let combined = format!("{report}{err}");
    assert!(combined.contains("no workspace snapshot"), "got:\n{combined}");
    assert_ne!(code, 1, "a missing snapshot is human-owed, never a failure");

    // A memo whose prose cites nothing still has a ledger, but has no
    // workspace-relative pointer for a reader to fail to resolve — so it
    // owes no snapshot and must not be nagged about one.
    let sb2 = Sandbox::new("missing-snapshot-nocites");
    sb2.write("a.txt", "alpha\n");
    sb2.run(&["look", "a.txt"]);
    sb2.run(&["fact", "--note", "a.txt begins with alpha"]);
    sb2.run(&["claim", "--proposition", "the file begins with alpha", "--cites", "F1"]);
    sb2.run_stdin(&["prose"], "Prose that cites nothing at all.");

    let (_c, rendered2, _e) = sb2.run(&["render"]);
    assert!(!rendered2.contains("*cites:"), "fixture should carry no citations:\n{rendered2}");
    sb2.write("nocites.md", &rendered2);
    let (_code2, report2, err2) = sb2.run(&["check", "nocites.md"]);
    let combined2 = format!("{report2}{err2}");
    assert!(
        !combined2.contains("no workspace snapshot"),
        "a memo citing nothing owes no snapshot:\n{combined2}"
    );
}

/// A fact cited from prose must resolve in the rendered document. Before
/// the facts table existed, `render` emitted `*cites: C2, F5*` while no
/// fact appeared anywhere in the output, so every such citation reported
/// as `cited but undefined` — the renderer emitting an id the checker had
/// no table to resolve it in.
#[test]
fn a_fact_cited_from_prose_resolves_in_the_rendered_document() {
    let sb = Sandbox::new("facts-table");
    sb.write("a.txt", "alpha\n");
    sb.run(&["look", "a.txt"]);
    sb.run(&["fact", "--note", "a.txt begins with alpha"]);
    sb.run(&["claim", "--proposition", "the file begins with alpha", "--cites", "F1"]);
    sb.run_stdin(&["prose", "--cites", "C1,F1"], "The file begins with alpha.");

    let memo = sb.dir.join("memo.md");
    sb.run(&["render", "--out", memo.to_str().unwrap()]);

    let rendered = std::fs::read_to_string(&memo).unwrap();
    assert!(rendered.contains("## Facts"), "render should carry a facts table:\n{rendered}");
    assert!(rendered.contains("Extent (captured)"), "extents belong in the document:\n{rendered}");
    // The extent is rendered as its repo-relative label, never the
    // absolute machine-local path a committed document must not carry.
    assert!(!rendered.contains("/Users/"), "no absolute paths in a rendered doc:\n{rendered}");

    let (_code, report, err) = sb.run(&["check", memo.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    assert!(
        !combined.contains("cited but undefined"),
        "a cited fact must resolve; report was:\n{combined}"
    );
}

/// `review` assembles the pairing that catching prose-claim drift needs.
/// Its motivating failure, from the FORK-94 memo: a paragraph opening
/// "never worse … a strict improvement" cited a claim about something
/// else entirely. Nothing detects that mechanically — the value is that
/// the two appear together instead of forty lines apart.
#[test]
fn review_puts_each_paragraph_beside_the_claims_it_cites() {
    let sb = Sandbox::new("review");
    sb.write("a.txt", "alpha\n");
    sb.run(&["look", "a.txt"]);
    sb.run(&["fact", "--note", "a.txt begins with alpha"]);
    sb.run(&["claim", "--proposition", "the file begins with alpha", "--cites", "F1"]);
    sb.run_stdin(&["prose", "--heading", "A heading", "--level", "2"], "");
    sb.run_stdin(&["prose", "--cites", "C1"], "The file begins with alpha, and is therefore fine.");
    sb.run_stdin(&["prose"], "A paragraph resting on nothing at all.");

    let (code, out, err) = sb.run(&["review"]);
    assert_eq!(code, 0, "stderr:\n{err}");

    // The paragraph and its claim must both appear, so they can be read
    // against each other.
    assert!(out.contains("is therefore fine"), "paragraph text missing:\n{out}");
    assert!(out.contains("cites C1: the file begins with alpha"), "claim text missing:\n{out}");

    // A heading is structure, not an assertion, so it is not listed as
    // owing a claim.
    assert!(!out.contains("A heading"), "headings must not be listed:\n{out}");

    // A paragraph citing nothing is the shape worth looking at hardest,
    // so it is kept and grouped rather than dropped.
    assert!(out.contains("paragraphs citing nothing"), "uncited section missing:\n{out}");
    assert!(out.contains("resting on nothing at all"), "uncited paragraph missing:\n{out}");

    // No score, no percentage, no aggregate anywhere in the output.
    assert!(!out.contains('%'), "review must not report a score:\n{out}");
}

/// A fact cited from prose is legitimate but is not a claim; the pairing
/// says so rather than silently listing one fewer row than the
/// paragraph's own `*cites:*` line promises.
#[test]
fn review_names_a_cited_fact_rather_than_dropping_it() {
    let sb = Sandbox::new("review-fact");
    sb.write("a.txt", "alpha\n");
    sb.run(&["look", "a.txt"]);
    sb.run(&["fact", "--note", "a.txt begins with alpha"]);
    sb.run(&["claim", "--proposition", "the file begins with alpha", "--cites", "F1"]);
    sb.run_stdin(&["prose", "--cites", "C1,F1"], "Both cited here.");

    let (_code, out, _err) = sb.run(&["review"]);
    assert!(out.contains("cites F1: not a claim in this workspace"), "got:\n{out}");
}
