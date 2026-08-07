//! Runs the five reddening checks plus the report-only observations
//! (cited-but-undefined, defined-but-uncited) against a parsed [`Document`].
//!
//! Check 5 (the dependency-propagation cascade) shares its citation
//! syntax with check 4 but not its scope: check 4 owns direct, hop-1
//! prose citations of an unsettled row. Check 5 owns everything check 4
//! cannot see — row→row citation edges (found inside a row's own
//! `claim`/`note` fields, at any hop, including hop 1) and any citation,
//! prose or row, reached transitively at hop ≥ 2. Between them every
//! citation of an unsettled row is caught exactly once.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use crate::citations::{
    abutting_context, citation_ids_in, normalize_literal, scan_citations, AbuttingContext, Citation,
};
use crate::evidence::{EvidenceRecord, Source, Verdict};
use crate::ledger::Claim;
use crate::model::{Designator, Kind, Row, Status};
use crate::parse::Document;

/// Everything the report needs, already computed. Kept as plain owned
/// data (not references into `Document`) so it's simple to assert on in
/// tests and simple to render.
pub struct Findings {
    /// Formatted `line N: message`, sorted by line — check 1.
    pub grammar_errors: Vec<String>,
    /// Formatted failures — check 2, enumerated rows only.
    pub subset_failures: Vec<String>,
    /// Rows check 2 could not run on at all: `(id, claim)`.
    pub coverage_skipped: Vec<(String, String)>,
    /// Formatted failures — check 3.
    pub abutting_failures: Vec<String>,
    /// Formatted informational near-misses for check 3 (never fail).
    pub abutting_candidates: Vec<String>,
    /// Formatted failures — check 4.
    pub unsettled_failures: Vec<String>,
    /// Formatted failures — check 5. One entry per unsettled root that
    /// has at least one qualifying dependent, each entry a multi-line
    /// block naming every dependent under that root (grouped, not one
    /// finding per dependent — see the module doc comment).
    pub cascade_failures: Vec<String>,
    /// IDs cited in prose with no matching row.
    pub cited_undefined: Vec<String>,
    /// Rows never cited anywhere: `(id, claim)`.
    pub defined_uncited: Vec<(String, String)>,
    /// READING/OBSERVED/ATTESTED rows: `(id, "KIND/STATUS", claim)`.
    pub human_owed_rows: Vec<(String, String, String)>,
    /// IDs of every RUN row, for the class-level correspondence line.
    pub run_row_ids: Vec<String>,

    // --- the grounding brief/record/check slice ---------------------
    /// How many claims the memo's evidence ledger table(s) carried, valid
    /// or not — distinguishes "no ledger in this file" from "a ledger
    /// with zero rows" for the report's early-exit guard.
    pub ledger_claims_found: usize,
    /// Formatted `line N: message` — ledger rows that could not be
    /// parsed at all (wrong cell count, empty id, duplicate id). Never
    /// silently dropped.
    pub ledger_errors: Vec<String>,
    /// A ledger claim with no evidence record at all: `(id, proposition)`.
    /// Human-owed — absence of evidence is not itself a failure.
    pub ungrounded_claims: Vec<(String, String)>,
    /// A ledger claim grounded, but only by evidence that derives to
    /// [`Kind::Attested`] for standing — today, that's *every* grounded
    /// claim, since ingestion (`tetel record`) is the only write path and
    /// every ingested record derives to `Attested` regardless of what it
    /// claims (see `evidence::EvidenceRecord::derived_kind`). Distinct
    /// from `ungrounded_claims`: this is "someone looked, off-instrument",
    /// not "nobody looked". `(id, proposition)`, human-owed, never a
    /// failure — it stops being the whole list only once a witnessed
    /// grounding exists to sit beside ingested ones.
    pub attested_grounded_claims: Vec<(String, String)>,
    /// One line per evidence record whose `source` is a path that does not
    /// resolve on disk. Human-owed, never a failure — mirrors the
    /// non-failing disposition of `cited_undefined`, one line per record
    /// (not aggregated into a bracketed list) since each names a distinct
    /// claim and path.
    pub unresolved_evidence_sources: Vec<String>,
    /// A ledger claim whose Domain/Extent cells are exactly
    /// [`crate::ledger::NO_SCOPE_DECLARED`] — minted by `tetel claim` via
    /// `compose::render`, which has no scope field to draw from.
    /// Human-owed, never a failure: distinct from `coverage_skipped`
    /// (a *declared* domain/extent this crate's checks can't enumerate,
    /// e.g. a `proc:`/`external:` designator) — this is a domain/extent
    /// that was never declared at all, and it says so plainly rather than
    /// let a silently-passing check imply a coverage claim nobody made.
    pub no_scope_claims: Vec<(String, String)>,
    /// Formatted failures: two evidence records for one claim disagreeing
    /// on verdict, or a verdict contradicting the author's own Status
    /// cell. A machine failure — an unresolved contradiction, the same
    /// shape as the unsettled-citation check.
    pub verdict_disagreements: Vec<String>,
    /// Whether the memo still matches the workspace snapshot beside it.
    /// See [`crate::snapshot`] for why a rendered document is not
    /// self-contained and what a mismatch means.
    pub provenance: crate::snapshot::Provenance,
    /// Whether the memo cites anything at all. A document with no
    /// citations owes no snapshot, so a missing one is only worth
    /// reporting when something in the text points into a workspace.
    pub cites_something: bool,
    /// Facts whose note names a location their own captured extent does
    /// not cover. Human-owed: see [`crate::scope`] for why this is a
    /// scope check rather than a truth check, and why it never refuses.
    /// Only populated when a snapshot shipped beside the memo, since
    /// facts appear nowhere in the rendered document.
    pub notes_outside_extent: Vec<crate::scope::OutsideExtent>,
    /// Refusals recorded between one fact's mint and the previous one —
    /// what the author tried and could not do in the window that produced
    /// each fact. Human-owed and verbatim; a mint following a refusal is
    /// frequently correct. Like `notes_outside_extent`, only populated
    /// when a snapshot shipped beside the memo, and only as complete as
    /// the log inside it.
    pub mint_windows: Vec<crate::facts::MintWindow>,
    /// Working trees this memo's facts observed in more than one state,
    /// and the facts that saw each. A record rather than a warning — two
    /// facts seeing one repository in two states is the normal condition
    /// of any design that outlives an edit. What it settles is whether
    /// two facts asserting opposite things about the same code disagree
    /// about the world or about the same world, which before this the
    /// ledger could not answer at all. Same snapshot dependency as
    /// `mint_windows`.
    pub tree_states: Vec<crate::worldstate::RootStates>,
    /// Facts whose markers predate roots being recorded, so their tree
    /// state describes the process's working directory rather than what
    /// they read. Reported rather than passed over: an ungradable record
    /// is not a matching one.
    pub tree_ungradable: Vec<String>,
    /// True when the memo's ledger has no scope columns for any claim to
    /// declare into — a tetel-authored ledger. Reported once at document
    /// level rather than once per claim; see
    /// [`claims_without_declared_scope`] for why that changed.
    pub ledger_has_no_scope_columns: bool,
    /// One line per grounded claim describing how its evidence stands:
    /// witnessed (extent captured by this tool, in a named workspace) or
    /// ingested (extent typed by a caller). Human-owed, printed item by
    /// item, never a failure — see [`grounding_provenance`].
    pub grounding_provenance: Vec<String>,
    /// `(claim id, pass, note)` per record whose verdict is
    /// [`Verdict::Qualifies`]. Human-owed, never a failure.
    ///
    /// Until this existed, a qualifying verdict was invisible. On the
    /// first real independent grounding pass, the two claims the grounder
    /// declined to confirm — because the numbers they rest on could not
    /// be verified from what it was given — printed as plainly "grounded",
    /// while the one claim where it qualified a single premise of a
    /// multi-premise argument was the only thing reddening the report.
    /// The check fired on the wrong claim and was silent on the right
    /// ones.
    pub qualified_claims: Vec<(String, String, String)>,
    /// Evidence whose recorded proposition digest does not match the
    /// claim's current text — the proposition was revised after being
    /// graded. A machine failure; see [`analyze_ledger`].
    pub stale_evidence: Vec<String>,
    /// Records that graded an earlier wording of a claim which *has*
    /// since been re-grounded. Human-owed history, never a failure — see
    /// [`analyze_ledger`] on why the distinction matters.
    pub superseded_evidence: Vec<String>,
}

