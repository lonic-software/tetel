//! Evidence records: one JSON object per line, appended to
//! `<memo>.evidence.jsonl`, never rewritten. The payload is shaped on
//! in-toto's Statement layer (v1: `_type`, `subject`, `predicateType`,
//! `predicate`; https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md)
//! where that shape fits naturally — no DSSE envelope, no in-toto crate,
//! just the field shape. `subject` carries one entry: the claim id as
//! `name`, and a sha256 digest of the exact proposition text the pass
//! graded, which is what `subject.digest` is required to carry and also
//! ties the record to the byte-exact text it was graded against.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::ledger::Claim;

pub const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";
pub const PREDICATE_TYPE: &str = "https://github.com/lonic-software/tetel/grounding-result/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Supports,
    Refutes,
    Qualifies,
}

impl Verdict {
    pub fn parse(s: &str) -> Option<Verdict> {
        match s {
            "supports" => Some(Verdict::Supports),
            "refutes" => Some(Verdict::Refutes),
            "qualifies" => Some(Verdict::Qualifies),
            _ => None,
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Verdict::Supports => "supports",
            Verdict::Refutes => "refutes",
            Verdict::Qualifies => "qualifies",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DigestSet {
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Subject {
    name: String,
    digest: DigestSet,
}

#[derive(Debug, Serialize, Deserialize)]
struct Predicate {
    verdict: String,
    pass: String,
    #[serde(default)]
    extent: Vec<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    pin: Option<String>,
    timestamp: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Statement {
    #[serde(rename = "_type")]
    type_: String,
    subject: Vec<Subject>,
    #[serde(rename = "predicateType")]
    predicate_type: String,
    predicate: Predicate,
}

/// What the CLI's `record` command reads — from a file or stdin, never
/// from argv, so a note with backticks or embedded newlines survives.
#[derive(Debug, Deserialize)]
pub struct RecordInput {
    pub claim: String,
    pub pass: String,
    pub verdict: String,
    #[serde(default)]
    pub extent: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub pin: Option<String>,
}

/// One evidence record, as read back out of `<memo>.evidence.jsonl` for
/// use by `tetel check`.
#[derive(Debug, Clone)]
pub struct EvidenceRecord {
    pub claim_id: String,
    pub verdict: Verdict,
    pub pass: String,
    #[allow(dead_code)] // carried for fidelity; no check reasons over it yet
    pub extent: Vec<String>,
    pub note: Option<String>,
    #[allow(dead_code)] // carried for fidelity; no check reasons over it yet
    pub pin: Option<String>,
    #[allow(dead_code)] // carried for fidelity; no check reasons over it yet
    pub timestamp: u64,
}

pub fn sha256_hex(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn build_statement(claim: &Claim, input: &RecordInput, verdict: Verdict) -> Statement {
    Statement {
        type_: STATEMENT_TYPE.to_string(),
        subject: vec![Subject {
            name: claim.id.clone(),
            digest: DigestSet {
                sha256: sha256_hex(&claim.proposition),
            },
        }],
        predicate_type: PREDICATE_TYPE.to_string(),
        predicate: Predicate {
            verdict: verdict.to_string(),
            pass: input.pass.clone(),
            extent: input.extent.clone(),
            note: input.note.clone(),
            pin: input.pin.clone(),
            timestamp: now_unix(),
        },
    }
}

/// The path `tetel` reads and appends evidence at, derived from the memo's
/// own path: `<memo>.evidence.jsonl`, sitting next to it.
pub fn evidence_path(memo: &Path) -> PathBuf {
    let mut s = memo.as_os_str().to_os_string();
    s.push(".evidence.jsonl");
    PathBuf::from(s)
}

#[derive(Debug)]
pub enum RecordError {
    MalformedJson(String),
    MalformedRecord(String),
    UnknownClaim(String),
    Io(String),
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordError::MalformedJson(e) => write!(f, "malformed record: invalid JSON: {e}"),
            RecordError::MalformedRecord(e) => write!(f, "malformed record: {e}"),
            RecordError::UnknownClaim(id) => write!(f, "unknown claim id `{id}` — not in this memo's evidence ledger"),
            RecordError::Io(e) => write!(f, "could not append evidence: {e}"),
        }
    }
}

/// Validate `input_json` against `claims` and, only if every check passes,
/// append exactly one line to `<memo>.evidence.jsonl`. Never a partial
/// write: the statement is fully built in memory before the file is
/// touched, and any failure returns before `append` is called.
pub fn record(memo: &Path, claims: &[Claim], input_json: &str) -> Result<(), RecordError> {
    let input: RecordInput =
        serde_json::from_str(input_json).map_err(|e| RecordError::MalformedJson(e.to_string()))?;

    if input.claim.trim().is_empty() {
        return Err(RecordError::MalformedRecord("missing `claim`".to_string()));
    }
    if input.pass.trim().is_empty() {
        return Err(RecordError::MalformedRecord("missing `pass` (grounding pass identity)".to_string()));
    }
    let verdict = Verdict::parse(input.verdict.trim()).ok_or_else(|| {
        RecordError::MalformedRecord(format!(
            "missing or invalid `verdict` (got {:?}); expected one of supports, refutes, qualifies",
            input.verdict
        ))
    })?;
    let claim = claims
        .iter()
        .find(|c| c.id == input.claim)
        .ok_or_else(|| RecordError::UnknownClaim(input.claim.clone()))?;

    let statement = build_statement(claim, &input, verdict);
    let line = serde_json::to_string(&statement)
        .map_err(|e| RecordError::Io(format!("could not serialize record: {e}")))?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(evidence_path(memo))
        .map_err(|e| RecordError::Io(e.to_string()))?;
    writeln!(file, "{line}").map_err(|e| RecordError::Io(e.to_string()))?;
    Ok(())
}

/// Read `<memo>.evidence.jsonl` if it exists. A missing file is not an
/// error — it means no grounding pass has recorded anything yet. Each
/// line that fails to parse is reported (`line N: ...`), never silently
/// skipped; parsing continues past it so one bad line doesn't hide the
/// rest.
pub fn load(memo: &Path) -> io::Result<(Vec<EvidenceRecord>, Vec<String>)> {
    let path = evidence_path(memo);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), Vec::new())),
        Err(e) => return Err(e),
    };

