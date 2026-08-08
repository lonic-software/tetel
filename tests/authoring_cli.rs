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

/// A command `run` spawns must never inherit the parent's stdin.
///
/// This has to spawn the binary by hand rather than use `Sandbox::run`,
/// and the reason is the whole point of the test: `Command::output()`
/// gives the child a **null** stdin already, so a `tee`/`cat` under
/// `sb.run` exits immediately whether or not the fix is present, and the
/// assertion would hold for a reason that has nothing to do with the bug.
/// The condition being reproduced is the MCP server's: stdin is a live
/// channel that never reaches EOF, so a child reading it blocks forever.
/// Hence a piped stdin that is deliberately kept open and never written
/// to — dropping the handle would close the pipe and let `cat` see EOF,
/// which is the same vacuity by a slower route.
#[test]
fn run_never_hands_a_child_the_parents_stdin() {
    let sb = Sandbox::new("run-stdin-null");
    let mut child = sb
        .command(&["run", "cat"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn tetel binary");

    // Held, not dropped: this pipe stands in for the JSON-RPC channel.
    let _held_open = child.stdin.take().expect("stdin was piped");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait failed") {
            break status;
        }
        if std::time::Instant::now() > deadline {
            child.kill().ok();
            child.wait().ok();
            panic!(
                "`tetel run cat` did not exit within 10s: the child inherited the parent's \
                 stdin and blocked on it. On the MCP surface that stdin is the request \
                 channel, and because the server answers serially this stalls every later call."
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };

    assert_eq!(status.code(), Some(0), "a command reading a null stdin should succeed with no output");

    // The observation is still recorded: a command that read nothing is a
    // bounded negative, and it leaves a trace like every other `run`. Closing
    // stdin must not turn the call into a no-op.
    let (code, _out, _err) = sb.run(&["fact", "--note", "ran a command that reads stdin"]);
    assert_eq!(code, 0, "a run observation must still be fact-worthy");
    assert!(
        sb.facts_jsonl().contains("proc: cat"),
        "the run must appear in the minted extent; facts.jsonl was:\n{}",
        sb.facts_jsonl()
    );
}

/// A tree holding source, one rendered memo with its snapshot and ledger,
/// and — the trap — an unrelated file sharing the memo's basename with no
/// snapshot beside it, so it is not a memo and must survive.
fn seed_a_tree_with_tetel_output(sb: &Sandbox) {
    sb.write("src/thing.rs", "fn f() { CENSUS_TARGET(); }\n");
    sb.write("docs/memo.md", "prose about CENSUS_TARGET\n");
    sb.write("docs/memo.md.tetel/facts.jsonl", "{\"note\":\"CENSUS_TARGET\"}\n");
    sb.write("docs/memo.md.evidence.jsonl", "{\"subject\":\"CENSUS_TARGET\"}\n");
    sb.write("other/memo.md", "unrelated file, same basename, CENSUS_TARGET\n");
}

#[test]
fn a_directory_search_skips_tetel_output_without_hiding_source_that_shares_a_memo_name() {
    let sb = Sandbox::new("grep-excludes-tetel");
    seed_a_tree_with_tetel_output(&sb);

    let (code, out, _err) = sb.run(&["look", "--grep", "CENSUS_TARGET", "."]);
    assert_eq!(code, 0);

    // Assert over the *match lines* only. The disclosure line names the
    // patterns it applied, so it contains `*.evidence.jsonl` verbatim and
    // a naive `!out.contains(...)` would fail against a correct search —
    // it did, when this test was first written.
    let matched: Vec<&str> = out.lines().filter(|l| l.starts_with("./")).collect();
    let hit = |needle: &str| matched.iter().any(|l| l.contains(needle));

    assert!(hit("src/thing.rs"), "source must still be searched; matches were:\n{matched:#?}");

    // All three tetel artifacts are gone: the snapshot directory, the
    // ledger, and the rendered memo identified by its sibling snapshot.
    assert!(!hit("memo.md.tetel"), "snapshot dir must be skipped; matches were:\n{matched:#?}");
    assert!(!hit(".evidence.jsonl"), "evidence ledger must be skipped; matches were:\n{matched:#?}");
    assert!(!hit("docs/memo.md"), "the rendered memo must be skipped; matches were:\n{matched:#?}");

    // The trap. `other/memo.md` has the memo's basename and no snapshot,
    // so it is ordinary prose. Excluding it would hide source from a
    // census under the banner of hiding tetel's output — which is exactly
    // what a suffix-stripped bare basename does.
    assert!(
        hit("other/memo.md"),
        "a file sharing a memo's basename but having no snapshot is NOT a memo and must be \
         searched; matches were:\n{matched:#?}"
    );
}

#[test]
fn a_search_the_caller_pointed_at_tetel_output_is_not_filtered() {
    let sb = Sandbox::new("grep-explicit-tetel");
    seed_a_tree_with_tetel_output(&sb);

    // A named file: the rendered memo itself.
    let (code, out, _err) = sb.run(&["look", "--grep", "CENSUS_TARGET", "docs/memo.md"]);
    assert_eq!(code, 0);
    assert!(out.contains("prose about"), "a named memo must still be read; output was:\n{out}");

    // A named evidence ledger, and this is the case that actually pins the
    // file-root branch. A named *memo* survives even without that branch,
    // because a file root yields no memo patterns and none of the static
    // ones match it — so naming a memo cannot tell the two implementations
    // apart. A ledger can: `--exclude=*.evidence.jsonl` matches it, and
    // grep skips a file named on the command line when `--exclude` matches
    // it. Without the branch this returns nothing. Found by mutation; the
    // first version of this test named only the memo and survived.
    let (code, out, _err) = sb.run(&["look", "--grep", "CENSUS_TARGET", "docs/memo.md.evidence.jsonl"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("subject"),
        "a named evidence ledger must still be read; output was:\n{out}"
    );

    // A named snapshot directory. This branch is forced rather than
    // chosen: grep suppresses a directory named as the recursion root when
    // `--exclude-dir` matches it, so passing the flags here would return
    // nothing at all and make a shipped record unreadable.
    let (code, out, _err) = sb.run(&["look", "--grep", "CENSUS_TARGET", "docs/memo.md.tetel"]);
    assert_eq!(code, 0);
    assert!(out.contains("facts.jsonl"), "a named snapshot must still be searched; output was:\n{out}");
}

#[test]
fn the_exclusion_set_is_recorded_in_the_search_label_not_only_printed() {
    let sb = Sandbox::new("grep-exclusion-recorded");
    seed_a_tree_with_tetel_output(&sb);
    sb.run(&["look", "--grep", "CENSUS_TARGET", "."]);
    let (code, _out, _err) = sb.run(&["fact", "--note", "censused CENSUS_TARGET"]);
    assert_eq!(code, 0);

    // The label is the record; the printed line is only convenience. It
    // has to name the memos rather than count them, or two searches over
    // different memo sets of one size would pin identically.
    let facts = sb.facts_jsonl();
    assert!(facts.contains("skipped tetel's own output"), "facts.jsonl was:\n{facts}");
    assert!(facts.contains("memo.md"), "the excluded memo must be named; facts.jsonl was:\n{facts}");

    // And a search that was *not* filtered says so, because silence is
    // ambiguous between "nothing hidden" and "nobody said".
    let sb2 = Sandbox::new("grep-exclusion-recorded-none");
    seed_a_tree_with_tetel_output(&sb2);
    sb2.run(&["look", "--grep", "CENSUS_TARGET", "docs/memo.md"]);
    sb2.run(&["fact", "--note", "read the memo directly"]);
    assert!(sb2.facts_jsonl().contains("no exclusions"), "facts.jsonl was:\n{}", sb2.facts_jsonl());
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

/// A snapshot that exists but carries no identity must say so, and must
/// not be reported as a missing snapshot.
///
/// The two were one case and both printed "no snapshot beside this
/// memo". A reader who went looking for the missing directory found it
/// present with six of its seven files — the message named a cause that
/// was not the cause, and cost real time. They also call for different
/// actions: a missing snapshot means the memo was never rendered by
/// `render --out`, while a snapshot without an identity was rendered by
/// a build that did not ship one and cannot be repaired now, since
/// minting an identity later would date the pass wrongly.
#[test]
fn a_snapshot_without_an_identity_is_not_reported_as_a_missing_snapshot() {
    let sb = Sandbox::new("snapshot-no-identity");
    let memo = memo_authored_by(&sb);
    let m = memo.to_str().unwrap();
    sb.run(&["--workspace", "grounder", "look", "alpha.rs"]);
    sb.run(&["--workspace", "grounder", "fact", "--note", "read independently"]);
    sb.run(&["--workspace", "grounder", "record", m,
             "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);

    // Simulate a memo rendered by a build that shipped no identity.
    let snapshot_identity = sb.dir.join("memo.md.tetel/identity.json");
    assert!(snapshot_identity.is_file(), "render must ship an identity in the first place");
    std::fs::remove_file(&snapshot_identity).unwrap();

    let (_c, report, err) = sb.run(&["check", m]);
    let combined = format!("{report}{err}");
    assert!(
        combined.contains("carries no identity"),
        "the real condition must be named:\n{combined}"
    );
    assert!(
        !combined.contains("no snapshot beside this memo"),
        "the snapshot is present — saying otherwise sends the reader after a phantom:\n{combined}"
    );
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
    assert_eq!(code, 1, "a claim out of proof must fail the machine check:\n{combined}");
    assert!(combined.contains("[out-of-proof]"), "got:\n{combined}");
    assert!(combined.contains("graded different text"), "got:\n{combined}");
    // The memo matches its snapshot, so drift is not what caught this.
    assert!(!combined.contains("[provenance-drift]"), "drift must not be the catcher:\n{combined}");
}

/// Re-grounding must actually clear the failure, because that is the
/// remedy `check` prints. The first version of this check failed on any
/// stale record, so re-grounding added a current record and left the red
/// standing — a red nobody could clear, since the evidence log is
/// append-only and has no supersede. The superseded record is still
/// shown, as history, in the human-owed half.
#[test]
fn re_grounding_a_revised_claim_clears_the_stale_failure() {
    let sb = Sandbox::new("stale-cleared");
    let memo = one_claim_memo(&sb);
    let m = memo.to_str().unwrap();

    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);
    sb.run(&["claim", "--revise", "C1", "--proposition", "alpha.rs defines exactly one function",
             "--why", "narrowing"]);
    sb.run(&["render", "--out", m]);
    let (code, report, err) = sb.run(&["check", m]);
    assert_eq!(code, 1, "stale first:\n{report}{err}");

    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports",
             "--note", "re-grounded against the narrowed wording"]);
    let (code, report, err) = sb.run(&["check", m]);
    let combined = format!("{report}{err}");
    assert_ne!(code, 1, "re-grounding must clear the failure:\n{combined}");
    assert!(!combined.contains("[out-of-proof]"), "no longer a machine failure:\n{combined}");
    // Still visible, as history rather than alarm.
    assert!(combined.contains("superseded evidence"), "the earlier record stays visible:\n{combined}");
}

/// A claim whose only evidence graded an earlier wording is still a
/// machine failure: nothing grades what it says today.
#[test]
fn a_claim_with_only_stale_evidence_still_fails() {
    let sb = Sandbox::new("stale-only");
    let memo = one_claim_memo(&sb);
    let m = memo.to_str().unwrap();

    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);
    sb.run(&["claim", "--revise", "C1", "--proposition", "alpha.rs does NOT define alpha()",
             "--why", "reversing"]);
    sb.run(&["render", "--out", m]);

    let (code, report, err) = sb.run(&["check", m]);
    let combined = format!("{report}{err}");
    assert_eq!(code, 1, "nothing grades the current wording:\n{combined}");
    assert!(combined.contains("[out-of-proof]"), "got:\n{combined}");
    assert!(combined.contains("Out of proof: nothing grades what this claim says today"), "got:\n{combined}");
}
/// The second instance of the same defect: `out-of-proof` was made
/// digest-aware and `verdict-disagreement` was not, so a contradiction
/// against text that no longer exists failed the check forever.
///
/// This is the loop the whole apparatus exists to produce — a pass
/// refutes a claim, the author fixes the wording, a later pass grounds
/// the new wording unanimously — and it ended in a permanent red. The
/// only escape was withdrawing the claim and re-issuing it under a fresh
/// id, which erases the refutation from the rendered ledger: the one
/// trail this loop exists to preserve.
#[test]
fn a_contradiction_against_superseded_text_is_cleared_by_re_grounding() {
    let sb = Sandbox::new("verdict-superseded");
    let memo = one_claim_memo(&sb);
    let m = memo.to_str().unwrap();

    // Two passes disagree about the original wording.
    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);
    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "refutes",
             "--note", "alpha.rs also defines beta()"]);
    let (code, report, err) = sb.run(&["check", m]);
    assert_eq!(code, 1, "the live contradiction must fail:\n{report}{err}");

    // The author resolves it by rewriting the claim, then re-grounds.
    sb.run(&["claim", "--revise", "C1", "--proposition", "alpha.rs defines at least one function",
             "--why", "the refutation was right; widening to what both passes agree on"]);
    sb.run(&["render", "--out", m]);
    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports",
             "--note", "re-grounded against the widened wording"]);

    let (code, report, err) = sb.run(&["check", m]);
    let combined = format!("{report}{err}");
    assert_ne!(code, 1, "the current wording is unanimously supported:\n{combined}");
    assert!(!combined.contains("[verdict-disagreement]"), "got:\n{combined}");
    assert!(!combined.contains("[out-of-proof]"), "got:\n{combined}");
    // Erasing the disagreement is not the remedy — it must still print.
    assert!(combined.contains("superseded evidence"), "the trail must survive:\n{combined}");
    assert!(combined.contains("alpha.rs also defines beta()"), "the refutation's note must survive:\n{combined}");
}