impl Findings {
    pub fn machine_check_failed(&self) -> bool {
        !self.grammar_errors.is_empty()
            || !self.subset_failures.is_empty()
            || !self.abutting_failures.is_empty()
            || !self.unsettled_failures.is_empty()
            || !self.cascade_failures.is_empty()
            || !self.ledger_errors.is_empty()
            || !self.verdict_disagreements.is_empty()
            || !self.stale_evidence.is_empty()
            || self.provenance_failed()
    }

    /// Drift and an unreadable snapshot are machine failures: both are
    /// objective contradictions between a document and the record it
    /// claims to rest on, decidable without a human.
    ///
    /// A *missing* snapshot is deliberately not a failure. Every memo
    /// authored before `render --out` existed lacks one, and turning that
    /// into a hard failure would grade the tooling's own history rather
    /// than the document. It is reported as human-owed instead.
    pub fn provenance_failed(&self) -> bool {
        matches!(
            self.provenance,
            crate::snapshot::Provenance::Drifted { .. }
                | crate::snapshot::Provenance::Unreadable(_)
        )
    }
}

fn covers(extent: &[Designator], d: &Designator) -> bool {
    extent.iter().any(|e| match (e, d) {
        (Designator::Path(ep), Designator::Path(dp)) => ep == dp,
        (Designator::Path(ep), Designator::Symbol { path: dp, .. }) => ep == dp,
        (
            Designator::Symbol { path: ep, symbol: es },
            Designator::Symbol { path: dp, symbol: ds },
        ) => ep == dp && es == ds,
        _ => false,
    })
}

fn designator_display(d: &Designator) -> String {
    match d {
        Designator::Symbol { path, symbol } => format!("{path}#{symbol}"),
        Designator::Path(p) => p.clone(),
        Designator::Proc(c) => format!("proc: {c}"),
        Designator::External(t) => format!("external: {t}"),
    }
}

