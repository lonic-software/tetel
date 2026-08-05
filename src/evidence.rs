//! Evidence records: one JSON object per line, appended to
//! `<memo>.evidence.jsonl`, never rewritten. The payload is shaped on
//! in-toto's Statement layer (v1: `_type`, `subject`, `predicateType`,
//! `predicate`; https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md)
//! where that shape fits naturally — no DSSE envelope, no in-toto crate,
//! just the field shape. `subject` carries one entry: the claim id as
//! `name`, and a sha256 digest of the exact proposition text the pass
//! graded, which is what `subject.digest` is required to carry and also
//! ties the record to the byte-exact text it was graded against.
//!
//! Every record this module can produce is **ingested**: someone reports
//! that an act happened elsewhere — verdict, extent and note, typed by a
//! caller. There is no witnessed-capture path in this crate yet (no code
//! path lets the tool observe the act itself), so [`INGESTED_PREDICATE_TYPE`]
//! is the *only* predicate type in circulation today. It exists, and is
//! named distinctly, so that when a witnessed `captured-fact` predicate
//! type is eventually added, the two are structurally distinguishable —
//! provenance is carried by which shape a record has, not by a field
//! someone remembers to set.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::ledger::Claim;
use crate::model::Kind;

pub const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";

/// The predicate type for **ingested** evidence — this crate's only
/// evidence shape today. A future witnessed-capture path (the tool
/// observing its own reads, extent captured rather than typed) must mint
/// its own `captured-fact`-shaped predicate type, distinct from this one,
/// and that captured variant must never gain a caller-supplied `extent`
/// field — the caller-supplied `extent` on *this* predicate is exactly
/// what marks a record as reported rather than witnessed, and giving the
/// captured type the same field would erase the distinction this type
/// exists to carry.
pub const INGESTED_PREDICATE_TYPE: &str = "https://github.com/lonic-software/tetel/grounding-result/v1";

/// The only values a reporter may state for `reported_kind` — what kind of
/// act they say this was. Carried verbatim as data (see [`Predicate`] and
/// [`EvidenceRecord`]); never paraphrased, never normalised. Validated
/// against this exact, case-sensitive set both when a record is written
/// and when one is read back, so a hand-edited or pre-this-slice file
/// can't smuggle an unrecognised claim through unnoticed.
pub const REPORTED_KINDS: &[&str] = &["run", "reading", "observed", "attested"];

fn valid_reported_kind(s: &str) -> bool {
    REPORTED_KINDS.contains(&s)
}

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

/// Where an ingested record's act is recorded: a file path, or
/// `proc:<session-or-agent>` for a transcript-only act with nothing on
/// disk to point at. Mirrors `model::Designator`'s `Path`/`Proc` shapes,
/// but is its own type: a source names exactly one place an act is
/// recorded, not a comma-separated scope, and `Designator`'s
/// `Symbol`/`External` forms don't answer "where is this recorded".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Path(String),
    Proc(String),
}