/// The converse: a contradiction among records that all grade the
/// *current* wording is still P and not-P, and still fails. Digest
/// awareness must not become a way to launder a live disagreement.
#[test]
fn a_contradiction_survives_a_revision_that_does_not_resolve_it() {
    let sb = Sandbox::new("verdict-still-live");
    let memo = one_claim_memo(&sb);
    let m = memo.to_str().unwrap();

    sb.run(&["claim", "--revise", "C1", "--proposition", "alpha.rs defines exactly one function",
             "--why", "narrowing"]);
    sb.run(&["render", "--out", m]);
    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports"]);
    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "refutes",
             "--note", "there is a second function"]);

    let (code, report, err) = sb.run(&["check", m]);
    let combined = format!("{report}{err}");
    assert_eq!(code, 1, "both records grade the current text:\n{combined}");
    assert!(combined.contains("[verdict-disagreement]"), "got:\n{combined}");
}

/// A qualification is human-owed, not a machine failure — but it is
/// still *about* a wording. Once the author has revised the text the
/// qualification was written against and re-grounded, the claim must
/// stop reading as qualified, or the human partition accumulates the
/// same undischargeable residue the machine one just shed. The record
/// itself stays, under superseded evidence.
#[test]
fn a_qualification_against_superseded_text_stops_reading_as_qualified() {
    let sb = Sandbox::new("verdict-qualified-superseded");
    let memo = one_claim_memo(&sb);
    let m = memo.to_str().unwrap();

    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "qualifies",
             "--note", "holds only on the single-threaded path"]);
    let (_c, report, err) = sb.run(&["check", m]);
    assert!(format!("{report}{err}").contains("C1: QUALIFIED"), "qualified first:\n{report}{err}");

    sb.run(&["claim", "--revise", "C1", "--proposition", "alpha.rs defines alpha() on the single-threaded path",
             "--why", "folding the qualification into the claim"]);
    sb.run(&["render", "--out", m]);
    sb.run(&["record", m, "--from-fact", "F1", "--claim", "C1", "--verdict", "supports",
             "--note", "the condition is now stated in the claim"]);

    let (code, report, err) = sb.run(&["check", m]);
    let combined = format!("{report}{err}");
    assert_ne!(code, 1, "nothing here is a machine failure:\n{combined}");
    assert!(!combined.contains("C1: QUALIFIED"), "the qualification was discharged:\n{combined}");
    assert!(combined.contains("single-threaded path"), "but the record stays visible:\n{combined}");
}

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

    // `a.rs` must not appear as something this mint *folded* — the
    // original assertion here was a blunt "must not appear anywhere",
    // which was right when the folding line was the only output and is
    // wrong now: the file appears deliberately, one section down, as
    // something the author *tried and failed* to open. Splitting the two
    // sections is what makes the assertion say what it means.
    let folded_section = out.split("refused since").next().unwrap_or(&out);
    assert!(
        !folded_section.contains("a.rs"),
        "the file that was never opened must not appear as folded:\n{out}"
    );
}