/// `ledger_claims` is the memo's own evidence ledger (see
/// [`crate::ledger::import`]), passed in so that a citation of a ledger
/// claim id is recognised as defined — a ledger claim is never a fenced
/// row, so `rows_by_id` alone used to be blind to it entirely (every
/// citation of one reported `cited but undefined`, on every memo
/// `render` produces, since its own evidence ledger is the only thing
/// its prose ever cites). Fixed here, not by weakening the check: a
/// citation resolving to neither a row nor a ledger claim is still
/// reported. See the `ledger_by_id` map built below for which checks a
/// ledger-claim citation does and does not participate in.
pub fn analyze(doc: &Document, ledger_claims: &[Claim]) -> Findings {
    let mut grammar_errors: Vec<(usize, String)> = doc
        .grammar_errors
        .iter()
        .map(|e| (e.line, e.message.clone()))
        .collect();

    let rows_by_id: HashMap<&str, &Row> = doc.rows.iter().map(|r| (r.id.as_str(), r)).collect();
    // A ledger claim is a citable id too — see the doc comment above on
    // why `rows_by_id` alone can't answer "is this id defined": a ledger
    // claim has no `Row` to sit in that map. This index exists only to
    // settle "is the id defined at all" (fed into the `None` arm below,
    // and into the row→row edge scan for check 5); it is deliberately
    // never consulted by check 3 (abutting literal, needs a row's
    // `value`) or check 4 (unsettled citation, needs a row's typed
    // `Status`) — a ledger claim has no equivalent of either field, and
    // inventing one (e.g. parsing `Status` prose for VERIFIED/REFUTED,
    // the way `author_status_verdict` does for a different purpose) would
    // fabricate a value this crate has no basis for. See the report for
    // which checks were affected and why each was left to not apply.
    let ledger_by_id: HashMap<&str, &Claim> = ledger_claims.iter().map(|c| (c.id.as_str(), c)).collect();
    // A third place an id can be defined: the rendered facts table.
    // Prose can cite a fact directly, and before the document carried
    // one, every such citation reported as undefined — the renderer
    // emitting an id the checker had no table to resolve it in.
    let fact_ids = crate::ledger::facts_table_ids(&doc.body);

    // Check 2 — domain ⊆ extent, enumerated rows only.
    let mut subset_failures = Vec::new();
    let mut coverage_skipped = Vec::new();
    for row in &doc.rows {
        if !row.fully_enumerated() {
            coverage_skipped.push((row.id.clone(), row.claim.clone()));
            continue;
        }
        let uncovered: Vec<String> = row
            .domain
            .iter()
            .filter(|d| !covers(&row.extent, d))
            .map(designator_display)
            .collect();
        if !uncovered.is_empty() {
            subset_failures.push(format!(
                "line {}: {} — domain not covered by extent: {}",
                row.line,
                row.id,
                uncovered.join(", ")
            ));
        }
    }

    // Citations drive checks 3 and 4, plus the two report-only lists.
    let citations = scan_citations(&doc.body);
    let mut cited_ids: HashSet<String> = HashSet::new();
    let mut cited_undefined = Vec::new();
    let mut unsettled_failures = Vec::new();
    let mut abutting_failures = Vec::new();
    let mut abutting_candidates = Vec::new();

    for cit in &citations {
        cited_ids.insert(cit.id.clone());
        match rows_by_id.get(cit.id.as_str()) {
            None => {
                // Defined by the ledger instead of a fenced row: the
                // citation resolves, so it is not cited-but-undefined —
                // but checks 3 and 4 have nothing to run against it (see
                // `ledger_by_id`'s doc comment above), so nothing further
                // happens for this arm either way.
                if !ledger_by_id.contains_key(cit.id.as_str())
                    && !fact_ids.contains(&cit.id)
                    && !cited_undefined.contains(&cit.id)
                {
                    cited_undefined.push(cit.id.clone());
                }
            }
            Some(row) => {
                // Check 4 — unmarked citation of an unsettled row; a stance
                // marker on an already-VERIFIED row is a grammar error.
                if cit.stance_refuted {
                    if row.status == Status::Verified {
                        grammar_errors.push((
                            cit.line,
                            format!(
                                "[!{}] marks a VERIFIED row as refuted — the stance marker on a VERIFIED row is itself a grammar error",
                                cit.id
                            ),
                        ));
                    }
                } else if row.status.is_unsettled() {
                    unsettled_failures.push(format!(
                        "line {}: bare citation [{}] of {} row {} — needs the [!{}] stance marker, or the row must be settled",
                        cit.line, cit.id, row.status, cit.id, cit.id
                    ));
                }

                // Check 3 — abutting-literal mismatch.
                match abutting_context(&doc.body[cit.line - 1], cit.col) {
                    AbuttingContext::Abutting(tok) => {
                        if let Some(value) = &row.value {
                            let norm = normalize_literal(&tok);
                            let value_trimmed = value.trim();
                            let value_norm = normalize_literal(value_trimmed);
                            // A prose literal matches either the row's
                            // whole value or just its trailing word —
                            // multi-token values ("code 137", "exit 0")
                            // are cited by their own last word, and this
                            // is a no-op for a value that's already a
                            // single token.
                            let value_tail = value_trimmed
                                .rsplit(char::is_whitespace)
                                .next()
                                .unwrap_or(value_trimmed);
                            let value_tail_norm = normalize_literal(value_tail);
                            if norm != value_norm && norm != value_tail_norm {
                                let marker = if cit.stance_refuted { "!" } else { "" };
                                abutting_failures.push(format!(
                                    "line {}: literal '{}' abuts [{}{}] but row {}'s value is '{}'",
                                    cit.line, norm, marker, cit.id, cit.id, value
                                ));
                            }
                        }
                    }
                    AbuttingContext::Candidate(tok) => {
                        let marker = if cit.stance_refuted { "!" } else { "" };
                        abutting_candidates.push(format!(
                            "line {}: '{}' is near citation [{}{}] but not at abutting distance — printed candidate, never a failure",
                            cit.line, normalize_literal(&tok), marker, cit.id
                        ));
                    }
                    AbuttingContext::None => {}
                }
            }
        }
    }

    // Check 5 — dependency-propagation cascade. Two edge sources feed a
    // shared graph: prose→row (`citations`, already scanned above) and
    // row→row (a row's own `claim`/`note` fields, scanned here — the
    // load-bearing new source). A self-citation is never a dependency
    // edge. A citation of an id with no matching row is folded into
    // `cited_undefined`, exactly like an undefined prose citation — same
    // non-failing disposition, same reason: nothing here validates a
    // free-text field's content beyond field-name well-formedness, and
    // treating this case differently from the identical prose case would
    // need its own justification this slice doesn't have. See the report
    // for the fuller argument.
    let mut row_citers: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    for row in &doc.rows {
        let mut seen: HashSet<String> = HashSet::new();
        let fields = std::iter::once(row.claim.as_str()).chain(row.note.as_deref());
        for field_text in fields {
            for (id, _stance) in citation_ids_in(field_text) {
                if id == row.id {
                    continue; // self-citation is not a dependency
                }
                if rows_by_id.contains_key(id.as_str()) {
                    cited_ids.insert(id.clone());
                    if seen.insert(id.clone()) {
                        row_citers
                            .entry(id.clone())
                            .or_default()
                            .push((row.id.clone(), row.line));
                    }
                } else if ledger_by_id.contains_key(id.as_str()) {
                    // Defined by the ledger, not a row: it cannot be a
                    // cascade root (no typed `Status`) or a row→row
                    // dependent (it isn't a `Row`), so it never enters
                    // `row_citers` — but it is not cited-but-undefined.
                    cited_ids.insert(id.clone());
                } else {
                    cited_ids.insert(id.clone());
                    if !cited_undefined.contains(&id) {
                        cited_undefined.push(id.clone());
                    }
                }
            }
        }
    }

    // Reverse index: which prose citations name a given row id — reused
    // from the citations already scanned for checks 3 and 4.
    let mut prose_citers: HashMap<&str, Vec<&Citation>> = HashMap::new();
    for cit in &citations {
        prose_citers.entry(cit.id.as_str()).or_default().push(cit);
    }

    // BFS, one root at a time, over the reverse graph (who cites this
    // id) starting at each unsettled row. `visited` seeds with the root
    // itself so a citation cycle back to the root terminates instead of
    // looping, and so the root is never reported as its own dependent.
    // Row hits are kept at every hop (row→row edges are never covered by
    // check 4, at any distance); prose hits only ever arrive at hop ≥ 2
    // here, because a root's own direct prose citers — hop 1 — are
    // deliberately never looked up: that's exactly what check 4 already
    // reports, so this loop must not rediscover it.
    let mut cascade_failures = Vec::new();
    for root in doc.rows.iter().filter(|r| r.status.is_unsettled()) {
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(root.id.clone());
        let mut queue: VecDeque<(String, usize, String)> = VecDeque::new();
        for (citer_id, _line) in row_citers.get(&root.id).cloned().unwrap_or_default() {
            if visited.insert(citer_id.clone()) {
                queue.push_back((citer_id, 1, root.id.clone()));
            }
        }

        let mut lines: Vec<(usize, String)> = Vec::new();
        while let Some((id, hop, cites)) = queue.pop_front() {
            let line = rows_by_id.get(id.as_str()).map(|r| r.line).unwrap_or(0);
            lines.push((hop, format!("hop {hop} [row] {id} (line {line}): cites {cites}")));

            if let Some(citers) = prose_citers.get(id.as_str()) {
                for cit in citers {
                    lines.push((
                        hop + 1,
                        format!("hop {} [prose] line {}: cites {}", hop + 1, cit.line, id),
                    ));
                }
            }
            if let Some(further) = row_citers.get(&id) {
                for (next_id, _next_line) in further {
                    if visited.insert(next_id.clone()) {
                        queue.push_back((next_id.clone(), hop + 1, id.clone()));
                    }
                }
            }
        }

        if lines.is_empty() {
            continue;
        }
        lines.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let dependents = lines.len();
        let body = lines
            .iter()
            .map(|(_, s)| format!("      {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        cascade_failures.push(format!(
            "root {} ({}) — {} dependent{}:\n{}",
            root.id,
            root.status,
            dependents,
            if dependents == 1 { "" } else { "s" },
            body
        ));
    }

    let mut defined_uncited = Vec::new();
    let mut human_owed_rows = Vec::new();
    let mut run_row_ids = Vec::new();
    for row in &doc.rows {
        if !cited_ids.contains(&row.id) {
            defined_uncited.push((row.id.clone(), row.claim.clone()));
        }
        match row.kind {
            Kind::Run => run_row_ids.push(row.id.clone()),
            Kind::Reading | Kind::Observed | Kind::Attested => {
                human_owed_rows.push((
                    row.id.clone(),
                    format!("{}/{}", row.kind, row.status),
                    row.claim.clone(),
                ));
            }
        }
    }
    // The same inverse for ledger claims: "an uncited claim prints,
    // default disposition delete" is a deliberate rule (see report.rs),
    // and it must hold for a ledger claim exactly as it does for a
    // fenced row — fixing citation resolution to see ledger claims
    // cannot also make an uncited one invisible in the other direction.
    for claim in ledger_claims {
        if !cited_ids.contains(&claim.id) {
            defined_uncited.push((claim.id.clone(), claim.proposition.clone()));
        }
    }

    grammar_errors.sort_by_key(|(line, _)| *line);
    let grammar_errors = grammar_errors
        .into_iter()
        .map(|(line, msg)| format!("line {line}: {msg}"))
        .collect();

    Findings {
        grammar_errors,
        subset_failures,
        coverage_skipped,
        abutting_failures,
        abutting_candidates,
        unsettled_failures,
        cascade_failures,
        cited_undefined,
        defined_uncited,
        human_owed_rows,
        run_row_ids,
        ledger_claims_found: 0,
        ledger_errors: Vec::new(),
        ungrounded_claims: Vec::new(),
        attested_grounded_claims: Vec::new(),
        unresolved_evidence_sources: Vec::new(),
        no_scope_claims: Vec::new(),
        verdict_disagreements: Vec::new(),
        // `analyze` grades a parsed document in isolation and never
        // touches the filesystem; provenance needs the memo's path, so
        // `check_file` fills both in.
        provenance: crate::snapshot::Provenance::Missing,
        cites_something: false,
        notes_outside_extent: Vec::new(),
        tree_states: Vec::new(),
        tree_ungradable: Vec::new(),
        mint_windows: Vec::new(),
        ledger_has_no_scope_columns: false,
        grounding_provenance: Vec::new(),
        qualified_claims: Vec::new(),
        stale_evidence: Vec::new(),
        superseded_evidence: Vec::new(),
    }
}

