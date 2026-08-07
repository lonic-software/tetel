//! `tetel` — a checker for markdown design documents whose factual claims
//! carry executable evidence. It never writes a file itself outside of
//! `tetel record`'s own append-only evidence log, never executes a
//! command from the document, and makes no network calls.

pub mod brief;
pub mod checks;
pub mod citations;
pub mod claims;
pub mod compose;
pub mod evidence;
pub mod facts;
pub mod ledger;
pub mod mcp;
pub mod model;
pub mod observe;
pub mod parse;
pub mod pending;
pub mod prose;
pub mod query;
pub mod report;
pub mod review;
pub mod scope;
pub mod snapshot;
pub mod workspace;
pub mod worldstate;

use std::path::Path;

pub use report::{EXIT_CHECK_FAILED, EXIT_CLEAN, EXIT_NO_ROWS};

/// Runs `check`'s five `tetel`-row checks against already-loaded source
/// text, with no evidence-ledger awareness (there is no file path to load
/// `<memo>.evidence.jsonl` from). `display_path` is used only in the
/// no-rows-found message. Kept for testing the row-grammar checks against
/// bare strings; the CLI always goes through [`check_file`].
pub fn check_str(display_path: &str, source: &str) -> (i32, String) {
    let doc = parse::parse_document(source);
    let ledger = ledger::import(&doc.body);
    let mut findings = checks::analyze(&doc, &ledger.claims);
    findings.ledger_claims_found = ledger.claims.len();
    findings.ledger_errors = ledger.errors.iter().map(|e| format!("line {}: {}", e.line, e.message)).collect();
    report::render(display_path, &doc, &findings)
}

/// Runs `check` against a file on disk: the five `tetel`-row checks, plus
/// the two ledger checks (ungrounded claims, verdict disagreement) against
/// whatever evidence has been recorded in `<file>.evidence.jsonl`, if it
/// exists.
pub fn check_file(path: &Path) -> std::io::Result<(i32, String)> {
    let source = std::fs::read_to_string(path)?;
    let doc = parse::parse_document(&source);
    let ledger = ledger::import(&doc.body);
    let mut findings = checks::analyze(&doc, &ledger.claims);

    let (evidence_records, evidence_errors) = evidence::load(path)?;
    let (ungrounded, attested_grounded, disagreements, qualified, stale, superseded) =
        checks::analyze_ledger(&ledger.claims, &evidence_records);

    findings.ledger_claims_found = ledger.claims.len();
    findings.ledger_errors = ledger
        .errors
        .iter()
        .map(|e| format!("line {}: {}", e.line, e.message))
        .chain(evidence_errors)
        .collect();
    findings.ungrounded_claims = ungrounded;
    findings.attested_grounded_claims = attested_grounded;
    findings.unresolved_evidence_sources = checks::unresolved_evidence_sources(&ledger.claims, &evidence_records);
    findings.no_scope_claims = checks::claims_without_declared_scope(&ledger.claims);
    findings.ledger_has_no_scope_columns = checks::ledger_has_no_scope_columns(&ledger.claims);
    // Who authored this memo, read from the snapshot shipped beside it —
    // without which self-grounding and independent grounding are
    // indistinguishable.
    // The two absent cases are kept apart: a memo never rendered by
    // `render --out` has no snapshot, while one rendered by a build that
    // did not ship an identity has a snapshot without one. They call for
    // different actions and only one of them is repairable.
    let snapshot_dir = snapshot::snapshot_path(path);
    let authoring_identity = match workspace::identity_of(&snapshot_dir) {
        Some(id) => checks::AuthoringIdentity::Known(id),
        None if snapshot_dir.is_dir() => checks::AuthoringIdentity::SnapshotWithoutIdentity,
        None => checks::AuthoringIdentity::NoSnapshot,
    };
    findings.grounding_provenance =
        checks::grounding_provenance(&ledger.claims, &evidence_records, &authoring_identity);
    findings.verdict_disagreements = disagreements;
    findings.qualified_claims = qualified;
    findings.stale_evidence = stale;
    findings.superseded_evidence = superseded;
    // Provenance is graded against the same bytes every other check saw,
    // never a re-read of the file.
    findings.provenance = snapshot::check(path, &source);
    // Facts live in the workspace, never in the rendered document, so the
    // note-vs-extent check is only possible when a snapshot was shipped
    // beside the memo. One more thing `render --out` buys a reviewer.
    let snapshot_dir = snapshot::snapshot_path(path);
    if snapshot_dir.is_dir() {
        if let Ok(facts) = facts::load_all(&snapshot_dir) {
            findings.notes_outside_extent = scope::outside_extent(&facts);
        }
    }
    // A document with no citations owes no snapshot; only one that points
    // at workspace-relative ids has a record to be missing.
    findings.cites_something = !citations::scan_citations(&doc.body).is_empty();

    Ok(report::render(&path.display().to_string(), &doc, &findings))
}

