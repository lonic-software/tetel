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

/// The witnessed path: a grounding pass rests a verdict on a fact it
/// captured itself, and the record carries the identity of the workspace
/// that captured it. This is what makes "this pass was independent" a
/// property `check` recomputes rather than a string someone typed —
/// `pass` is validated only for being non-empty on the ingested path.
#[test]
fn record_from_fact_mints_a_witnessed_record_carrying_workspace_identity() {
    let sb = Sandbox::new("from-fact");
    sb.write("alpha.rs", "fn alpha() {}\n");

    // The author writes a memo.
    sb.run(&["--workspace", "author", "look", "alpha.rs"]);
    sb.run(&["--workspace", "author", "fact", "--note", "alpha.rs defines alpha()"]);
    sb.run(&["--workspace", "author", "claim", "--proposition", "alpha.rs defines alpha()", "--cites", "F1"]);
    sb.run_stdin(&["--workspace", "author", "prose", "--cites", "C1"], "The file defines alpha().");
    let memo = sb.dir.join("memo.md");
    sb.run(&["--workspace", "author", "render", "--out", memo.to_str().unwrap()]);

    // The grounding pass is a *fresh workspace* making its own
    // observation — that is what "independent" means structurally here.
    sb.run(&["--workspace", "grounder", "look", "alpha.rs"]);
    sb.run(&["--workspace", "grounder", "fact", "--note", "read independently"]);
    let (code, out, err) = sb.run(&[
        "--workspace", "grounder", "record", memo.to_str().unwrap(),
        "--from-fact", "F1", "--claim", "C1", "--verdict", "supports",
    ]);
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(out.contains("witnessed, workspace"), "got: {out}");

    let raw = std::fs::read_to_string(sb.dir.join("memo.md.evidence.jsonl")).unwrap();
    let rec: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
    assert_eq!(rec["predicateType"], tetel::evidence::CAPTURED_PREDICATE_TYPE);

    // The extent was copied from the grounder's own fact, not typed —
    // there is no flag by which a caller could supply one.
    assert_eq!(rec["predicate"]["extent"][0], "alpha.rs");
    // The pass is the workspace identity, not a name someone chose.
    let pass = rec["predicate"]["pass"].as_str().unwrap();
    assert!(!pass.is_empty() && pass != "grounder", "pass must be the identity, got: {pass}");

    let (_c, report, err) = sb.run(&["check", memo.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    assert!(combined.contains("1 of 1 record(s) witnessed"), "got:\n{combined}");
    assert!(
        combined.contains("remains owed to the run protocol"),
        "what a record cannot establish must be said, not implied:\n{combined}"
    );
}

/// A witnessed record and an ingested one for the same claim must remain
/// distinguishable — the whole point of separate predicate types.
#[test]
fn witnessed_and_ingested_records_are_counted_apart() {
    let sb = Sandbox::new("witnessed-vs-ingested");
    sb.write("alpha.rs", "fn alpha() {}\n");
    sb.run(&["--workspace", "a", "look", "alpha.rs"]);
    sb.run(&["--workspace", "a", "fact", "--note", "alpha.rs defines alpha()"]);
    sb.run(&["--workspace", "a", "claim", "--proposition", "alpha.rs defines alpha()", "--cites", "F1"]);
    sb.run_stdin(&["--workspace", "a", "prose", "--cites", "C1"], "Defines alpha().");
    let memo = sb.dir.join("memo.md");
    sb.run(&["--workspace", "a", "render", "--out", memo.to_str().unwrap()]);

    sb.run(&["--workspace", "a", "record", memo.to_str().unwrap(),
             "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);
    sb.run_stdin(
        &["record", memo.to_str().unwrap()],
        r#"{"claim":"C1","pass":"i-said-so","verdict":"supports","reported_kind":"run","source":"proc:someone","extent":["whatever I typed"]}"#,
    );

    let (_c, report, err) = sb.run(&["check", memo.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    assert!(combined.contains("1 of 2 record(s) witnessed"), "got:\n{combined}");
}

/// An ingested-only claim must say so plainly: its extent was typed by
/// the reporter, and its `pass` is whatever the reporter wrote.
#[test]
fn an_ingested_only_claim_is_named_as_such() {
    let sb = Sandbox::new("ingested-only");
    sb.write("alpha.rs", "fn alpha() {}\n");
    sb.run(&["--workspace", "a", "look", "alpha.rs"]);
    sb.run(&["--workspace", "a", "fact", "--note", "alpha.rs defines alpha()"]);
    sb.run(&["--workspace", "a", "claim", "--proposition", "alpha.rs defines alpha()", "--cites", "F1"]);
    sb.run_stdin(&["--workspace", "a", "prose", "--cites", "C1"], "Defines alpha().");
    let memo = sb.dir.join("memo.md");
    sb.run(&["--workspace", "a", "render", "--out", memo.to_str().unwrap()]);
    sb.run_stdin(
        &["record", memo.to_str().unwrap()],
        r#"{"claim":"C1","pass":"whatever","verdict":"supports","reported_kind":"run","source":"proc:someone","extent":["typed"]}"#,
    );

    let (_c, report, err) = sb.run(&["check", memo.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    assert!(combined.contains("all 1 record(s) ingested"), "got:\n{combined}");
    assert!(combined.contains("not captured by this tool"), "got:\n{combined}");
}

/// Author a memo in `author`, returning its path. Shared by the
/// self-versus-independent grounding tests below.
fn memo_authored_by(sb: &Sandbox) -> PathBuf {
    sb.write("alpha.rs", "fn alpha() {}\n");
    sb.run(&["--workspace", "author", "look", "alpha.rs"]);
    sb.run(&["--workspace", "author", "fact", "--note", "alpha.rs defines alpha()"]);
    sb.run(&["--workspace", "author", "claim", "--proposition", "alpha.rs defines alpha()", "--cites", "F1"]);
    sb.run_stdin(&["--workspace", "author", "prose", "--cites", "C1"], "Defines alpha().");
    let memo = sb.dir.join("memo.md");
    sb.run(&["--workspace", "author", "render", "--out", memo.to_str().unwrap()]);
    memo
}

/// The distinction the mechanism exists for. An author grounding their own
/// claims is the arrangement measured at 78% scope-equal — no better than
/// hand-authored rows. Independence is what moved that to 33%. A check
/// that reported both the same way would be confidently wrong, so the
/// snapshot ships the authoring workspace's identity to make them
/// distinguishable.
#[test]
fn an_author_grounding_their_own_claim_is_reported_as_self_grounded() {
    let sb = Sandbox::new("self-ground");
    let memo = memo_authored_by(&sb);

    sb.run(&["--workspace", "author", "record", memo.to_str().unwrap(),
             "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);

    let (_c, report, err) = sb.run(&["check", memo.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    assert!(combined.contains("SELF-GROUNDED"), "got:\n{combined}");
    assert!(combined.contains("no independent pass has run"), "got:\n{combined}");
}

#[test]
fn a_fresh_workspace_grounding_the_claim_is_reported_as_independent() {
    let sb = Sandbox::new("independent-ground");
    let memo = memo_authored_by(&sb);

    sb.run(&["--workspace", "grounder", "look", "alpha.rs"]);
    sb.run(&["--workspace", "grounder", "fact", "--note", "read independently"]);
    sb.run(&["--workspace", "grounder", "record", memo.to_str().unwrap(),
             "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);

    let (_c, report, err) = sb.run(&["check", memo.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    assert!(combined.contains("independently grounded"), "got:\n{combined}");
    assert!(!combined.contains("SELF-GROUNDED"), "got:\n{combined}");
}

/// Both at once must report both. Collapsing to one verdict would either
/// hide a real independent pass or hide that the author graded their own
/// work — each misleading in a different direction.
#[test]
fn a_claim_grounded_by_both_reports_both() {
    let sb = Sandbox::new("mixed-ground");
    let memo = memo_authored_by(&sb);

    sb.run(&["--workspace", "grounder", "look", "alpha.rs"]);
    sb.run(&["--workspace", "grounder", "fact", "--note", "read independently"]);
    sb.run(&["--workspace", "grounder", "record", memo.to_str().unwrap(),
             "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);
    sb.run(&["--workspace", "author", "record", memo.to_str().unwrap(),
             "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);

    let (_c, report, err) = sb.run(&["check", memo.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    assert!(combined.contains("MIXED"), "got:\n{combined}");
    assert!(combined.contains("not as independent confirmation"), "got:\n{combined}");
}

/// Without a snapshot there is no authoring identity to compare against,
/// and the check says that rather than guessing either way.
#[test]
fn without_a_snapshot_the_distinction_is_reported_as_undeterminable() {
    let sb = Sandbox::new("no-snapshot-ground");
    let memo = memo_authored_by(&sb);
    let bare = sb.dir.join("bare.md");
    std::fs::copy(&memo, &bare).unwrap();

    sb.run(&["--workspace", "author", "record", bare.to_str().unwrap(),
             "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);

    let (_c, report, err) = sb.run(&["check", bare.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    assert!(combined.contains("cannot be determined from here"), "got:\n{combined}");
}

/// Build a one-claim memo, returning its path. Fixtures for the
/// verdict-policy tests are authored here rather than copied from any
/// real corpus — this repo is public and must carry no private memo's
/// text or paths.
fn one_claim_memo(sb: &Sandbox) -> PathBuf {
    sb.write("alpha.rs", "fn alpha() {}\n");
    sb.run(&["look", "alpha.rs"]);
    sb.run(&["fact", "--note", "alpha.rs defines alpha()"]);
    sb.run(&["claim", "--proposition", "alpha.rs defines alpha()", "--cites", "F1"]);
    sb.run_stdin(&["prose", "--cites", "C1"], "Defines alpha().");
    let memo = sb.dir.join("memo.md");
    sb.run(&["render", "--out", memo.to_str().unwrap()]);
    memo
}

/// `supports` beside `qualifies` is not a contradiction, and must not
/// redden the machine partition. It is what an honest grounder produces
/// when a claim rests on several facts and one premise holds only under
/// a condition — and `record --from-fact` takes one fact per record, so
/// that shape is the tool's own, not a misuse of it.
#[test]
fn supports_beside_qualifies_is_not_a_machine_failure() {
    let sb = Sandbox::new("verdict-mixed");
    let memo = one_claim_memo(&sb);
    let m = memo.to_str().unwrap();

    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);
    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "qualifies",
             "--note", "holds only for the single-threaded path"]);

    let (code, report, err) = sb.run(&["check", m]);
    let combined = format!("{report}{err}");
    assert_ne!(code, 1, "supports + qualifies must not fail the machine check:\n{combined}");
    assert!(!combined.contains("[verdict-disagreement]"), "got:\n{combined}");
    // But it must be visible: a qualification is the most load-bearing
    // thing a grounding pass produces, and it used to print nowhere.
    assert!(combined.contains("C1: QUALIFIED"), "got:\n{combined}");
    assert!(combined.contains("single-threaded path"), "the note must be quoted:\n{combined}");
}

/// `supports` and `refutes` on one proposition is P and not-P — still a
/// machine failure, regardless of which pass wrote either.
#[test]
fn supports_and_refutes_still_fails_the_machine_check() {
    let sb = Sandbox::new("verdict-contradiction");
    let memo = one_claim_memo(&sb);
    let m = memo.to_str().unwrap();

    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);
    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "refutes"]);

    let (code, report, err) = sb.run(&["check", m]);
    let combined = format!("{report}{err}");
    assert_eq!(code, 1, "a real contradiction must still redden:\n{combined}");
    assert!(combined.contains("[verdict-disagreement]"), "got:\n{combined}");
}

/// The old check compared only *adjacent* records, so with the ordering
/// `supports, qualifies, refutes` it reported the two qualifies pairs and
/// never compared (1,3) — the one real contradiction went unmentioned
/// while the report reddened for the wrong reason. Set-based comparison
/// fixes both halves.
#[test]
fn a_contradiction_is_found_whatever_order_the_records_arrive_in() {
    let sb = Sandbox::new("verdict-ordering");
    let memo = one_claim_memo(&sb);
    let m = memo.to_str().unwrap();

    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);
    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "qualifies",
             "--note", "only under condition X"]);
    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "refutes"]);

    let (code, report, err) = sb.run(&["check", m]);
    let combined = format!("{report}{err}");
    assert_eq!(code, 1, "the supports/refutes pair must be found:\n{combined}");
    // Exactly one finding for the claim, not one per adjacent pair.
    assert_eq!(
        combined.matches("[verdict-disagreement]").count(),
        1,
        "one finding per claim, not per pair:\n{combined}"
    );
    assert!(combined.contains("C1: QUALIFIED"), "the qualification still prints:\n{combined}");
}

/// A bare `qualifies` is uninterpretable: which of "holds under a
/// condition" and "I could not establish this" it means, and what the
/// condition is, exist nowhere else. Format-level, so a refusal is
/// allowed where a heuristic one would not be.
#[test]
fn a_qualifies_verdict_without_a_note_is_refused() {
    let sb = Sandbox::new("verdict-bare-qualifies");
    let memo = one_claim_memo(&sb);
    let m = memo.to_str().unwrap();

    let (code, _out, err) = sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1",
                                     "--verdict", "qualifies"]);
    assert_eq!(code, 1, "a bare qualifies must be refused");
    assert!(err.contains("needs a `note`"), "stderr:\n{err}");

    // And nothing was written.
    let ev = sb.dir.join("memo.md.evidence.jsonl");
    assert!(!ev.exists() || std::fs::read_to_string(&ev).unwrap().trim().is_empty());
}

/// Evidence grades a *proposition*, not an id. Revising a claim's text
/// after it was graded used to leave the old evidence attached — through
/// the ordinary `claim --revise` + `render --out` path, with the memo
/// still matching its snapshot, no drift, and the machine partition
/// clean. The tool reported evidence for a proposition nobody had
/// examined, and this test is the negation case: the claim now asserts
/// the exact opposite of what was graded.
#[test]
fn revising_a_graded_proposition_makes_its_evidence_stale() {
    let sb = Sandbox::new("stale-evidence");
    let memo = one_claim_memo(&sb);
    let m = memo.to_str().unwrap();

    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);
    let (code, report, err) = sb.run(&["check", m]);
    assert_ne!(code, 1, "clean before the revision:\n{report}{err}");

    sb.run(&["claim", "--revise", "C1", "--proposition", "alpha.rs does NOT define alpha()",
             "--why", "reversing the claim entirely"]);
    sb.run(&["render", "--out", m]);

    let (code, report, err) = sb.run(&["check", m]);
    let combined = format!("{report}{err}");
    assert_eq!(code, 1, "stale evidence must fail the machine check:\n{combined}");
    assert!(combined.contains("[stale-evidence]"), "got:\n{combined}");
    assert!(combined.contains("graded different text"), "got:\n{combined}");
    // The memo matches its snapshot, so drift is not what caught this.
    assert!(!combined.contains("[provenance-drift]"), "drift must not be the catcher:\n{combined}");
}

/// Re-grounding after the revision clears it: the new record carries the
/// new text's digest.
#[test]
fn re_grounding_a_revised_claim_clears_the_stale_finding() {
    let sb = Sandbox::new("stale-cleared");
    let memo = one_claim_memo(&sb);
    let m = memo.to_str().unwrap();

    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);
    sb.run(&["claim", "--revise", "C1", "--proposition", "alpha.rs defines exactly one function",
             "--why", "narrowing"]);
    sb.run(&["render", "--out", m]);
    let (code, _r, _e) = sb.run(&["check", m]);
    assert_eq!(code, 1, "stale first");

    // The old record stays — the log is append-only — so the finding
    // persists until the wording it graded is restored. Re-grounding adds
    // a current record beside it; the stale one is still named, which is
    // correct: it graded text this claim no longer carries.
    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports",
             "--note", "re-grounded against the narrowed wording"]);
    let (_c, report, err) = sb.run(&["check", m]);
    let combined = format!("{report}{err}");
    assert_eq!(
        combined.matches("[stale-evidence]").count(),
        1,
        "exactly the one superseded record is named, not the fresh one:\n{combined}"
    );
}

/// A paragraph must be able to gain a citation after the fact. Until it
/// could, `tetel review`'s own instruction — mint the missing claim and
/// cite it — was unreachable for existing prose: `--cites` parsed beside
/// `--revise` and was discarded without a word.
#[test]
fn a_revision_can_attach_citations_to_a_paragraph_that_had_none() {
    let sb = Sandbox::new("revise-cites");
    sb.write("a.rs", "fn a() {}\n");
    sb.run(&["look", "a.rs"]);
    sb.run(&["fact", "--note", "a.rs defines a()"]);
    sb.run(&["claim", "--proposition", "a.rs defines a()", "--cites", "F1"]);
    sb.run_stdin(&["prose"], "Written before its claim existed.");

    let (_c, before, _e) = sb.run(&["render"]);
    assert!(!before.contains("*cites: C1*"), "starts uncited:\n{before}");

    sb.run_stdin(
        &["prose", "--revise", "P1", "--why", "attaching the claim minted afterwards", "--cites", "C1"],
        "Written before its claim existed.",
    );
    let (_c, after, _e) = sb.run(&["render"]);
    assert!(after.contains("*cites: C1*"), "revision must attach the citation:\n{after}");
}

/// Omitting `--cites` on a revision leaves existing citations alone —
/// "absent" means unchanged, not cleared. A revision written before the
/// field existed carries no cite at all and must replay identically.
#[test]
fn a_revision_without_cites_leaves_existing_citations_untouched() {
    let sb = Sandbox::new("revise-keeps-cites");
    sb.write("a.rs", "fn a() {}\n");
    sb.run(&["look", "a.rs"]);
    sb.run(&["fact", "--note", "a.rs defines a()"]);
    sb.run(&["claim", "--proposition", "a.rs defines a()", "--cites", "F1"]);
    sb.run_stdin(&["prose", "--cites", "C1"], "Cited from the start.");

    sb.run_stdin(&["prose", "--revise", "P1", "--why", "reworded only"], "Reworded, same claim.");
    let (_c, out, _e) = sb.run(&["render"]);
    assert!(out.contains("Reworded, same claim."), "text changed:\n{out}");
    assert!(out.contains("*cites: C1*"), "citation must survive a text-only revision:\n{out}");
}

/// A buffer is not necessarily what you just looked at: a revision does
/// not clear it and a failed `look` does not add to it, so a mint can
/// fold an observation from an earlier line of enquiry. That happened,
/// silently, producing a fact whose note described two source files its
/// extent never covered. `fact` now says what it folded and how old it is.
#[test]
fn fact_reports_what_it_folded_so_a_stale_buffer_is_visible() {
    let sb = Sandbox::new("stale-buffer");
    sb.write("a.rs", "fn a() {}\n");

    sb.run(&["run", "echo", "leftover"]);
    sb.run(&["fact", "--note", "the first fact"]);
    sb.run(&["run", "echo", "second"]);
    // A revision does not clear the buffer — this is what leaves the
    // leftover in place.
    sb.run(&["fact", "--revise", "F1", "--why", "reworded", "--note", "the first fact, reworded"]);
    // A malformed range fails and adds nothing.
    let (code, _o, err) = sb.run(&["look", "a.rs", "--lines", "1-2"]);
    assert_ne!(code, 0, "a malformed range must fail: {err}");

    let (_c, out, _e) = sb.run(&["fact", "--note", "a note that would claim to be about a.rs"]);
    assert!(out.contains("folding:"), "mint must report what it folded:\n{out}");
    assert!(out.contains("echo second"), "the leftover must be named:\n{out}");
    assert!(!out.contains("a.rs"), "the file that was never opened must not appear:\n{out}");
}