/// The author's own verdict, read off a ledger `Status` cell's leading
/// bolded keyword — high-confidence only. A `Status` cell is free prose
/// ("**DISCHARGED BY A RUN, and narrowed.**", "**CITED, NOT
/// RE-OBSERVED.**"...), and guessing a verdict out of every shape it can
/// take would mean fabricating disagreements the author never stated.
/// This only ever fires on the two unambiguous keywords the corpus's own
/// `tetel` row grammar already treats as opposites (`VERIFIED`/
/// `REFUTED`); anything else — including `OWED`-shaped or qualified
/// prose — is left unclassified rather than guessed at, mirroring
/// citations.rs's own Abutting-vs-Candidate confidence split.
fn author_status_verdict(status_cell: &str) -> Option<Verdict> {
    let verified = status_cell.find("VERIFIED");
    let refuted = status_cell.find("REFUTED");
    match (verified, refuted) {
        (Some(v), Some(r)) => Some(if v < r { Verdict::Supports } else { Verdict::Refutes }),
        (Some(_), None) => Some(Verdict::Supports),
        (None, Some(_)) => Some(Verdict::Refutes),
        (None, None) => None,
    }
}

/// `(ungrounded, attested_grounded, verdict_disagreements, qualified)`.
///
/// `qualified` is `(claim id, pass, note)` per qualifying record —
/// human-owed, never a failure. A grounder that reports "true only under
/// a condition this proposition does not state", or "I could not
/// establish this from what I was given", has produced the most
/// load-bearing sentence in its whole pass; it belongs in front of a
/// reader, not buried in a file.
type LedgerFindings = (
    Vec<(String, String)>,
    Vec<(String, String)>,
    Vec<String>,
    Vec<(String, String, String)>,
    Vec<String>,
    Vec<String>,
);

