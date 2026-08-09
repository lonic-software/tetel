//! `tetel` — prevention at authoring time, not review afterwards: designs
//! whose factual claims cannot be written without the executable evidence
//! that holds them up.
//!
//! Markdown is what `render` emits, not what this crate is about. The
//! evidence, the claims and the facts they rest on live in the workspace
//! and in the snapshot beside a rendered document; the render target is
//! the most replaceable thing here.
//!
//! # What this crate writes, executes and reaches
//!
//! Stated per command, because a single sentence about the whole crate
//! was wrong for months: it claimed nothing was written outside `record`'s
//! evidence log, which was true when tetel was only a checker and false
//! from the first authoring command onward.
//!
//! - **`check`, `brief`, `query`, `review`, `workspaces` write nothing.**
//!   They read the document, its ledger and its snapshot, and return.
//! - **`look`, `run`, `fact`, `claim`, `target`, `transplant`, `prose` write
//!   only inside the workspace state directory** (`$TETEL_STATE_HOME` or
//!   `~/.local/state/tetel`), never inside a repository — see
//!   [`workspace`] for why that separation is load-bearing.
//! - **`render --out` writes two things**: the named document, and the
//!   snapshot directory beside it.
//! - **`record` appends** one line to `<memo>.evidence.jsonl`, never
//!   rewriting an existing one.
//! - **Any command may append to its workspace's `refusals.log`** when it
//!   refuses, which is the point of that log.
//!
//! Two properties hold across all of them, and both are deliberate:
//! **nothing here ever executes a command named by a document** — a memo
//! arriving in a pull request must not be able to run code on whoever
//! checks it — and **there are no network calls anywhere**, which the
//! dependency set enforces rather than the prose. `run` executes the
//! command the *author* types, in the author's own session, and is the
//! only process this crate spawns besides `git` for a tree marker.

pub mod brief;
pub mod buildid;
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
pub mod targets;
pub mod transplants;
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
    report::render(display_path, &doc, &findings, &buildid::label())
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
    findings.out_of_proof = stale;
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
            // Which working trees these facts saw, and in how many states.
            // Read from the same shipped `facts.jsonl`; nothing derived is
            // stored, and a re-run recomputes it.
            let trees = worldstate::tree_report(&facts);
            findings.tree_states = trees.divergent;
            findings.tree_ungradable = trees.ungradable_facts;
        }
        // Modification targets, re-verified against the record rather
        // than trusted because the document says so. `target` refuses an
        // uncensused declaration at authoring time, so this can only fire
        // on a document that never passed through that verb.
        if let (Ok(targets), Ok(facts)) =
            (targets::load_all(&snapshot_dir), facts::load_all(&snapshot_dir))
        {
            findings.uncensused_targets = checks::census_findings(&doc.body, &targets, &facts);
        }
        // The premise inventory, re-verified the same way and for the
        // same reason: the verb refuses a premise that is not the donor's
        // words and `render --out` refuses an unanswered one, so anything
        // found here is a document that never passed through either.
        if let (Ok(transplants), Ok(facts), Ok(claim_list)) = (
            transplants::load_all(&snapshot_dir),
            facts::load_all(&snapshot_dir),
            claims::load_all(&snapshot_dir),
        ) {
            findings.unquoted_premises =
                checks::premise_findings(&doc.body, &transplants, &facts, &claim_list);
        }
        // The refusals recorded in each fact's mint window, recovered
        // from the two files the snapshot already ships: `facts.jsonl`'s
        // Create timestamps and `refusals.log`. This is the half of
        // TET-32 that does not depend on the author having read a line
        // at the terminal.
        findings.mint_windows = facts::mint_windows(&snapshot_dir);
        // Prose blocks whose text (or citations) postdate the settling
        // of what they cite. Read `prose.jsonl` directly rather than via
        // `prose::load_all`, which discards every timestamp — see
        // `checks::prose_after_proof`'s doc comment. `ledger.claims` is
        // deliberately used here, not the snapshot's `claims.jsonl`: it
        // is the same claim list already handed to `analyze_ledger`
        // above, imported from the rendered document, which is what
        // every `proposition_digest` on record was actually computed
        // over.
        // `if let Ok(...)` here — silently skipping a `prose.jsonl` that
        // exists but fails to parse — is *not* the silent-drop shape
        // `checks::prose_after_proof`'s own doc comment warns against
        // (that one is about a successful read over the wrong claims
        // source producing wrong digests with a clean exit). This is
        // the same file `snapshot::check` already read moments ago, at
        // the top of this function, via the identical
        // `workspace::read_jsonl::<ProseEvent>` call inside
        // `compose::render` → `prose::load_all` (same path, same target
        // type). A `prose.jsonl` that fails to parse here therefore
        // already failed that earlier read too, and `findings.provenance`
        // is already `Unreadable`, which `machine_check_failed` already
        // reddens under `[provenance-drift]` — loudly, before this block
        // runs. This `Err` arm is reachable only alongside a failure
        // already reported elsewhere, never on its own; see
        // `check_a_corrupted_prose_jsonl_reddens_via_provenance_drift_not_silently`
        // in tests/authoring_cli.rs, which pins that (it needs a real
        // rendered snapshot to corrupt, hence authoring_cli rather than
        // check_cli's static fixtures).
        if let Ok(prose_events) =
            workspace::read_jsonl::<prose::ProseEvent>(&snapshot_dir.join("prose.jsonl"))
        {
            let mut listed = checks::prose_after_proof(&prose_events, &ledger.claims, &evidence_records);
            // The line each block's text begins on in the rendered
            // document — from the one function that describes the
            // block-to-line correspondence, so this can't drift from
            // what `render` actually wrote.
            if let Ok(offsets) = compose::block_offsets(&snapshot_dir) {
                for item in &mut listed {
                    item.line = offsets.get(&item.block_id).copied();
                }
            }
            findings.prose_revised_since_proof = listed;
        }
    }
    // A rendered target row with no snapshot behind it cannot be graded
    // either way, so it is reported rather than failed — the same standing
    // choice every other snapshot-dependent finding makes.
    if !snapshot_dir.is_dir() {
        findings.unverifiable_targets = checks::rendered_targets(&doc.body);
        findings.unverifiable_transplants = checks::rendered_transplants(&doc.body);
    }
    // A document with no citations owes no snapshot; only one that points
    // at workspace-relative ids has a record to be missing.
    findings.cites_something = !citations::scan_citations(&doc.body).is_empty();

    Ok(report::render(&path.display().to_string(), &doc, &findings, &buildid::label()))
}

