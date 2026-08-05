//! End-to-end tests for tetel's authoring commands (`look`, `run`,
//! `fact`, `claim`, `prose`, `render`, `query`). Each test gets a
//! private sandbox directory used both as the child process's working
//! directory (so `look`/`run` see files that exist only for this test)
//! and, via `TETEL_STATE_HOME`, as the root its session state lives
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
        std::fs::read_to_string(self.state_home().join("sessions/default/facts.jsonl")).unwrap_or_default()
    }

    fn claims_jsonl(&self) -> String {
        std::fs::read_to_string(self.state_home().join("sessions/default/claims.jsonl")).unwrap_or_default()
    }

    fn prose_jsonl(&self) -> String {
        std::fs::read_to_string(self.state_home().join("sessions/default/prose.jsonl")).unwrap_or_default()
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

    let (code, _out, err) = sb.run(&["claim", "--prop", "a claim resting on nothing"]);
    assert_ne!(code, 0, "a claim with no --from must be refused");
    assert!(err.contains("--from"), "stderr was:\n{err}");

    let (code, _out, err) = sb.run(&["claim", "--prop", "x", "--from", "F999"]);
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

    let (code, out, err) = sb.run(&["claim", "--prop", "lib.rs starts with `line 1`", "--from", "F1"]);
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
    sb.run(&["claim", "--prop", "the original proposition", "--from", "F1"]);

    let (code, _out, err) = sb.run(&["claim", "--revise", "C1", "--prop", "the revised proposition", "--why", "narrowed after review"]);
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
    sb.run(&["claim", "--prop", "a claim", "--from", "F1"]);

    let (_, out, _) = sb.run(&["query", "deps", "F1"]);
    assert!(out.contains("C1"), "output was:\n{out}");

    let (_, out, _) = sb.run(&["query", "deps", "C1"]);
    assert!(out.contains("F1"), "output was:\n{out}");
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