/// The checks this slice and the grounding-provenance slice on top of it
/// add, run independently of the five `tetel`-row checks above: a claim
/// with no evidence record at all (human-owed — absence isn't a failure);
/// a claim grounded only by evidence that derives to `Attested` standing
/// (human-owed, and distinct from the first — someone looked,
/// off-instrument); and two verdicts that contradict each other, whether
/// that's two grounding passes disagreeing or a pass contradicting the
/// author's own `Status` cell (a machine failure — an unresolved
/// contradiction).
pub fn analyze_ledger(claims: &[Claim], evidence: &[EvidenceRecord]) -> LedgerFindings {
    let mut ungrounded = Vec::new();
    let mut attested_grounded = Vec::new();
    let mut disagreements = Vec::new();
    let mut qualified = Vec::new();
    let mut stale_digests = Vec::new();
    let mut superseded = Vec::new();

    for claim in claims {
        let records: Vec<&EvidenceRecord> = evidence.iter().filter(|e| e.claim_id == claim.id).collect();

        // A record grades a *proposition*, not an id. The digest of the
        // text it graded is written into every record; comparing it to
        // the claim's current text is what stops evidence transferring to
        // a rewritten claim.
        //
        // Without this, revising a proposition through the ordinary
        // `claim --revise` + `render --out` path left its old evidence
        // attached — including to the exact negation of what was graded,
        // with the memo still matching its snapshot, no drift, and the
        // machine partition clean.
        //
        // **Whether a stale record fails depends on what sits beside it.**
        // The first version of this check failed on any stale record, and
        // that was undischargeable: the evidence log is append-only with
        // no supersession, so re-grounding — the remedy this very check
        // printed — added a current record and left the failure standing.
        // A red nobody can clear is the defect this crate rejected in the
        // verdict-disagreement check hours earlier, rebuilt here.
        //
        // So: a claim with no record matching its current text is a
        // machine failure, because nothing grades what it now says. A
        // claim that has one is grounded, and its superseded records are
        // history — worth printing, never a failure. Re-grounding is now
        // a remedy that works.
        let current = crate::evidence::sha256_hex(&claim.proposition);
        let (fresh, stale): (Vec<&&EvidenceRecord>, Vec<&&EvidenceRecord>) = records
            .iter()
            .filter(|r| !r.proposition_digest.is_empty())
            .partition(|r| r.proposition_digest == current);

        for r in &stale {
            let line = format!(
                "{} — evidence from pass {} graded different text than this claim now carries \
(recorded digest {}…, current {}…).\n      that pass said: {} — {}",
                claim.id,
                r.pass,
                &r.proposition_digest[..r.proposition_digest.len().min(12)],
                &current[..current.len().min(12)],
                r.verdict,
                r.note.as_deref().unwrap_or("(no note)"),
            );
            if fresh.is_empty() {
                stale_digests.push(format!(
                    "{line}\n      Nothing grades what this claim says today. Re-ground it \
against the current wording, or restore the wording it was graded against."
                ));
            } else {
                superseded.push(format!(
                    "{line}\n      Superseded: {} record(s) grade the current wording. Kept \
because it is what an earlier pass actually found, and the log is append-only.",
                    fresh.len()
                ));
            }
        }

        if records.is_empty() {
            ungrounded.push((claim.id.clone(), claim.proposition.clone()));
            continue;
        }

        // Every check below asks what the evidence says about *what this
        // claim says now*, so every one of them reads this set rather
        // than `records`.
        //
        // Making only the stale/superseded partition digest-aware and
        // leaving the verdict checks comparing all records was the same
        // defect one layer down, and it punished exactly the loop this
        // crate exists to produce: a pass refutes a claim, the author
        // fixes the wording, a later pass grounds the new wording
        // unanimously — and `verdict-disagreement` still fired, forever,
        // on a supports/refutes pair against text that no longer exists.
        // The only escape was withdrawing the claim and re-issuing it
        // under a fresh id, which erases the refutation from the rendered
        // ledger — the one trail the loop exists to preserve. An
        // undischargeable red that can only be cleared by destroying
        // evidence is worse than the one it replaced.
        //
        // A record with no digest at all predates the field. It is
        // treated as grading the current text rather than dropped:
        // dropping it would silently retire these checks for every
        // document written before digests existed, turning a live
        // contradiction into a clean report. Grandfathering can at worst
        // report a disagreement that a revision has since resolved —
        // which is the direction that fails loudly.
        let grading_current: Vec<&EvidenceRecord> = records
            .iter()
            .copied()
            .filter(|r| r.proposition_digest.is_empty() || r.proposition_digest == current)
            .collect();

        // Distinct from "ungrounded": at least one record grades the
        // current text, and every one that does derives to `Attested` for
        // standing purposes. Today that's unconditionally true — ingestion
        // is the only write path, and `derived_kind` never returns
        // anything else for an ingested record — but the check is written
        // against `derived_kind`, not against "has any evidence", so it
        // keeps working once a witnessed grounding can also land here.
        //
        // The emptiness guard is not redundant: a claim whose records all
        // grade superseded text has already been reported as a machine
        // failure by the partition above, and `all` over nothing is true,
        // so without it that claim would also be announced as grounded.
        if !grading_current.is_empty() && grading_current.iter().all(|r| r.derived_kind() == Kind::Attested) {
            attested_grounded.push((claim.id.clone(), claim.proposition.clone()));
        }

        // A contradiction is `supports` *and* `refutes` on one
        // proposition — P and not-P, decidable without a human, which is
        // what belongs in the machine partition.
        //
        // Two things this deliberately no longer does.
        //
        // It no longer treats `qualifies` as contradicting anything.
        // `qualifies` means "true under a condition the proposition does
        // not state" or "I could not establish this from what I was
        // given"; neither is inconsistent with another pass finding the
        // proposition supported. The author-vs-record comparison below
        // has always taken that view — only Supports/Refutes contradict —
        // and the record-vs-record loop disagreeing with it was the
        // inconsistency, not the doctrine. It also made an honest
        // multi-fact grounding unwritable in effect: `record --from-fact`
        // takes one fact per record, so a claim resting on several facts
        // accrues several same-pass records, and a grounder qualifying
        // one premise of a multi-premise argument reddened the memo for
        // being precise. Worse, the red was undischargeable — the
        // evidence log is append-only with no supersession, so nothing
        // the author could do afterwards would clear it.
        //
        // And it compares the whole set rather than adjacent pairs. The
        // old `windows(2)` loop compared (1,2) and (2,3) but never (1,3),
        // so records ordered `supports, qualifies, refutes` reported two
        // findings about the qualifies pairs and never mentioned the one
        // real contradiction. Order-dependent, and wrong in the direction
        // that hides the failure.
        //
        // Pass identity is deliberately not a condition. For an ingested
        // record `pass` is caller-typed free text, so resting a machine
        // verdict on passes differing would put a contradiction behind an
        // unverifiable field — and same-pass Supports+Refutes on one
        // proposition is a formal contradiction regardless of who wrote
        // both.
        let supporting: Vec<&&EvidenceRecord> =
            grading_current.iter().filter(|r| r.verdict == Verdict::Supports).collect();
        let refuting: Vec<&&EvidenceRecord> =
            grading_current.iter().filter(|r| r.verdict == Verdict::Refutes).collect();
        if !supporting.is_empty() && !refuting.is_empty() {
            let side = |rs: &[&&EvidenceRecord]| -> String {
                rs.iter()
                    .map(|r| {
                        format!(
                            "\n      pass {} => {} — {}",
                            r.pass,
                            r.verdict,
                            r.note.as_deref().unwrap_or("(no note)")
                        )
                    })
                    .collect()
            };
            disagreements.push(format!(
                "{} — {}{}{}",
                claim.id,
                claim.proposition,
                side(&supporting),
                side(&refuting),
            ));
        }

        // A qualified claim is human-owed, and until now it was silent.
        // On the first real grounding pass, the two claims the grounder
        // explicitly declined to confirm read as plainly "grounded", with
        // nothing anywhere in the report saying so — while the one claim
        // where it qualified a single premise of a multi-premise argument
        // was the one that reddened. The check fired on the wrong claim
        // and said nothing about the right ones.
        //
        // Digest-scoped for the same reason the machine half is: folding
        // a qualification into the claim's wording and re-grounding is
        // how an author *discharges* one, and a line that survived it
        // would leave the human partition accumulating exactly the
        // undischargeable residue the machine partition just shed. The
        // record itself still prints, under superseded evidence.
        for r in grading_current.iter().filter(|r| r.verdict == Verdict::Qualifies) {
            qualified.push((
                claim.id.clone(),
                r.pass.clone(),
                r.note.clone().unwrap_or_else(|| "(no note)".to_string()),
            ));
        }

        if let Some(author_verdict) = author_status_verdict(&claim.status) {
            for record in &grading_current {
                let contradicts = matches!(
                    (author_verdict, record.verdict),
                    (Verdict::Supports, Verdict::Refutes) | (Verdict::Refutes, Verdict::Supports)
                );
                if contradicts {
                    disagreements.push(format!(
                        "{} — {}\n      pass {} => {} — {}\n      pass {} => {} — {}",
                        claim.id,
                        claim.proposition,
                        "author (Status cell)",
                        author_verdict,
                        claim.status,
                        record.pass,
                        record.verdict,
                        record.note.as_deref().unwrap_or("(no note)"),
                    ));
                }
            }
        }
    }

    (ungrounded, attested_grounded, disagreements, qualified, stale_digests, superseded)
}