/// Runs `brief` against a file on disk: every claim in its evidence
/// ledger, id and proposition only, scope withheld. Returns `(exit_code,
/// output)`; a memo with no evidence ledger at all uses [`EXIT_NO_ROWS`].
pub fn brief_file(path: &Path, json: bool) -> std::io::Result<(i32, String)> {
    let source = std::fs::read_to_string(path)?;
    let doc = parse::parse_document(&source);
    let ledger = ledger::import(&doc.body);
    let display_path = path.display().to_string();

    if ledger.claims.is_empty() && ledger.errors.is_empty() {
        return Ok((
            EXIT_NO_ROWS,
            format!("no evidence ledger found in {display_path} — nothing to brief.\n"),
        ));
    }

    let mut out = String::new();
    let code = if ledger.errors.is_empty() { EXIT_CLEAN } else { EXIT_CHECK_FAILED };
    if !ledger.errors.is_empty() {
        out.push_str(&format!("ledger import errors in {display_path} (never silently dropped):\n"));
        for e in &ledger.errors {
            out.push_str(&format!("  - line {}: {}\n", e.line, e.message));
        }
        out.push('\n');
    }
    let items = brief::build(&ledger.claims);
    out.push_str(&if json {
        brief::render_json(&items)
    } else {
        brief::render_text(&display_path, &items)
    });
    Ok((code, out))
}

/// Grounds a claim on a fact this workspace captured, appending one
/// witnessed record to `<memo>.evidence.jsonl`. Returns the workspace
/// identity the record now carries.
///
/// The extent is copied from the fact, never taken from a caller — see
/// [`evidence::record_from_fact`] for why that absence is the whole
/// distinction between this path and [`record_file`].
pub fn record_from_fact_file(
    memo: &Path,
    workspace_dir: &Path,
    claim_id: &str,
    verdict: evidence::Verdict,
    fact_id: &str,
    note: Option<String>,
) -> std::io::Result<Result<String, evidence::RecordError>> {
    let source = std::fs::read_to_string(memo)?;
    let doc = parse::parse_document(&source);
    let ledger = ledger::import(&doc.body);

    let facts = facts::load_all(workspace_dir)?;
    let Some(fact) = facts.iter().find(|f| f.id == fact_id) else {
        return Ok(Err(evidence::RecordError::MalformedRecord(format!(
            "no fact `{fact_id}` in this workspace; try `tetel query facts`"
        ))));
    };
    let identity = workspace::identity(workspace_dir)?;
    Ok(
        evidence::record_from_fact(memo, &ledger.claims, claim_id, verdict, fact, &identity, note)
            .map(|()| identity.id),
    )
}

/// Runs `record` against a file on disk: validates `input_json` (a single
/// grounding result, shaped as described in `evidence.rs`) against the
/// memo's own evidence ledger, and if it is well-formed and its claim id
/// is known, appends exactly one line to `<memo>.evidence.jsonl`. Never a
/// partial write.
///
/// This is the *ingested* path: extent and source are typed by the caller.
/// See [`record_from_fact_file`] for the witnessed one.
pub fn record_file(path: &Path, input_json: &str) -> std::io::Result<Result<(), evidence::RecordError>> {
    let source = std::fs::read_to_string(path)?;
    let doc = parse::parse_document(&source);
    let ledger = ledger::import(&doc.body);
    Ok(evidence::record(path, &ledger.claims, input_json))
}