impl Source {
    /// `None` for an empty string, or a `proc:` prefix with nothing after
    /// it — both are refused by [`record`] rather than stored.
    pub fn parse(raw: &str) -> Option<Source> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(rest) = trimmed.strip_prefix("proc:") {
            let rest = rest.trim();
            if rest.is_empty() {
                return None;
            }
            return Some(Source::Proc(rest.to_string()));
        }
        Some(Source::Path(trimmed.to_string()))
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
    /// What kind of act the reporter says this was — `run` | `reading` |
    /// `observed` | `attested`. Stored exactly as given; see
    /// [`REPORTED_KINDS`]. Never read by anything that computes standing:
    /// see [`EvidenceRecord::derived_kind`] for why.
    reported_kind: String,
    /// Where the act is recorded — a file path, or `proc:<session-or-agent>`.
    /// Required: an ingested record with nothing preserved anywhere is
    /// refused outright, not merely flagged. Stored verbatim; parsed back
    /// into a [`Source`] by whoever needs to resolve it.
    source: String,
    /// Caller-supplied. This field is exactly what an ingested record is
    /// allowed to have and a witnessed one (once it exists) never will —
    /// see the doc comment on [`INGESTED_PREDICATE_TYPE`].
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
    /// What kind of act the reporter says this was — required, one of
    /// [`REPORTED_KINDS`]. Stored verbatim, never paraphrased or
    /// normalised, and never consulted when standing is derived.
    pub reported_kind: String,
    /// Where the act is recorded — a file path, or
    /// `proc:<session-or-agent>` for a transcript-only act. Required: a
    /// record naming nothing preserved anywhere is refused.
    pub source: String,
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
    /// What the reporter said this act was. Carried for fidelity and
    /// display; deliberately *not* read by [`EvidenceRecord::derived_kind`].
    #[allow(dead_code)] // carried for fidelity; no check reasons over its value directly — see derived_kind
    pub reported_kind: String,
    /// Where the act is recorded — a file path, or `proc:<session-or-agent>`.
    /// Read by `checks::unresolved_evidence_sources`.
    pub source: String,
    #[allow(dead_code)] // carried for fidelity; no check reasons over it yet
    pub extent: Vec<String>,
    pub note: Option<String>,
    #[allow(dead_code)] // carried for fidelity; no check reasons over it yet
    pub pin: Option<String>,
    #[allow(dead_code)] // carried for fidelity; no check reasons over it yet
    pub timestamp: u64,
}