/// One line per evidence record whose `source` designator is a path that
/// does not resolve on disk. A `proc:` source is never checked here — it
/// names a transcript, not a file, so there is nothing on disk to resolve
/// and its absence is not this check's business.
///
/// Non-failing, human-owed, mirroring `cited_undefined`'s disposition:
/// fabricating an attested fact should require fabricating a preserved
/// artifact, but a *missing* artifact is residue for a human to chase, not
/// grounds to redden the run — the same reasoning that keeps an undefined
/// citation out of the machine-checked partition.
///
/// Resolved against the current working directory — the same resolution a
/// path given on the command line gets. There is no existing convention in
/// this crate for resolving a designator against the memo's own directory
/// instead; `domain`/`extent` designators are never resolved against the
/// filesystem at all.
pub fn unresolved_evidence_sources(claims: &[Claim], evidence: &[EvidenceRecord]) -> Vec<String> {
    let propositions: HashMap<&str, &str> = claims.iter().map(|c| (c.id.as_str(), c.proposition.as_str())).collect();
    let mut out = Vec::new();
    for record in evidence {
        if let Some(Source::Path(path)) = Source::parse(&record.source) {
            if !Path::new(&path).exists() {
                let proposition = propositions.get(record.claim_id.as_str()).copied().unwrap_or("");
                out.push(format!(
                    "{}: evidence source `{}` does not resolve — {}",
                    record.claim_id, path, proposition
                ));
            }
        }
    }
    out
}