/// The other half of the same mint: what the author *could not* do in the
/// window that produced this fact.
///
/// `folding:` says what was taken. It cannot say why what the author
/// expected is missing, and that gap is the incident — two `look` calls
/// refused for a malformed range, each leaving the buffer untouched, and
/// a mint that folded a leftover from an earlier line of enquiry. Both
/// halves are needed, and the refusal half is the one that names the file.
#[test]
fn a_mint_names_the_refusals_that_explain_what_is_missing() {
    let sb = Sandbox::new("mint-refusals");
    sb.write("a.rs", "fn a() {}\n");
    sb.write("b.rs", "fn b() {}\n");

    sb.run(&["run", "echo", "leftover-from-earlier"]);
    let (code, _o, _e) = sb.run(&["look", "a.rs", "--lines", "1-2"]);
    assert_ne!(code, 0, "a malformed range must fail");
    let (code, _o, _e) = sb.run(&["look", "b.rs", "--lines", "3-4"]);
    assert_ne!(code, 0, "a malformed range must fail");

    let (_c, out, _e) = sb.run(&["fact", "--note", "a.rs and b.rs both define one function"]);
    assert!(out.contains("refused since the previous fact"), "got:\n{out}");
    // The whole point: the replay names the files the author believed
    // they had captured. A bare "invalid --lines" would say a look failed
    // without saying which file is missing.
    assert!(out.contains("a.rs"), "the first refused file must be named:\n{out}");
    assert!(out.contains("b.rs"), "the second refused file must be named:\n{out}");

    // The window boundary is deliberately not asserted here. Timestamps
    // are whole seconds and a test runs inside one, so an end-to-end run
    // cannot distinguish "the window works" from "everything happened in
    // the same second" — and the design's `>=` boundary means a
    // same-second refusal is shown twice on purpose. That logic is a pure
    // function over timestamps and is tested as one, in
    // `workspace::tests`, where the seconds can be set rather than raced.
}

