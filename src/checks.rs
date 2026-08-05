//! Runs the four reddening checks plus the report-only observations
//! (cited-but-undefined, defined-but-uncited) against a parsed [`Document`].

use std::collections::{HashMap, HashSet};

use crate::citations::{abutting_context, normalize_literal, scan_citations, AbuttingContext};
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
    /// IDs cited in prose with no matching row.
    pub cited_undefined: Vec<String>,
    /// Rows never cited anywhere: `(id, claim)`.
    pub defined_uncited: Vec<(String, String)>,
    /// READING/OBSERVED/ATTESTED rows: `(id, "KIND/STATUS", claim)`.
    pub human_owed_rows: Vec<(String, String, String)>,
    /// IDs of every RUN row, for the class-level correspondence line.
    pub run_row_ids: Vec<String>,
}

impl Findings {
    pub fn machine_check_failed(&self) -> bool {
        !self.grammar_errors.is_empty()
            || !self.subset_failures.is_empty()
            || !self.abutting_failures.is_empty()
            || !self.unsettled_failures.is_empty()
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

pub fn analyze(doc: &Document) -> Findings {
    let mut grammar_errors: Vec<(usize, String)> = doc
        .grammar_errors
        .iter()
        .map(|e| (e.line, e.message.clone()))
        .collect();

    let rows_by_id: HashMap<&str, &Row> = doc.rows.iter().map(|r| (r.id.as_str(), r)).collect();

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
                if !cited_undefined.contains(&cit.id) {
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
        cited_undefined,
        defined_uncited,
        human_owed_rows,
        run_row_ids,
    }
}

#[cfg(test)]
mod tests {
    use crate::parse::parse_document;
    use crate::report::{render, EXIT_CHECK_FAILED, EXIT_CLEAN, EXIT_NO_ROWS};

    use super::*;

    fn check(source: &str) -> (i32, String, Findings) {
        let doc = parse_document(source);
        let findings = analyze(&doc);
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