/// Ledger claims minted with no declared scope at all (see
/// [`crate::ledger::NO_SCOPE_DECLARED`] and `compose::render`). Both cells
/// must match the sentinel exactly — an ordinary hand-written ledger row
/// whose Domain/Extent text happens to coincide is not this crate's
/// business to guess at, and `compose::render` always writes both cells
/// identically, so a real match never falls short of both.
/// Claims in a ledger that *has* scope columns but left them at the
/// no-scope sentinel.
///
/// Deliberately narrowed. This used to fire on every claim of every
/// tetel-authored memo, because `render` wrote the sentinel into two
/// columns unconditionally — eight identical lines on the first real
/// memo, saying the same structural fact about tetel rather than anything
/// about the document. A finding no author can ever discharge is not a
/// finding; it dilutes the human-owed list this design says should
/// *concentrate* reading.
///
/// A ledger with no scope columns at all now says so once, at document
/// level (see [`Findings::ledger_has_no_scope_columns`]). This function is
/// left for the case it was actually about: a table that offers the
/// columns and carries the sentinel anyway.
pub fn claims_without_declared_scope(claims: &[Claim]) -> Vec<(String, String)> {
    claims
        .iter()
        .filter(|c| {
            c.has_scope_columns
                && c.domain == crate::ledger::NO_SCOPE_DECLARED
                && c.extent == crate::ledger::NO_SCOPE_DECLARED
        })
        .map(|c| (c.id.clone(), c.proposition.clone()))
        .collect()
}

/// How each grounded claim's evidence stands, one line per claim.
///
/// # Why this is printed rather than checked
///
/// A grounding pass declares its own independence today: `pass` is a free
/// string in `record`'s JSON, validated only for being non-empty. So
/// "this claim was independently grounded" is an assertion nothing can
/// contradict, on a mechanism whose entire measured value (78% → 33%)
/// comes from independence being real.
///
/// A witnessed record fixes the checkable half. Its extent was copied
/// from a fact this tool captured, and it carries the identity of the
/// workspace that captured it — so whether a pass grounded claims in its
/// *own* observations is recomputable rather than claimed.
///
/// # What it still cannot establish, said out loud
///
/// That the grounding agent saw only the brief. Nothing in a record can
/// show that: it is a property of the sandbox the agent was handed, and
/// stays owed to a run protocol. This prints what it knows and names what
/// it does not, rather than letting a witnessed record read as a stronger
/// guarantee than it is.
/// What the snapshot beside a memo can say about who authored it.
///
/// The two negative cases are deliberately distinct. They were one
/// `None` and the report called both "no snapshot beside this memo",
/// which sent a reader looking for a missing directory that was in fact
/// present with six of its seven files — the message named a cause that
/// was not the cause. They also differ in what a reader should do: a
/// missing snapshot means the memo was never rendered by `render --out`,
/// while a snapshot without an identity means it was rendered by a build
/// that did not ship one, and cannot be repaired after the fact because
/// minting an identity now would date the pass wrongly.
pub enum AuthoringIdentity {
    /// No snapshot directory beside the memo at all.
    NoSnapshot,
    /// A snapshot exists but carries no readable `identity.json`.
    SnapshotWithoutIdentity,
    /// The identity of the workspace that authored the memo.
    Known(String),
}

pub fn grounding_provenance(
    claims: &[Claim],
    evidence: &[EvidenceRecord],
    authoring_identity: &AuthoringIdentity,
) -> Vec<String> {
    let mut out = Vec::new();
    for claim in claims {
        let records: Vec<&EvidenceRecord> =
            evidence.iter().filter(|e| e.claim_id == claim.id).collect();
        if records.is_empty() {
            continue;
        }
        let witnessed: Vec<&&EvidenceRecord> = records.iter().filter(|r| r.witnessed).collect();
        if witnessed.is_empty() {
            out.push(format!(
                "{}: grounded, all {} record(s) ingested — extent typed by the reporter, not captured by this tool; `pass` is whatever the reporter wrote",
                claim.id,
                records.len()
            ));
        } else {
            let passes: Vec<&str> = {
                let mut p: Vec<&str> = witnessed.iter().map(|r| r.pass.as_str()).collect();
                p.sort_unstable();
                p.dedup();
                p
            };
            // The distinction the whole mechanism exists for. A claim
            // grounded in the same workspace that authored it was graded
            // by the author against their own reading — the arrangement
            // measured at 78% scope-equal, no better than hand-authored
            // rows. Independence is what moved that to 33%. A check that
            // could not tell the two apart would be reporting the wrong
            // thing confidently, which is worse than reporting nothing.
            let standing = match authoring_identity {
                AuthoringIdentity::NoSnapshot => "no snapshot beside this memo, so whether the \
grounding workspace is also the authoring one cannot be determined from here"
                    .to_string(),
                AuthoringIdentity::SnapshotWithoutIdentity => "the snapshot beside this memo \
carries no identity, so whether the grounding workspace is also the authoring one cannot be \
determined from here — the memo was rendered by a build that did not ship one, and it cannot be \
added now without re-dating the pass"
                    .to_string(),
                AuthoringIdentity::Known(author) => {
                    // Counted separately rather than collapsed to a
                    // single verdict: a claim can carry both a
                    // self-grounded record and an independent one, and
                    // saying only "self-grounded" there would hide a real
                    // independent pass, while saying only "independently
                    // grounded" would hide that the author also graded
                    // their own work.
                    let by_author = passes.iter().filter(|p| *p == author).count();
                    let by_others = passes.len() - by_author;
                    match (by_author, by_others) {
                        (0, _) => "independently grounded: no grounding workspace here is the one \
that authored this memo"
                            .to_string(),
                        (_, 0) => "SELF-GROUNDED: the only workspace that graded this claim is the \
one that authored the memo, so no independent pass has run on it. The author's own reading is the \
arrangement measured at 78% scope-equal; independence is what moved that to 33%"
                            .to_string(),
                        (a, o) => format!(
                            "MIXED: {a} grounding workspace(s) authored this memo and {o} did not \
— read the self-grounded record(s) as the author's own reading, not as independent confirmation"
                        ),
                    }
                }
            };
            out.push(format!(
                "{}: grounded, {} of {} record(s) witnessed in workspace(s) {} — {}. That the \
grounding pass saw only the brief is not shown by any record and remains owed to the run protocol",
                claim.id,
                witnessed.len(),
                records.len(),
                passes.join(", "),
                standing
            ));
        }
    }
    out
}