/// The whole incident, end to end, through `check`.
///
/// The mint-time line works only if the author reads it in the moment.
/// This is the other half: the same signal recovered at grading time,
/// when nobody is relying on the author's attention. Before this, a memo
/// built on a fact whose note reached past a leftover extent rendered and
/// checked clean with exit 0, and nothing anywhere said the two `look`
/// calls the author believed they had made were refused.
#[test]
fn check_lists_the_refusals_recorded_in_a_facts_own_mint_window() {
    let sb = Sandbox::new("check-mint-window");
    sb.write("a.rs", "fn a() {}\n");

    sb.run(&["run", "echo", "leftover-from-earlier"]);
    let (code, _o, _e) = sb.run(&["look", "a.rs", "--lines", "1-5"]);
    assert_ne!(code, 0, "a malformed range must fail");

    sb.run(&["fact", "--note", "a.rs defines exactly one function"]);
    sb.run(&["claim", "--proposition", "a.rs defines exactly one function", "--cites", "F1"]);
    sb.run_stdin(&["prose", "--cites", "C1"], "It defines one function.");
    let memo = sb.dir.join("memo.md");
    sb.run(&["render", "--out", memo.to_str().unwrap()]);

    let (code, report, err) = sb.run(&["check", memo.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    // Human-owed: never a failure. A mint after a refusal is often right.
    assert_eq!(code, 0, "this must not redden the machine partition:\n{combined}");
    assert!(combined.contains("human-owed:"), "got:\n{combined}");
    assert!(
        combined.contains("refusal(s) recorded"),
        "check must recover the mint window from the shipped snapshot:\n{combined}"
    );
    assert!(combined.contains("a.rs"), "the refused file must be named:\n{combined}");
    // The category must be advertised in the preamble too — a listed
    // category with a silent instance is the failure this project has
    // already recorded once.
    assert!(
        combined.contains("refusals recorded in a fact's own mint window"),
        "the preamble must name the category it prints:\n{combined}"
    );
}

/// A memo whose facts were minted with nothing refused says nothing
/// about mint windows — the same reason the mint-time line is silent.
#[test]
fn check_says_nothing_about_mint_windows_when_nothing_was_refused() {
    let sb = Sandbox::new("check-no-windows");
    let memo = one_claim_memo(&sb);
    let (_c, report, err) = sb.run(&["check", memo.to_str().unwrap()]);
    let combined = format!("{report}{err}");
    assert!(!combined.contains("refusal(s) recorded"), "got:\n{combined}");
}

/// A mint with nothing refused in its window says nothing about
/// refusals. The line is a report of something that happened, not a
/// standing section that prints "none" — an empty heading on every mint
/// is noise, and noise is what makes a real one invisible.
#[test]
fn a_clean_mint_says_nothing_about_refusals() {
    let sb = Sandbox::new("mint-no-refusals");
    sb.write("a.rs", "fn a() {}\n");
    sb.run(&["look", "a.rs"]);
    let (_c, out, _e) = sb.run(&["fact", "--note", "a.rs defines a()"]);
    assert!(out.contains("folding:"), "got:\n{out}");
    assert!(!out.contains("refused since"), "nothing was refused:\n{out}");
}

/// Document order was authoring order, so writing prose as discoveries
/// happen — what the brief asks for — produced a document in discovery
/// order, and the only route to a well-ordered one was deferring all
/// prose to the end: exactly the pattern the brief exists to prevent.
/// The tool's shape rewarded the anti-pattern, and no brief text can
/// outweigh that.
#[test]
fn prose_can_be_inserted_before_an_existing_block() {
    let sb = Sandbox::new("prose-before");
    sb.run_stdin(&["prose"], "Discovered first, but belongs second.");
    sb.run_stdin(&["prose", "--before", "P1"], "Discovered second, but belongs first.");

    let (_c, out, _e) = sb.run(&["render"]);
    let first = out.find("belongs first").expect("both blocks render");
    let second = out.find("belongs second").expect("both blocks render");
    assert!(first < second, "insertion must decide order, not authoring:\n{out}");
}

/// Headings insert too — a section discovered late still opens where it
/// belongs.
#[test]
fn a_heading_can_be_inserted_before_an_existing_block() {
    let sb = Sandbox::new("heading-before");
    sb.run_stdin(&["prose"], "A paragraph written before its own section heading.");
    sb.run_stdin(&["prose", "--heading", "-", "--level", "2", "--before", "P1"], "The section");

    let (_c, out, _e) = sb.run(&["render"]);
    assert!(
        out.find("## The section").unwrap() < out.find("A paragraph written").unwrap(),
        "the heading must precede its paragraph:\n{out}"
    );
}

/// An anchor that does not exist is refused, not silently appended —
/// otherwise a typo reorders the document without saying so.
#[test]
fn inserting_before_an_unknown_block_is_refused() {
    let sb = Sandbox::new("before-unknown");
    sb.run_stdin(&["prose"], "The only block.");
    let (code, _o, err) = sb.run_stdin(&["prose", "--before", "P99"], "Should not land.");
    assert_eq!(code, 1, "an unknown anchor must be refused");
    assert!(err.contains("no such prose block to insert before"), "stderr:\n{err}");

    let (_c, out, _e) = sb.run(&["render"]);
    assert!(!out.contains("Should not land"), "nothing must be written:\n{out}");
}

/// Logs written before insertion existed carry no anchor and must replay
/// in exactly the order they always did.
#[test]
fn a_log_without_anchors_still_replays_in_append_order() {
    let sb = Sandbox::new("before-backcompat");
    for (i, t) in ["first", "second", "third"].iter().enumerate() {
        sb.run_stdin(&["prose"], &format!("Block {} is {t}.", i + 1));
    }
    let (_c, out, _e) = sb.run(&["render"]);
    let (a, b, c) = (
        out.find("is first").unwrap(),
        out.find("is second").unwrap(),
        out.find("is third").unwrap(),
    );
    assert!(a < b && b < c, "append order must be preserved:\n{out}");
}

// --- TET-5: the marker must describe the tree that was read ------------

/// Make `dir` a git repository with one commit, so `worldstate` has
/// something to resolve. Identity is passed per-command rather than
/// configured, so the test never depends on the machine's git config.
fn init_repo(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git must be on PATH for this test")
            .status
            .success();
        assert!(ok, "git {args:?} failed in {}", dir.display());
    };
    git(&["init", "-q"]);
    std::fs::write(dir.join("f.txt"), "original\n").unwrap();
    git(&["add", "f.txt"]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"]);
}

/// Every `(world_root, world_state)` pair in the pending buffer, in order.
fn pending_markers(sb: &Sandbox) -> Vec<(String, String)> {
    let raw = std::fs::read_to_string(sb.state_home().join("workspaces/default/pending.json"))
        .expect("pending buffer must exist");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["world_root"].as_str().unwrap_or_default().to_string(),
                e["world_state"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[test]
fn a_marker_tracks_the_tree_that_was_read_not_the_one_the_process_stood_in() {
    // The defect this replaced: `worldstate` ran `git` in the process's
    // own working directory and stamped that answer onto every
    // observation. Measured directly, the marker moved when a repository
    // nobody had read changed and stayed put when the one being read did.
    // A unit test could not see this — the old one asserted only that a
    // marker came back — so it is asserted end to end, through the binary.
    let sb = Sandbox::new("worldstate-follows-the-read");
    let a = sb.dir.join("repoA");
    let b = sb.dir.join("repoB");
    init_repo(&a);
    init_repo(&b);

    // The sandbox dir itself is the process's cwd; put a repo under it and
    // run from there so "where we stand" and "what we read" differ.
    let run_in_a = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tetel"));
        cmd.args(args);
        cmd.current_dir(&a);
        cmd.env("TETEL_STATE_HOME", sb.state_home());
        let out = cmd.output().expect("failed to run tetel");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    };

    let b_file = b.join("f.txt");
    run_in_a(&["look", b_file.to_str().unwrap()]);

    // Dirty the repository we are standing in but did not read.
    std::fs::write(a.join("f.txt"), "A is now dirty\n").unwrap();
    run_in_a(&["look", b_file.to_str().unwrap()]);

    // Now dirty the one we actually read.
    std::fs::write(&b_file, "B is now dirty\n").unwrap();
    run_in_a(&["look", b_file.to_str().unwrap()]);

    let m = pending_markers(&sb);
    assert_eq!(m.len(), 3, "three observations expected: {m:?}");

    // Premise: git resolved at all. Without this the two assertions below
    // would both hold vacuously on a machine with no usable git.
    assert!(
        m.iter().all(|(root, _)| root.ends_with("repoB")),
        "every marker must name the tree that was read: {m:?}"
    );

    assert_eq!(
        m[0].1, m[1].1,
        "an unread repository going dirty must not move the marker: {m:?}"
    );
    assert_ne!(
        m[1].1, m[2].1,
        "the repository that was read going dirty must move the marker: {m:?}"
    );
}

#[test]
fn a_run_marker_names_the_tree_the_command_ran_in() {
    // `run` is the deliberate exception: a command names no path, and it
    // genuinely executed in this process's working directory.
    let sb = Sandbox::new("worldstate-run-uses-cwd");
    let a = sb.dir.join("repoA");
    init_repo(&a);

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tetel"));
    cmd.args(["run", "--", "echo", "hello"]);
    cmd.current_dir(&a);
    cmd.env("TETEL_STATE_HOME", sb.state_home());
    let out = cmd.output().expect("failed to run tetel");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let m = pending_markers(&sb);
    assert_eq!(m.len(), 1);
    assert!(m[0].0.ends_with("repoA"), "run must name the tree it ran in: {m:?}");
}

#[test]
fn check_reports_which_facts_saw_which_tree_state() {
    let sb = Sandbox::new("worldstate-check-divergence");
    let a = sb.dir.join("repoA");
    init_repo(&a);

    let run_in_a = |args: &[&str]| -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tetel"));
        cmd.args(args);
        cmd.current_dir(&a);
        cmd.env("TETEL_STATE_HOME", sb.state_home());
        let out = cmd.output().expect("failed to run tetel");
        assert!(
            out.status.code() == Some(0) || out.status.code() == Some(1),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let f = a.join("f.txt");
    let fs_ = f.to_str().unwrap();

    // F1 against the committed tree.
    run_in_a(&["look", fs_]);
    run_in_a(&["fact", "--note", "the file says original"]);

    // F2 against a modified one — the ticket's F3/F8 shape: two facts
    // about one file, taken against opposite states of one tree.
    std::fs::write(&f, "replaced\n").unwrap();
    run_in_a(&["look", fs_]);
    run_in_a(&["fact", "--note", "the file says replaced"]);

    run_in_a(&["claim", "--proposition", "the file has a value", "--cites", "F1"]);
    run_in_a(&["prose", "--text", "See [C1]."]);

    let memo = sb.dir.join("memo.md");
    run_in_a(&["render", "--out", memo.to_str().unwrap()]);
    let report = run_in_a(&["check", memo.to_str().unwrap()]);

    assert!(
        report.contains("different working-tree states"),
        "check must report the divergence: {report}"
    );
    assert!(report.contains("F1"), "and which facts saw which state: {report}");
    assert!(report.contains("F2"), "and which facts saw which state: {report}");
    // Never a failure — this is a record, not a defect.
    assert!(
        report.contains("machine-checked: clean") || !report.contains("[world"),
        "tree divergence must never enter the machine-checked partition: {report}"
    );
}

#[test]
fn a_witnessed_record_carries_the_tree_it_graded_and_an_ingested_one_cannot() {
    // The second run behind TET-5: a grounding pass read the live working
    // tree while the memo pinned an older commit, and nothing in any
    // artifact said so. This is the field that makes that answerable.
    let sb = Sandbox::new("witnessed-world");
    let repo = sb.dir.join("repo");
    init_repo(&repo);

    let in_repo = |args: &[&str]| -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tetel"));
        cmd.args(args);
        cmd.current_dir(&repo);
        cmd.env("TETEL_STATE_HOME", sb.state_home());
        let out = cmd.output().expect("failed to run tetel");
        (
            out.status.code().unwrap(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    in_repo(&["--workspace", "author", "look", "f.txt"]);
    in_repo(&["--workspace", "author", "fact", "--note", "f.txt says original"]);
    in_repo(&["--workspace", "author", "claim", "--proposition", "f.txt says original", "--cites", "F1"]);
    in_repo(&["--workspace", "author", "prose", "--text", "See [C1]."]);
    let memo = sb.dir.join("memo.md");
    in_repo(&["--workspace", "author", "render", "--out", memo.to_str().unwrap()]);

    in_repo(&["--workspace", "grounder", "look", "f.txt"]);
    in_repo(&["--workspace", "grounder", "fact", "--note", "read independently"]);
    let (code, _, err) = in_repo(&[
        "--workspace", "grounder", "record", memo.to_str().unwrap(),
        "--from-fact", "F1", "--claim", "C1", "--verdict", "supports",
    ]);
    assert_eq!(code, 0, "stderr:\n{err}");

    let raw = std::fs::read_to_string(sb.dir.join("memo.md.evidence.jsonl")).unwrap();
    let witnessed: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
    let world = witnessed["predicate"]["world"].as_array().expect("witnessed records carry `world`");
    assert_eq!(world.len(), 1, "one repository in one state, said once: {world:?}");
    assert!(
        world[0]["root"].as_str().unwrap().ends_with("repo"),
        "the marker must name the tree that was read: {world:?}"
    );
    assert!(world[0]["state"].as_str().unwrap().starts_with("git:"), "{world:?}");

    // The ingested path has no way to supply one, and says so by being
    // empty rather than by carrying whatever a caller typed.
    let input = serde_json::json!({
        "claim": "C1", "pass": "someone-else", "verdict": "supports",
        "reported_kind": "reading", "source": "notes.md",
        "extent": ["f.txt"],
        "world": [{"root": "/invented", "state": "git:deadbeef"}],
    });
    let (code, _, err) = sb.run_stdin(
        &["record", memo.to_str().unwrap()],
        &serde_json::to_string(&input).unwrap(),
    );
    assert_eq!(code, 0, "stderr:\n{err}");
    let raw = std::fs::read_to_string(sb.dir.join("memo.md.evidence.jsonl")).unwrap();
    let ingested: serde_json::Value =
        serde_json::from_str(raw.lines().last().unwrap()).unwrap();
    assert_eq!(ingested["predicateType"], tetel::evidence::INGESTED_PREDICATE_TYPE);
    assert_eq!(
        ingested["predicate"]["world"].as_array().map(Vec::len),
        Some(0),
        "a typed marker must not survive into the record: {ingested}"
    );
}

#[test]
fn a_grep_of_a_single_file_is_keyed_by_that_file_not_by_a_line_number() {
    // grep prints a filename only when given more than one file to search,
    // so a single-file search returned `<line>:<match>` and the line
    // number was read as the filename. The consequences were silent and
    // two: the observation overlapped nothing, and a bare integer has no
    // parent directory, so its world-tree marker fell back to this
    // process's working directory — the exact defect TET-5 removed.
    let sb = Sandbox::new("grep-single-file-key");
    let repo = sb.dir.join("repo");
    init_repo(&repo);
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::write(repo.join("sub/target.txt"), "alpha\nbeta\ngamma\n").unwrap();

    // Stand in an unrelated repository, so a cwd-derived marker is
    // distinguishable from a correctly resolved one.
    let elsewhere = sb.dir.join("elsewhere");
    init_repo(&elsewhere);

    let target = repo.join("sub/target.txt");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tetel"));
    cmd.args(["look", target.to_str().unwrap(), "--grep", "beta"]);
    cmd.current_dir(&elsewhere);
    cmd.env("TETEL_STATE_HOME", sb.state_home());
    let out = cmd.output().expect("failed to run tetel");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let raw = std::fs::read_to_string(sb.state_home().join("workspaces/default/pending.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let all = v.as_array().unwrap();
    // The per-file hits, which is what this test is about. TET-28 added a
    // whole-search record alongside them, keyed on the search root rather
    // than on anything matched; `a_grep_that_matched_records_where_it_was_rooted`
    // owns that one.
    let entries: Vec<_> = all.iter().filter(|e| e["kind"] == "GrepMatch").collect();
    assert_eq!(entries.len(), 1, "one matching file, one per-file observation: {all:?}");

    let key = entries[0]["key"].as_str().unwrap();
    assert!(
        key.ends_with("target.txt"),
        "the key must be the file, not the line it matched on: {key}"
    );
    let root = entries[0]["world_root"].as_str().unwrap();
    assert!(
        root.ends_with("repo"),
        "the marker must name the searched file's tree, not the directory we stood in: {root}"
    );
}

#[test]
fn a_single_file_grep_overlaps_a_plain_read_of_the_same_file() {
    // `pending.rs` documents that `look`, `look --lines` and `look --grep`
    // all key on the resolved path, so "any two observations of the same
    // file overlap regardless of what range or pattern produced them".
    // That contract was false for a *single-file* grep, whose key was the
    // line number it matched on — so such an observation overlapped
    // nothing at all, silently. The behaviour is pinned here rather than
    // left resting on the missing-`-H` fix being remembered.
    let sb = Sandbox::new("overlap-single-file-grep");
    sb.write("src/lib.rs", "fn alpha() {}\nfn beta() {}\n");
    sb.write("src/other.rs", "fn gamma() {}\n");

    // F1: a grep of one specific file — the shape that was broken.
    let (code, _, err) = sb.run(&["look", "src/lib.rs", "--grep", "beta"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    let (code, _, err) = sb.run(&["fact", "--note", "lib.rs defines beta"]);
    assert_eq!(code, 0, "stderr:\n{err}");

    // F2: a plain read of the same file.
    let (code, _, err) = sb.run(&["look", "src/lib.rs"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    let (code, _, err) = sb.run(&["fact", "--note", "the whole of lib.rs"]);
    assert_eq!(code, 0, "stderr:\n{err}");

    // F3: an unrelated file, which must never be reported.
    let (code, _, err) = sb.run(&["look", "src/other.rs"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    let (code, _, err) = sb.run(&["fact", "--note", "an unrelated file"]);
    assert_eq!(code, 0, "stderr:\n{err}");

    let (code, out, err) = sb.run(&["claim", "--proposition", "lib.rs defines beta", "--cites", "F1"]);
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(
        out.contains("F2"),
        "a plain read of the file a single-file grep searched must overlap it:\n{out}"
    );
    assert!(!out.contains("F3"), "an unrelated file's fact must never overlap:\n{out}");
}

// --- the pin: nothing asserted anything about it until this sweep -------

/// Every `pin` in this workspace's log, in mint order.
fn pins(sb: &Sandbox, workspace: &str) -> Vec<String> {
    let p = sb.state_home().join("workspaces").join(workspace).join("facts.jsonl");
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["event"] == "Create")
        .map(|v| v["pin"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn two_facts_from_identical_observations_pin_identically() {
    // The floor. Without this, "the pin changed" carries no information,
    // because it might change for no reason at all.
    let sb = Sandbox::new("pin-deterministic");
    let repo = sb.dir.join("repo");
    init_repo(&repo);

    let run_in = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tetel"));
        c.args(args).current_dir(&repo).env("TETEL_STATE_HOME", sb.state_home());
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    };

    run_in(&["look", "f.txt"]);
    run_in(&["fact", "--note", "first read"]);
    run_in(&["look", "f.txt"]);
    run_in(&["fact", "--note", "an identical read, different note"]);

    let p = pins(&sb, "default");
    assert_eq!(p.len(), 2);
    assert_eq!(
        p[0], p[1],
        "same file, same tree, same output — the note is not part of the pin: {p:?}"
    );
}

#[test]
fn a_fact_taken_against_a_changed_tree_pins_differently() {
    // The property TET-5 added and nothing verified: the working-tree
    // marker is folded into the pin, so two facts whose *observed file* is
    // byte-identical still pin apart when the tree around them moved.
    //
    // Written as a falsifier for that specific fold: the label, the output
    // and the file are all identical between the two mints, so the marker
    // is the only input that differs. Remove it from the hash and this is
    // the test that fails.
    let sb = Sandbox::new("pin-tracks-tree");
    let repo = sb.dir.join("repo");
    init_repo(&repo);

    let run_in = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tetel"));
        c.args(args).current_dir(&repo).env("TETEL_STATE_HOME", sb.state_home());
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    };

    run_in(&["look", "f.txt"]);
    run_in(&["fact", "--note", "before the tree moved"]);

    // A different, unread file changes — so this fact's own extent, label
    // and captured output are all unchanged.
    std::fs::write(repo.join("unrelated.txt"), "the tree is now dirty\n").unwrap();
    let ok = Command::new("git").arg("-C").arg(&repo).args(["add", "unrelated.txt"]).output().unwrap().status.success();
    assert!(ok, "test setup: git add must succeed");

    run_in(&["look", "f.txt"]);
    run_in(&["fact", "--note", "after the tree moved"]);

    let p = pins(&sb, "default");
    assert_eq!(p.len(), 2);
    assert_ne!(
        p[0], p[1],
        "the same file read against two tree states must not share a pin: {p:?}"
    );
}

// --- TET-28: a search records where it was rooted ----------------------

/// Every whole-search entry in a workspace's pending buffer, as
/// `(key, world_root, pattern)`.
///
/// Read through the buffer rather than the fact log so the assertions
/// below are about what capture recorded, not about what the fold kept.
fn search_entries(sb: &Sandbox, workspace: &str) -> Vec<(String, String, String)> {
    let raw = std::fs::read_to_string(
        sb.state_home().join(format!("workspaces/{workspace}/pending.json")),
    )
    .expect("pending buffer must exist");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .filter(|e| matches!(e["kind"].as_str(), Some("Search") | Some("NoMatch")))
        .map(|e| {
            (
                e["key"].as_str().unwrap_or_default().to_string(),
                e["world_root"].as_str().unwrap_or_default().to_string(),
                e["pattern"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[test]
fn a_grep_that_matched_records_where_it_was_rooted() {
    // The defect this closes, and the correction that produced TET-28's
    // design: a grep that found matches recorded one entry per matched
    // file and nothing about the search root, so a repo-wide search and a
    // single-file search that hit the same file were byte-identical in the
    // record. "Was this rooted at the worktree" — the whole of what a
    // census asserts — had nothing to read. Only the zero-match case
    // recorded a root.
    //
    // Asserted end to end through the binary, because the thing under test
    // is what capture writes down.
    let sb = Sandbox::new("tet28-search-rooting");
    let repo = sb.dir.join("repo");
    init_repo(&repo);
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::write(repo.join("sub/deep.txt"), "needle lives here\n").unwrap();

    let run_in = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tetel"));
        c.args(args).current_dir(&repo).env("TETEL_STATE_HOME", sb.state_home());
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    };

    // Three searches for the same pattern, finding the same file, differing
    // only in where they started.
    run_in(&["look", "--workspace", "wide", "--grep", "needle", "."]);
    run_in(&["look", "--workspace", "subdir", "--grep", "needle", "sub"]);
    run_in(&["look", "--workspace", "onefile", "--grep", "needle", "sub/deep.txt"]);

    for w in ["wide", "subdir", "onefile"] {
        let e = search_entries(&sb, w);
        assert_eq!(e.len(), 1, "{w}: exactly one whole-search record expected: {e:?}");
        assert_eq!(e[0].2, "needle", "{w}: the pattern must be stored structurally");
        assert!(!e[0].1.is_empty(), "{w}: premise — git must have resolved a root");
    }

    // Premise: all three found the same file, so nothing below is
    // discriminating on what was matched.
    let wide = search_entries(&sb, "wide").remove(0);
    let subdir = search_entries(&sb, "subdir").remove(0);
    let onefile = search_entries(&sb, "onefile").remove(0);
    assert_eq!(wide.1, subdir.1, "all three searched one worktree");
    assert_eq!(wide.1, onefile.1, "all three searched one worktree");

    // The discrimination that was impossible before this change.
    assert_eq!(wide.0, wide.1, "a search rooted at the worktree keys on its own root");
    assert_ne!(subdir.0, subdir.1, "a search rooted at a subdirectory is not worktree-rooted");
    assert_ne!(onefile.0, onefile.1, "a single-file search is not worktree-rooted");
}

#[test]
fn a_search_entry_survives_the_fold_into_a_fact() {
    // An observation's kind and pattern used to be dropped at mint time —
    // the extent kept key, label and marker, and what produced them was
    // recoverable only by reading the label as prose. TET-28's predicate
    // runs against a *fact*, read from the snapshot, so what matters is
    // what survives the fold, not what capture wrote.
    let sb = Sandbox::new("tet28-search-survives-fold");
    let repo = sb.dir.join("repo");
    init_repo(&repo);

    let run_in = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tetel"));
        c.args(args).current_dir(&repo).env("TETEL_STATE_HOME", sb.state_home());
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    };

    run_in(&["look", "--grep", "original", "."]);
    run_in(&["fact", "--note", "a census of the symbol"]);

    let raw = std::fs::read_to_string(sb.state_home().join("workspaces/default/facts.jsonl")).unwrap();
    let ev: serde_json::Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
    let extent = ev["extent"].as_array().unwrap();

    let search: Vec<_> = extent.iter().filter(|e| e["kind"] == "Search").collect();
    assert_eq!(search.len(), 1, "the whole-search record must survive the fold: {extent:#?}");
    assert_eq!(search[0]["pattern"], "original", "the pattern must survive the fold");
    assert_eq!(
        search[0]["key"], search[0]["world_root"],
        "and must still say it was worktree-rooted"
    );

    // The per-file hits survive as their own kind, so the predicate can
    // tell a census from the matches it found.
    assert!(
        extent.iter().any(|e| e["kind"] == "GrepMatch"),
        "per-file hits must still be recorded: {extent:#?}"
    );
}

// --- TET-28: the census refusal ----------------------------------------

/// A repository with a symbol used from two directories, so a sweep of
/// one of them is honest and still too narrow — the motivating defect's
/// shape.
fn repo_with_two_users(sb: &Sandbox) -> std::path::PathBuf {
    let repo = sb.dir.join("repo");
    init_repo(&repo);
    std::fs::create_dir_all(repo.join("core")).unwrap();
    std::fs::create_dir_all(repo.join("edge")).unwrap();
    std::fs::write(repo.join("core/a.rs"), "fn gate() {}\nfn main() { gate(); }\n").unwrap();
    // The second caller, in a directory a core-only sweep never opens.
    std::fs::write(repo.join("edge/b.rs"), "fn other() { gate(); }\n").unwrap();
    repo
}

#[test]
fn a_target_is_refused_when_its_census_swept_less_than_the_worktree() {
    // The defect this exists for: a memo recommended modifying a symbol
    // and spent a repo-wide negative about its callers, while the caller
    // sweep behind it was scoped to one directory. The fact was honest —
    // its own note said so — and nothing compared the sweep's width to
    // what was claimed from it.
    let sb = Sandbox::new("tet28-narrow-sweep-refused");
    let repo = repo_with_two_users(&sb);

    let run = |args: &[&str]| -> (bool, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tetel"));
        c.args(args).current_dir(&repo).env("TETEL_STATE_HOME", sb.state_home());
        let o = c.output().unwrap();
        (o.status.success(), String::from_utf8_lossy(&o.stderr).into_owned())
    };

    // An honest sweep of one directory.
    assert!(run(&["look", "--grep", "gate", "core"]).0);
    assert!(run(&["fact", "--note", "callers of gate, by grep over core/"]).0);

    let (ok, err) = run(&["target", "gate", "--cites", "F1"]);
    assert!(!ok, "a sweep narrower than the worktree must not census a target");
    assert!(
        err.contains("rooted at") && err.contains("core"),
        "the refusal must say which root fell short, not merely that one did: {err}"
    );

    // The remedy the refusal names, and nothing else, must clear it.
    assert!(run(&["look", "--grep", "gate", "."]).0);
    assert!(run(&["fact", "--note", "callers of gate, worktree-wide"]).0);
    let (ok, err) = run(&["target", "gate", "--cites", "F2"]);
    assert!(ok, "a worktree-rooted census must be accepted: {err}");
}

#[test]
fn a_census_pattern_must_be_the_symbol_itself() {
    // Containment would be unsound in the dangerous direction: a longer
    // pattern finds strictly fewer occurrences, so accepting `fn gate`
    // as a census of `gate` would accept a search that misses every
    // caller and finds only the definition.
    let sb = Sandbox::new("tet28-pattern-byte-equality");
    let repo = repo_with_two_users(&sb);

    let run = |args: &[&str]| -> (bool, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tetel"));
        c.args(args).current_dir(&repo).env("TETEL_STATE_HOME", sb.state_home());
        let o = c.output().unwrap();
        (o.status.success(), String::from_utf8_lossy(&o.stderr).into_owned())
    };

    // Rooted at the worktree, so only the pattern is wrong.
    assert!(run(&["look", "--grep", "fn gate", "."]).0);
    assert!(run(&["fact", "--note", "found the definition"]).0);
    let (ok, err) = run(&["target", "gate", "--cites", "F1"]);
    assert!(!ok, "a narrower pattern must not census the symbol");
    assert!(err.contains("byte for byte"), "the refusal must name the comparison: {err}");
}

#[test]
fn a_fact_that_was_read_rather_than_searched_censuses_nothing() {
    let sb = Sandbox::new("tet28-read-not-searched");
    let repo = repo_with_two_users(&sb);

    let run = |args: &[&str]| -> (bool, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tetel"));
        c.args(args).current_dir(&repo).env("TETEL_STATE_HOME", sb.state_home());
        let o = c.output().unwrap();
        (o.status.success(), String::from_utf8_lossy(&o.stderr).into_owned())
    };

    assert!(run(&["look", "core/a.rs"]).0);
    assert!(run(&["fact", "--note", "read the definition"]).0);
    let (ok, err) = run(&["target", "gate", "--cites", "F1"]);
    assert!(!ok, "reading a file is not censusing a symbol");
    assert!(err.contains("no search at all"), "{err}");
}

#[test]
fn a_target_row_the_snapshot_never_declared_fails_the_machine_partition() {
    // `target` refuses at authoring time, so a workspace-authored memo
    // cannot carry an uncensused target. This is the other direction: a
    // document edited after rendering, which never passed through the
    // refusing verb at all. A reviewer reads the table, so a table that
    // can say what the record does not support is the lie that matters.
    let sb = Sandbox::new("tet28-tampered-target-row");
    let repo = repo_with_two_users(&sb);
    let memo = sb.dir.join("memo.md");

    let run = |args: &[&str]| -> bool {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tetel"));
        c.args(args).current_dir(&repo).env("TETEL_STATE_HOME", sb.state_home());
        c.output().unwrap().status.success()
    };

    assert!(run(&["look", "--grep", "gate", "."]));
    assert!(run(&["fact", "--note", "callers of gate, worktree-wide"]));
    assert!(run(&["claim", "--proposition", "gate has two callers", "--cites", "F1"]));
    assert!(run(&["prose", "--text", "Two callers. See [C1]."]));
    assert!(run(&["target", "gate", "--cites", "F1"]));
    assert!(run(&["render", "--out", memo.to_str().unwrap()]));

    let clean = std::fs::read_to_string(&memo).unwrap();
    assert!(clean.contains("| `gate` | F1 |"), "premise: the target rendered:\n{clean}");
    let (code, out, _) = sb.run(&["check", memo.to_str().unwrap()]);
    assert_eq!(code, 0, "premise: the honest document checks clean:\n{out}");

    // Add a row nobody declared.
    std::fs::write(
        &memo,
        clean.replace("| `gate` | F1 |", "| `gate` | F1 |\n| `never_declared` | F1 |"),
    )
    .unwrap();
    let (code, out, _) = sb.run(&["check", memo.to_str().unwrap()]);
    assert_eq!(code, 1, "an invented target row must fail:\n{out}");
    assert!(
        out.contains("[uncensused-target]") && out.contains("never_declared"),
        "and must say which row and why:\n{out}"
    );
}

#[test]
fn the_targets_section_renders_when_it_is_empty() {
    // Under-declaration is unreachable by any refusal — deciding a memo
    // recommended something it never declared is the heuristic read the
    // whole design rejects. Visibility is the substitute, so the section
    // must not vanish when it would be embarrassing.
    let sb = Sandbox::new("tet28-empty-section-renders");
    let repo = repo_with_two_users(&sb);
    let memo = sb.dir.join("memo.md");

    let run = |args: &[&str]| -> bool {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tetel"));
        c.args(args).current_dir(&repo).env("TETEL_STATE_HOME", sb.state_home());
        c.output().unwrap().status.success()
    };

    assert!(run(&["look", "core/a.rs"]));
    assert!(run(&["fact", "--note", "read the definition"]));
    assert!(run(&["claim", "--proposition", "gate should be rewritten", "--cites", "F1"]));
    assert!(run(&["prose", "--text", "The implementer should rewrite it. See [C1]."]));
    assert!(run(&["render", "--out", memo.to_str().unwrap()]));

    let rendered = std::fs::read_to_string(&memo).unwrap();
    assert!(
        rendered.contains("## Modification targets") && rendered.contains("None declared"),
        "a memo telling an implementer to rewrite something must show its empty census section:\n{rendered}"
    );
}

// --- TET-29: the transplant premise inventory ------------------------
//
// A donor whose stated premise is a clause of a comment the author is
// already quoting — the shape of the motivating defect, where the
// precondition sat inside text minted as *support* for the transplant
// and nobody noticed it was also a condition on it.
fn repo_with_a_commented_donor(sb: &Sandbox) -> std::path::PathBuf {
    let repo = sb.dir.join("repo");
    init_repo(&repo);
    std::fs::write(
        repo.join("donor.rs"),
        "fn walk() {\n\
         \x20   // Record the visit before checking, not after: this is sound\n\
         \x20   // only because a parent is remote-known or was walked earlier\n\
         \x20   // in this same session.\n\
         \x20   visit();\n\
         }\n",
    )
    .unwrap();
    std::fs::write(repo.join("dest.rs"), "fn audit() { walk(); }\n").unwrap();
    repo
}

/// Set up a workspace with a donor fact (F1), a censused destination
/// target (T1) and a transplant (X1) declared between them.
fn transplant_fixture(sb: &Sandbox, repo: &std::path::Path) -> impl Fn(&[&str]) -> (bool, String) {
    let state = sb.state_home();
    let repo = repo.to_path_buf();
    move |args: &[&str]| -> (bool, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tetel"));
        c.args(args).current_dir(&repo).env("TETEL_STATE_HOME", &state);
        let o = c.output().unwrap();
        (
            o.status.success(),
            format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)),
        )
    }
}

#[test]
fn a_premise_the_donor_never_wrote_is_refused() {
    // The whole point: "transcribed verbatim" cannot be an honour-system
    // field. A paraphrase quietly weaker than the donor's own words is
    // exactly the failure the inventory exists to catch, so a premise
    // that is not in the captured bytes is not a quotation.
    let sb = Sandbox::new("tet29-invented-premise");
    let repo = repo_with_a_commented_donor(&sb);
    let run = transplant_fixture(&sb, &repo);

    assert!(run(&["look", "donor.rs"]).0);
    assert!(run(&["fact", "--note", "the donor's walk discipline"]).0);
    assert!(run(&["look", "--grep", "walk", "."]).0);
    assert!(run(&["fact", "--note", "every use of walk"]).0);
    assert!(run(&["target", "walk", "--cites", "F2"]).0);
    let (ok, out) = run(&["transplant", "--from", "F1", "--into", "T1"]);
    assert!(ok, "a donor fact and a live target must be enough to declare: {out}");

    // A plausible paraphrase of the real premise, in the author's words.
    let (ok, out) = run(&[
        "transplant",
        "--premise",
        "X1",
        "--text",
        "parents are always known before their children",
    ]);
    assert!(!ok, "a premise the donor never wrote must be refused");
    assert!(
        out.contains("not the donor's words"),
        "the refusal must say what failed — provenance, not truth: {out}"
    );

    // The donor's actual clause, byte for byte out of the capture.
    let (ok, out) = run(&[
        "transplant",
        "--premise",
        "X1",
        "--text",
        "a parent is remote-known or was walked earlier",
    ]);
    assert!(ok, "the donor's own words must be accepted: {out}");
}

#[test]
fn a_premise_may_span_lines_and_keep_its_comment_markers() {
    // The strictness objection, settled: a wrapped comment with `//`
    // prefixes and indentation inside it IS contiguous bytes in a `look`
    // capture. Stripping that noise would need per-language comment
    // knowledge, which this crate deliberately does not have.
    let sb = Sandbox::new("tet29-multiline-premise");
    let repo = repo_with_a_commented_donor(&sb);
    let run = transplant_fixture(&sb, &repo);

    assert!(run(&["look", "donor.rs"]).0);
    assert!(run(&["fact", "--note", "the donor's walk discipline"]).0);
    assert!(run(&["look", "--grep", "walk", "."]).0);
    assert!(run(&["fact", "--note", "every use of walk"]).0);
    assert!(run(&["target", "walk", "--cites", "F2"]).0);
    assert!(run(&["transplant", "--from", "F1", "--into", "T1"]).0);

    let wrapped = "    // only because a parent is remote-known or was walked earlier\n    // in this same session.";
    let (ok, out) = run(&["transplant", "--premise", "X1", "--text", wrapped]);
    assert!(ok, "a premise wrapped across lines with its markers intact must be accepted: {out}");
}

#[test]
fn a_premise_may_not_straddle_two_observations() {
    // The unsoundness the design found: a fact's output is the join of
    // its observations, so text spanning the seam between two captures is
    // a substring of the join while never having been contiguous in
    // anything anyone looked at. Accepting it would let an author
    // assemble a sentence the source does not contain out of two that it
    // does.
    let sb = Sandbox::new("tet29-seam-straddle");
    let repo = sb.dir.join("repo");
    init_repo(&repo);
    std::fs::write(repo.join("one.rs"), "the visit is recorded first\n").unwrap();
    std::fs::write(repo.join("two.rs"), "because the ordering is reversed\n").unwrap();
    std::fs::write(repo.join("dest.rs"), "fn audit() {}\n").unwrap();
    let run = transplant_fixture(&sb, &repo);

    // One fact folding two observations — the ordinary case.
    assert!(run(&["look", "one.rs"]).0);
    assert!(run(&["look", "two.rs"]).0);
    assert!(run(&["fact", "--note", "both halves of the donor"]).0);
    assert!(run(&["look", "--grep", "audit", "."]).0);
    assert!(run(&["fact", "--note", "every use of audit"]).0);
    assert!(run(&["target", "audit", "--cites", "F2"]).0);
    assert!(run(&["transplant", "--from", "F1", "--into", "T1"]).0);

    // Contiguous in the joined output, contiguous in neither capture.
    // Each file's bytes end in a newline and the join adds another, so
    // the seam is `\n\n` — spelling it with one would make this test pass
    // for the wrong reason, by failing containment against the join too.
    let straddling = "recorded first\n\nbecause the ordering";
    let (ok, out) = run(&["transplant", "--premise", "X1", "--text", straddling]);
    assert!(
        !ok,
        "text spanning the seam between two observations was never contiguous in anything the \
         author saw, and must not pass as a quotation"
    );
    assert!(out.contains("not the donor's words"), "{out}");

    // Either half alone is a genuine quotation.
    assert!(run(&["transplant", "--premise", "X1", "--text", "the visit is recorded first"]).0);
}

#[test]
fn a_donor_fact_minted_before_boundaries_were_recorded_cannot_be_quoted() {
    // The compatibility reading TET-28 set for `kind` and `pattern`:
    // absent means *not recorded*, never a default, so a legacy fact can
    // never satisfy the requirement and the remedy is the cheap one.
    let sb = Sandbox::new("tet29-legacy-donor");
    let repo = repo_with_a_commented_donor(&sb);
    let run = transplant_fixture(&sb, &repo);

    assert!(run(&["look", "donor.rs"]).0);
    assert!(run(&["fact", "--note", "the donor"]).0);
    assert!(run(&["look", "--grep", "walk", "."]).0);
    assert!(run(&["fact", "--note", "every use of walk"]).0);
    assert!(run(&["target", "walk", "--cites", "F2"]).0);

    // Strip the field back out, as a fact minted by an older build has it.
    let facts_path = sb.state_home().join("workspaces/default/facts.jsonl");
    let facts = std::fs::read_to_string(&facts_path).unwrap();
    let stripped: String = facts
        .lines()
        .map(|l| {
            let mut v: serde_json::Value = serde_json::from_str(l).unwrap();
            if let Some(extent) = v.get_mut("extent").and_then(|e| e.as_array_mut()) {
                for e in extent {
                    e.as_object_mut().unwrap().remove("out_len");
                }
            }
            serde_json::to_string(&v).unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&facts_path, format!("{stripped}\n")).unwrap();

    let (ok, out) = run(&["transplant", "--from", "F1", "--into", "T1"]);
    assert!(!ok, "a fact with no observation boundaries cannot be quoted from");
    assert!(
        out.contains("look at the donor again"),
        "the refusal must name the remedy rather than only the fault: {out}"
    );
}

#[test]
fn render_out_refuses_a_document_with_a_premise_nobody_answered() {
    // The refusal deliberately placed later than the verb and earlier
    // than `check`: an unanswered premise is legal while authoring —
    // transcribe first, answer after — but a memo carrying one should
    // fail to come into existence rather than exist and fail a check the
    // author may never run.
    let sb = Sandbox::new("tet29-render-completeness");
    let repo = repo_with_a_commented_donor(&sb);
    let run = transplant_fixture(&sb, &repo);

    assert!(run(&["look", "donor.rs"]).0);
    assert!(run(&["fact", "--note", "the donor's walk discipline"]).0);
    assert!(run(&["look", "--grep", "walk", "."]).0);
    assert!(run(&["fact", "--note", "every use of walk"]).0);
    assert!(run(&["target", "walk", "--cites", "F2"]).0);
    assert!(run(&["transplant", "--from", "F1", "--into", "T1"]).0);
    assert!(run(&["claim", "--proposition", "the walk order carries over", "--cites", "F1"]).0);
    assert!(run(&["transplant", "--premise", "X1", "--text", "in this same session"]).0);

    // A preview still assembles: the half-built state must stay visible
    // while it is being worked on.
    let (ok, preview) = run(&["render"]);
    assert!(ok, "a preview render must not refuse: {preview}");
    assert!(
        preview.contains("not yet answered"),
        "a preview must show the unanswered premise loudly:\n{preview}"
    );

    let (ok, out) = run(&["render", "--out", "memo.md"]);
    assert!(!ok, "`render --out` must refuse a document with an unanswered premise");
    assert!(out.contains("X1.1"), "the refusal must name which premise: {out}");
    assert!(
        !repo.join("memo.md").exists(),
        "the refused document must not have been written"
    );

    // The remedy the refusal names, and nothing else, clears it.
    assert!(run(&["transplant", "--discharge", "X1.1", "--cites", "C1"]).0);
    let (ok, out) = run(&["render", "--out", "memo.md"]);
    assert!(ok, "an answered premise must let the document be written: {out}");
    let rendered = std::fs::read_to_string(repo.join("memo.md")).unwrap();
    assert!(
        rendered.contains("in this same session") && rendered.contains("C1"),
        "the donor's words must be on the page beside the claim answering them:\n{rendered}"
    );
}

#[test]
fn a_transplant_must_land_on_a_declared_target() {
    // The composition with TET-28: a transplant installs a mechanism
    // somewhere, and that somewhere is a symbol the design is thereby
    // telling an implementer to modify. Requiring a live target makes one
    // declaration force the other's census.
    let sb = Sandbox::new("tet29-destination-is-a-target");
    let repo = repo_with_a_commented_donor(&sb);
    let run = transplant_fixture(&sb, &repo);

    assert!(run(&["look", "donor.rs"]).0);
    assert!(run(&["fact", "--note", "the donor"]).0);
    let (ok, out) = run(&["transplant", "--from", "F1", "--into", "T1"]);
    assert!(!ok, "a transplant with no declared destination must be refused");
    assert!(
        out.contains("tetel target"),
        "the refusal must name the verb that fixes it: {out}"
    );
}

#[test]
fn a_transplant_row_the_snapshot_never_declared_fails_the_machine_partition() {
    // The hand-authored document: `check` re-verifies against the shipped
    // snapshot rather than trusting the page, in both directions.
    let sb = Sandbox::new("tet29-invented-transplant-row");
    let repo = repo_with_a_commented_donor(&sb);
    let run = transplant_fixture(&sb, &repo);

    assert!(run(&["look", "donor.rs"]).0);
    assert!(run(&["fact", "--note", "the donor's walk discipline"]).0);
    assert!(run(&["look", "--grep", "walk", "."]).0);
    assert!(run(&["fact", "--note", "every use of walk"]).0);
    assert!(run(&["target", "walk", "--cites", "F2"]).0);
    assert!(run(&["claim", "--proposition", "the walk order carries over", "--cites", "F1"]).0);
    assert!(run(&["prose", "--text", "The order carries over. See [C1]."]).0);
    assert!(run(&["render", "--out", "memo.md"]).0);

    // A transplant section invented in the document, with no record
    // behind it anywhere.
    let memo = repo.join("memo.md");
    let text = std::fs::read_to_string(&memo).unwrap();
    std::fs::write(
        &memo,
        text.replace(
            "## Transplants\n",
            "## Transplants\n\n### X7 — F1 into `walk` (T1)\n",
        ),
    )
    .unwrap();

    let (_, out) = run(&["check", "memo.md"]);
    assert!(
        out.contains("[unquoted-premise]") && out.contains("X7"),
        "an invented transplant row must fail the machine partition:\n{out}"
    );
}

#[test]
fn a_premise_answered_by_a_withdrawn_claim_is_unanswered_again() {
    // The completeness question is asked in one place so `render --out`
    // and `check` cannot answer it differently — and a discharge whose
    // claim has since been withdrawn is not an answer.
    let sb = Sandbox::new("tet29-withdrawn-answer");
    let repo = repo_with_a_commented_donor(&sb);
    let run = transplant_fixture(&sb, &repo);

    assert!(run(&["look", "donor.rs"]).0);
    assert!(run(&["fact", "--note", "the donor's walk discipline"]).0);
    assert!(run(&["look", "--grep", "walk", "."]).0);
    assert!(run(&["fact", "--note", "every use of walk"]).0);
    assert!(run(&["target", "walk", "--cites", "F2"]).0);
    assert!(run(&["transplant", "--from", "F1", "--into", "T1"]).0);
    assert!(run(&["claim", "--proposition", "the walk order carries over", "--cites", "F1"]).0);
    assert!(run(&["transplant", "--premise", "X1", "--text", "in this same session"]).0);
    assert!(run(&["transplant", "--discharge", "X1.1", "--cites", "C1"]).0);
    assert!(run(&["render", "--out", "memo.md"]).0);

    assert!(run(&["claim", "--withdraw", "C1", "--why", "it does not hold after all"]).0);
    let (ok, out) = run(&["render", "--out", "memo2.md"]);
    assert!(!ok, "withdrawing the answering claim leaves the premise unanswered: {out}");
    assert!(out.contains("X1.1"), "{out}");
}