/// Runs `brief` against a file on disk: every claim in its evidence
/// ledger, id and proposition only, scope withheld. Returns `(exit_code,
/// output)`; a memo with no evidence ledger at all uses [`EXIT_NO_ROWS`].
/// `floor` is the confirmation floor the owed list is computed against —
/// see [`brief::DEFAULT_FLOOR`]. A floor of zero is refused by both front
/// ends before reaching here, because it would reproduce the empty owed
/// section a switched-off flag would give, and the decision not to have a
/// flag depends on the parameter not becoming one.
pub fn brief_file(path: &Path, json: bool, floor: u32) -> std::io::Result<(i32, String)> {
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
    // The owed list is computed here rather than inside `brief::build`,
    // which is handed claims and nothing else — a confidentiality device
    // enforced by shape rather than by care at render time, and the reason
    // `BriefItem` grew no field for this. Everything the predicate reads is
    // memo-derived: the claims from the document, the records from the
    // evidence file beside it, the authoring identity from the snapshot
    // beside that. No authoring workspace is opened, which is what makes an
    // owed list a viable surface for a pass given a path and nothing else.
    let (evidence_records, _) = evidence::load(path)?;
    let snapshot_dir = snapshot::snapshot_path(path);
    let authoring_identity = match workspace::identity_of(&snapshot_dir) {
        Some(id) => checks::AuthoringIdentity::Known(id),
        None if snapshot_dir.is_dir() => checks::AuthoringIdentity::SnapshotWithoutIdentity,
        None => checks::AuthoringIdentity::NoSnapshot,
    };
    let owed = checks::owed_claims(&ledger.claims, &evidence_records, &authoring_identity, floor);

    let items = brief::build(&ledger.claims);
    out.push_str(&if json {
        brief::render_json(&items, &owed, floor)
    } else {
        brief::render_text(&display_path, &items, &owed, floor)
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