/// Whether the memo's ledger is a tetel-authored one, whose format has no
/// scope columns for any claim to declare into.
pub fn ledger_has_no_scope_columns(claims: &[Claim]) -> bool {
    !claims.is_empty() && claims.iter().all(|c| !c.has_scope_columns)
}

#[cfg(test)]
mod tests {
    use crate::parse::parse_document;
    use crate::report::{render, EXIT_CHECK_FAILED, EXIT_CLEAN, EXIT_NO_ROWS};

    use super::*;

    fn check(source: &str) -> (i32, String, Findings) {
        let doc = parse_document(source);
        let ledger = crate::ledger::import(&doc.body);
        let findings = analyze(&doc, &ledger.claims);
        let (code, text) = render("test.md", &doc, &findings, "tetel test build");
        (code, text, findings)
    }

    #[test]
    fn no_tetel_blocks_is_a_distinct_exit_code() {
        let (code, out, _) = check("# just prose\n\nno rows here.\n");
        assert_eq!(code, EXIT_NO_ROWS);
        assert_ne!(code, EXIT_CLEAN);
        assert!(out.contains("no tetel rows found"));
    }

    #[test]
    fn duplicate_id_is_a_grammar_error() {
        let source = "\
```tetel
id: X-1
claim: First claim.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: READING
status: VERIFIED
```

```tetel
id: X-1
claim: Second claim reusing the same id.
domain: b.rs#g
extent: b.rs#g
pin: p1
kind: READING
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(code, EXIT_CHECK_FAILED);
        assert!(findings.grammar_errors.iter().any(|e| e.contains("duplicate id")));
    }

    #[test]
    fn value_forbidden_on_reading_is_a_grammar_error() {
        let source = "\
```tetel
id: X-2
claim: A reading row that wrongly carries a value.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: READING
value: 3
status: VERIFIED
```
";
        let (_, _, findings) = check(source);
        assert!(findings
            .grammar_errors
            .iter()
            .any(|e| e.contains("forbidden") && e.contains("value")));
    }

    #[test]
    fn stance_marker_on_verified_row_is_a_grammar_error() {
        let source = "\
Cited as refuted even though it is verified [!X-3].

```tetel
id: X-3
claim: A verified claim wrongly cited as refuted.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: READING
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(code, EXIT_CHECK_FAILED);
        assert!(findings
            .grammar_errors
            .iter()
            .any(|e| e.contains("VERIFIED") && e.contains("refuted")));
    }

    #[test]
    fn proc_designator_skips_subset_check_and_lands_in_coverage_skipped() {
        let source = "\
```tetel
id: X-4
claim: A proc-shaped row whose coverage is not machine-checked.
domain: proc: grep -rn Foo src
extent: proc: grep -rn Foo src
pin: p1
kind: READING
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(code, EXIT_CLEAN, "proc: rows must not fail the subset check");
        assert!(findings.subset_failures.is_empty());
        assert_eq!(findings.coverage_skipped.len(), 1);
        assert_eq!(findings.coverage_skipped[0].0, "X-4");
    }

    #[test]
    fn cited_but_undefined_id_is_reported_without_failing() {
        let source = "\
Refers to a row that was never defined [GHOST-1].

```tetel
id: X-5
claim: An unrelated defined row.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: READING
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(code, EXIT_CLEAN, "an undefined citation must not fail the run");
        assert_eq!(findings.cited_undefined, vec!["GHOST-1".to_string()]);
    }

    #[test]
    fn defined_but_uncited_id_is_reported_without_failing() {
        let source = "\
```tetel
id: X-6
claim: A row nobody ever cites.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: READING
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(code, EXIT_CLEAN);
        assert_eq!(findings.defined_uncited.len(), 1);
        assert_eq!(findings.defined_uncited[0].0, "X-6");
    }

    #[test]
    fn non_abutting_literal_is_a_candidate_never_a_failure() {
        let source = "\
The gateway answers at 29s, or thereabouts, according to [X-7].

```tetel
id: X-7
claim: The gateway's health endpoint responds within its SLA.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: OBSERVED
run: curl
value: 31s
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(
            code, EXIT_CLEAN,
            "a non-abutting literal must never fail, even when it disagrees with the value"
        );
        assert!(findings.abutting_failures.is_empty());
        assert_eq!(findings.abutting_candidates.len(), 1);
    }
}