    let mut records = Vec::new();
    let mut errors = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line) {
            Ok(r) => records.push(r),
            Err(e) => errors.push(format!("{}:{line_no}: {e}", path.display())),
        }
    }
    Ok((records, errors))
}

fn parse_line(line: &str) -> Result<EvidenceRecord, String> {
    let statement: Statement = serde_json::from_str(line).map_err(|e| format!("invalid JSON: {e}"))?;
    if statement.type_ != STATEMENT_TYPE {
        return Err(format!("unexpected `_type` `{}`", statement.type_));
    }
    let subject = statement
        .subject
        .first()
        .ok_or_else(|| "`subject` is empty".to_string())?;
    let verdict = Verdict::parse(&statement.predicate.verdict)
        .ok_or_else(|| format!("invalid verdict `{}`", statement.predicate.verdict))?;
    Ok(EvidenceRecord {
        claim_id: subject.name.clone(),
        verdict,
        pass: statement.predicate.pass.clone(),
        extent: statement.predicate.extent.clone(),
        note: statement.predicate.note.clone(),
        pin: statement.predicate.pin.clone(),
        timestamp: statement.predicate.timestamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Claim;

    fn claim(id: &str, proposition: &str) -> Claim {
        Claim {
            line: 1,
            id: id.to_string(),
            proposition: proposition.to_string(),
            domain: "d".to_string(),
            extent: "e".to_string(),
            kind: None,
            status: "**VERIFIED**".to_string(),
            pin: None,
        }
    }

    #[test]
    fn records_a_well_formed_input_and_round_trips_through_load() {
        let dir = std::env::temp_dir().join(format!("tetel-evidence-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let memo = dir.join("memo.md");
        let claims = vec![claim("X-1", "X does the thing")];
        let input = r#"{"claim":"X-1","pass":"agent-b","verdict":"supports","note":"line one\nline two with `backticks`"}"#;
        record(&memo, &claims, input).expect("valid record must be accepted");

        let (records, errors) = load(&memo).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].claim_id, "X-1");
        assert_eq!(records[0].verdict, Verdict::Supports);
        assert_eq!(records[0].note.as_deref(), Some("line one\nline two with `backticks`"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_claim_id_is_refused_and_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("tetel-evidence-test-{}-unknown", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let memo = dir.join("memo.md");
        let claims = vec![claim("X-1", "X does the thing")];
        let input = r#"{"claim":"GHOST","pass":"agent-b","verdict":"supports"}"#;
        let err = record(&memo, &claims, input).unwrap_err();
        assert!(matches!(err, RecordError::UnknownClaim(_)));
        assert!(!evidence_path(&memo).exists(), "a refused record must not write a file");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_verdict_is_refused() {
        let dir = std::env::temp_dir().join(format!("tetel-evidence-test-{}-noverdict", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let memo = dir.join("memo.md");
        let claims = vec![claim("X-1", "X does the thing")];
        let input = r#"{"claim":"X-1","pass":"agent-b","verdict":"maybe"}"#;
        let err = record(&memo, &claims, input).unwrap_err();
        assert!(matches!(err, RecordError::MalformedRecord(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn malformed_json_is_refused() {
        let dir = std::env::temp_dir().join(format!("tetel-evidence-test-{}-badjson", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let memo = dir.join("memo.md");
        let claims = vec![claim("X-1", "X does the thing")];
        let err = record(&memo, &claims, "{not json").unwrap_err();
        assert!(matches!(err, RecordError::MalformedJson(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn second_append_never_rewrites_the_first_line() {
        let dir = std::env::temp_dir().join(format!("tetel-evidence-test-{}-append", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let memo = dir.join("memo.md");
        let claims = vec![claim("X-1", "X does the thing"), claim("X-2", "Y does another thing")];
        record(&memo, &claims, r#"{"claim":"X-1","pass":"a","verdict":"supports"}"#).unwrap();
        let first_line = std::fs::read_to_string(evidence_path(&memo)).unwrap();
        record(&memo, &claims, r#"{"claim":"X-2","pass":"b","verdict":"refutes"}"#).unwrap();
        let after = std::fs::read_to_string(evidence_path(&memo)).unwrap();
        assert!(after.starts_with(&first_line), "the first line must survive a second append unchanged");
        assert_eq!(after.lines().count(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }
}
