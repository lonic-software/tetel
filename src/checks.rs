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
                if !ledger_by_id.contains_key(cit.id.as_str()) && !cited_undefined.contains(&cit.id) {
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

/// `(ungrounded, attested_grounded, disagreements)` — see [`analyze_ledger`].
type LedgerFindings = (Vec<(String, String)>, Vec<(String, String)>, Vec<String>);

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

    for claim in claims {
        let records: Vec<&EvidenceRecord> = evidence.iter().filter(|e| e.claim_id == claim.id).collect();
        if records.is_empty() {
            ungrounded.push((claim.id.clone(), claim.proposition.clone()));
            continue;
        }

        // Distinct from "ungrounded": at least one record exists, and
        // every one of them derives to `Attested` for standing purposes.
        // Today that's unconditionally true — ingestion is the only write
        // path, and `derived_kind` never returns anything else for an
        // ingested record — but the check is written against
        // `derived_kind`, not against "has any evidence", so it keeps
        // working once a witnessed grounding can also land here.
        if records.iter().all(|r| r.derived_kind() == Kind::Attested) {
            attested_grounded.push((claim.id.clone(), claim.proposition.clone()));
        }

        for pair in records.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if a.verdict != b.verdict {
                disagreements.push(format!(
                    "{} — {}\n      pass {} => {} — {}\n      pass {} => {} — {}",
                    claim.id,
                    claim.proposition,
                    a.pass,
                    a.verdict,
                    a.note.as_deref().unwrap_or("(no note)"),
                    b.pass,
                    b.verdict,
                    b.note.as_deref().unwrap_or("(no note)"),
                ));
            }
        }

        if let Some(author_verdict) = author_status_verdict(&claim.status) {
            for record in &records {
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

    (ungrounded, attested_grounded, disagreements)
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
pub fn claims_without_declared_scope(claims: &[Claim]) -> Vec<(String, String)> {
    claims
        .iter()
        .filter(|c| c.domain == crate::ledger::NO_SCOPE_DECLARED && c.extent == crate::ledger::NO_SCOPE_DECLARED)
        .map(|c| (c.id.clone(), c.proposition.clone()))
        .collect()
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
        let (code, text) = render("test.md", &doc, &findings);
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
