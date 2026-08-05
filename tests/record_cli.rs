//! End-to-end tests for `tetel record`. Each test copies a checked-in
//! memo fixture into a private temporary directory before recording
//! against it, since `record` appends to `<memo>.evidence.jsonl` on disk
//! and the checked-in fixtures must stay read-only and unmodified by a
//! test run.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// A private, per-test copy of `fixture_name` inside a fresh temp
/// directory, cleaned up when it drops.
struct Sandbox {
    dir: PathBuf,
    memo: PathBuf,
}

impl Sandbox {
    fn new(fixture_name: &str, test_tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tetel-record-cli-test-{test_tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let memo = dir.join("memo.md");
        std::fs::copy(fixture(fixture_name), &memo).unwrap();
        Sandbox { dir, memo }
    }

    fn evidence_path(&self) -> PathBuf {
        let mut s = self.memo.as_os_str().to_os_string();
        s.push(".evidence.jsonl");
        PathBuf::from(s)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn record_via_stdin(memo: &Path, input: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tetel"))
        .arg("record")
        .arg(memo)
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

#[test]
fn appends_a_well_formed_record_from_stdin() {
    let sandbox = Sandbox::new("ledger_check.md", "wellformed");
    let (code, _out, err) = record_via_stdin(
        &sandbox.memo,
        r#"{"claim":"L-1","pass":"agent-a","verdict":"supports","note":"Opened `foo` in full."}"#,
    );
    assert_eq!(code, 0, "stderr:\n{err}");
    let contents = std::fs::read_to_string(sandbox.evidence_path()).unwrap();
    assert_eq!(contents.lines().count(), 1);
    let parsed: serde_json::Value = serde_json::from_str(contents.lines().next().unwrap()).unwrap();
    assert_eq!(parsed["_type"], "https://in-toto.io/Statement/v1");
    assert_eq!(parsed["subject"][0]["name"], "L-1");
    assert!(parsed["subject"][0]["digest"]["sha256"].is_string());
    assert_eq!(parsed["predicate"]["verdict"], "supports");
    assert_eq!(parsed["predicate"]["pass"], "agent-a");
    assert_eq!(parsed["predicate"]["note"], "Opened `foo` in full.");
}

#[test]
fn appends_a_well_formed_record_from_an_input_file() {
    let sandbox = Sandbox::new("ledger_check.md", "inputfile");
    let input_path = sandbox.dir.join("input.json");
    std::fs::write(&input_path, r#"{"claim":"L-2","pass":"agent-b","verdict":"qualifies","note":"partial"}"#).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_tetel"))
        .arg("record")
        .arg(&sandbox.memo)
        .arg("--input")
        .arg(&input_path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let contents = std::fs::read_to_string(sandbox.evidence_path()).unwrap();
    assert_eq!(contents.lines().count(), 1);
    assert!(contents.contains("qualifies"));
}

#[test]
fn a_note_with_backticks_and_embedded_newlines_survives_byte_exact() {
    let sandbox = Sandbox::new("ledger_check.md", "notesurvival");
    let note = "line one\nline two with `backticks` and a \"quote\"\nline three";
    let input = serde_json::json!({
        "claim": "L-1",
        "pass": "agent-c",
        "verdict": "supports",
        "note": note,
    })
    .to_string();
    let (code, _out, err) = record_via_stdin(&sandbox.memo, &input);
    assert_eq!(code, 0, "stderr:\n{err}");
    let contents = std::fs::read_to_string(sandbox.evidence_path()).unwrap();
    // The file itself is exactly one physical line — the JSON encoder
    // escaped the embedded newlines rather than emitting them raw, which
    // is what keeps this append-only format one-record-per-line.
    assert_eq!(contents.lines().count(), 1, "an embedded newline must not create a second physical line");
    let parsed: serde_json::Value = serde_json::from_str(contents.lines().next().unwrap()).unwrap();
    assert_eq!(parsed["predicate"]["note"], note, "the note must decode back byte-exact");
}

#[test]
fn refuses_an_unknown_claim_id_and_writes_nothing() {
    let sandbox = Sandbox::new("ledger_check.md", "unknownclaim");
    let (code, _out, err) = record_via_stdin(
        &sandbox.memo,
        r#"{"claim":"GHOST","pass":"agent-a","verdict":"supports"}"#,
    );
    assert_ne!(code, 0, "an unknown claim id must be refused");
    assert!(err.contains("unknown claim id"), "stderr was:\n{err}");
    assert!(!sandbox.evidence_path().exists(), "a refused record must never create the evidence file");
}

#[test]
fn refuses_a_missing_verdict_and_writes_nothing() {
    let sandbox = Sandbox::new("ledger_check.md", "missingverdict");
    let (code, _out, err) = record_via_stdin(&sandbox.memo, r#"{"claim":"L-1","pass":"agent-a"}"#);
    assert_ne!(code, 0, "a missing verdict must be refused");
    assert!(err.contains("verdict"), "stderr was:\n{err}");
    assert!(!sandbox.evidence_path().exists());
}

#[test]
fn refuses_malformed_json_and_writes_nothing() {
    let sandbox = Sandbox::new("ledger_check.md", "malformedjson");
    let (code, _out, err) = record_via_stdin(&sandbox.memo, "{this is not json");
    assert_ne!(code, 0, "malformed JSON must be refused");
    assert!(err.contains("malformed record"), "stderr was:\n{err}");
    assert!(!sandbox.evidence_path().exists());
}

#[test]
fn a_second_append_never_rewrites_the_first_record() {
    let sandbox = Sandbox::new("ledger_check.md", "appendonly");
    let (code1, _, err1) = record_via_stdin(&sandbox.memo, r#"{"claim":"L-1","pass":"agent-a","verdict":"supports"}"#);
    assert_eq!(code1, 0, "stderr:\n{err1}");
    let first_contents = std::fs::read_to_string(sandbox.evidence_path()).unwrap();

    let (code2, _, err2) = record_via_stdin(&sandbox.memo, r#"{"claim":"L-2","pass":"agent-b","verdict":"refutes"}"#);
    assert_eq!(code2, 0, "stderr:\n{err2}");
    let second_contents = std::fs::read_to_string(sandbox.evidence_path()).unwrap();

    assert!(
        second_contents.starts_with(&first_contents),
        "the first record must be byte-identical and first after a second append"
    );
    assert_eq!(second_contents.lines().count(), 2);
}