impl EvidenceRecord {
    /// The kind this record counts as for anything that derives a claim's
    /// standing — always [`Kind::Attested`], regardless of what
    /// `reported_kind` says.
    ///
    /// This is deliberate, not a stub: the reporter states what kind of
    /// act it was, but the tool witnessed only the *saying*, never the
    /// act — a report of a run is not a run. Treating every ingested
    /// record as `Attested` for standing is what keeps ingestion from
    /// moving a claim above "vouched" (permanently human-owed): reading,
    /// observed and attested rows all cap out there, and this method's
    /// whole job is to make sure a reported `run` can't sneak past that
    /// cap by claiming a stronger kind than the tool ever saw for itself.
    pub fn derived_kind(&self) -> Kind {
        Kind::Attested
    }
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
        predicate_type: INGESTED_PREDICATE_TYPE.to_string(),
        predicate: Predicate {
            verdict: verdict.to_string(),
            pass: input.pass.clone(),
            reported_kind: input.reported_kind.clone(),
            source: input.source.clone(),
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
    if !valid_reported_kind(input.reported_kind.trim()) {
        return Err(RecordError::MalformedRecord(format!(
            "missing or invalid `reported_kind` (got {:?}); expected one of {}",
            input.reported_kind,
            REPORTED_KINDS.join(", ")
        )));
    }
    // Refused, not merely flagged: an ingested record naming nothing
    // preserved anywhere would let a caller fabricate an attested fact
    // for free. `Source::parse` rejects both an empty string and a
    // `proc:` prefix with nothing after it.
    if Source::parse(&input.source).is_none() {
        return Err(RecordError::MalformedRecord(
            "missing or malformed `source` (a file path, or `proc:<session-or-agent>`)".to_string(),
        ));
    }
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
    if !valid_reported_kind(&statement.predicate.reported_kind) {
        return Err(format!("invalid `reported_kind` `{}`", statement.predicate.reported_kind));
    }
    if Source::parse(&statement.predicate.source).is_none() {
        return Err(format!("missing or malformed `source` `{}`", statement.predicate.source));
    }
    Ok(EvidenceRecord {
        claim_id: subject.name.clone(),
        verdict,
        pass: statement.predicate.pass.clone(),
        reported_kind: statement.predicate.reported_kind.clone(),
        source: statement.predicate.source.clone(),
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
        let input = r#"{"claim":"X-1","pass":"agent-b","verdict":"supports","reported_kind":"observed","source":"src/lib.rs","note":"line one\nline two with `backticks`"}"#;
        record(&memo, &claims, input).expect("valid record must be accepted");

        let (records, errors) = load(&memo).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].claim_id, "X-1");
        assert_eq!(records[0].verdict, Verdict::Supports);
        assert_eq!(records[0].note.as_deref(), Some("line one\nline two with `backticks`"));
        assert_eq!(records[0].reported_kind, "observed");
        assert_eq!(records[0].source, "src/lib.rs");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_claim_id_is_refused_and_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("tetel-evidence-test-{}-unknown", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let memo = dir.join("memo.md");
        let claims = vec![claim("X-1", "X does the thing")];
        let input = r#"{"claim":"GHOST","pass":"agent-b","verdict":"supports","reported_kind":"observed","source":"src/lib.rs"}"#;
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
        let input = r#"{"claim":"X-1","pass":"agent-b","verdict":"maybe","reported_kind":"observed","source":"src/lib.rs"}"#;
        let err = record(&memo, &claims, input).unwrap_err();
        assert!(matches!(err, RecordError::MalformedRecord(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_reported_kind_is_refused_and_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("tetel-evidence-test-{}-nokind", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let memo = dir.join("memo.md");
        let claims = vec![claim("X-1", "X does the thing")];
        let input = r#"{"claim":"X-1","pass":"agent-b","verdict":"supports","source":"src/lib.rs"}"#;
        let err = record(&memo, &claims, input).unwrap_err();
        assert!(
            matches!(err, RecordError::MalformedJson(_) | RecordError::MalformedRecord(_)),
            "a record with no `reported_kind` at all must be refused: {err}"
        );
        assert!(!evidence_path(&memo).exists(), "a refused record must not write a file");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_reported_kind_value_is_refused() {
        let dir = std::env::temp_dir().join(format!("tetel-evidence-test-{}-badkind", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let memo = dir.join("memo.md");
        let claims = vec![claim("X-1", "X does the thing")];
        let input = r#"{"claim":"X-1","pass":"agent-b","verdict":"supports","reported_kind":"witnessed","source":"src/lib.rs"}"#;
        let err = record(&memo, &claims, input).unwrap_err();
        assert!(matches!(err, RecordError::MalformedRecord(_)));
        assert!(!evidence_path(&memo).exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_source_is_refused_and_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("tetel-evidence-test-{}-nosource", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let memo = dir.join("memo.md");
        let claims = vec![claim("X-1", "X does the thing")];
        let input = r#"{"claim":"X-1","pass":"agent-b","verdict":"supports","reported_kind":"observed"}"#;
        let err = record(&memo, &claims, input).unwrap_err();
        assert!(
            matches!(err, RecordError::MalformedJson(_) | RecordError::MalformedRecord(_)),
            "a record with no `source` at all must be refused: {err}"
        );
        assert!(!evidence_path(&memo).exists(), "a refused record must not write a file");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reported_kind_run_is_still_derived_as_attested_for_standing() {
        // The reporter says this was a run; the tool witnessed only the
        // saying. Derived treatment must ignore the claim entirely — see
        // `EvidenceRecord::derived_kind`'s doc comment for why this
        // asymmetry is the point, not an oversight.
        let dir = std::env::temp_dir().join(format!("tetel-evidence-test-{}-runkind", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let memo = dir.join("memo.md");
        let claims = vec![claim("X-1", "X does the thing")];
        let input = r#"{"claim":"X-1","pass":"agent-b","verdict":"supports","reported_kind":"run","source":"src/lib.rs"}"#;
        record(&memo, &claims, input).expect("a `run`-shaped reported_kind is a valid value, not refused");

        let (records, errors) = load(&memo).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(records[0].reported_kind, "run", "the claimed kind is carried verbatim");
        assert_eq!(
            records[0].derived_kind(),
            Kind::Attested,
            "derived standing must never be `run` for an ingested record, no matter what it claims"
        );

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
        record(
            &memo,
            &claims,
            r#"{"claim":"X-1","pass":"a","verdict":"supports","reported_kind":"observed","source":"src/lib.rs"}"#,
        )
        .unwrap();
        let first_line = std::fs::read_to_string(evidence_path(&memo)).unwrap();
        record(
            &memo,
            &claims,
            r#"{"claim":"X-2","pass":"b","verdict":"refutes","reported_kind":"attested","source":"proc:agent-b"}"#,
        )
        .unwrap();
        let after = std::fs::read_to_string(evidence_path(&memo)).unwrap();
        assert!(after.starts_with(&first_line), "the first line must survive a second append unchanged");
        assert_eq!(after.lines().count(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }
}
