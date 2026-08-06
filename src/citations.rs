//! Scans a document's body prose (fenced regions already blanked out by
//! `parse::parse_document`) for citations, and classifies what
//! immediately precedes each one for the abutting-literal check.
//!
//! # Two citation forms, one meaning
//!
//! Inline `[ID]` / `[!ID]` is the hand-authored form: it pins a citation
//! to a specific spot in a sentence, which is what the abutting-literal
//! check needs a column for.
//!
//! `*cites: C1, C4*` on a line of its own is what [`crate::compose`]
//! emits for a block authored through `tetel prose --cites`. It attributes
//! the whole preceding block rather than one phrase in it.
//!
//! Both must be scanned. Recognising only the bracket form was a real
//! defect: `render` emitted the trailer form, `check` could not read it,
//! and every claim a rendered document cited was reported "defined but
//! never cited — default disposition is delete". Following that advice on
//! the first document authored this way would have deleted a ledger whose
//! prose cited every row. This is the mirror of the same seam fixed in
//! "check: stop reporting a memo's own ledger claims as cited-but-
//! undefined" — the renderer and the checker must agree on the syntax, in
//! both directions.

/// One citation found in body prose — inline `[ID]`/`[!ID]`, or an id
/// within a `*cites: …*` trailer line.
#[derive(Debug, Clone)]
pub struct Citation {
    /// 1-based line number.
    pub line: usize,
    /// 0-based byte offset of the citation's `[` on that line.
    pub col: usize,
    pub id: String,
    /// True for `[!ID]` — "cited as refuted".
    pub stance_refuted: bool,
}

/// Whether a bracketed token has the shape of an id this system mints or
/// a corpus ledger uses: starts with an uppercase ASCII letter, and
/// contains a digit or a hyphen. `F5`, `C1`, `P1`, `S3-L17`, `E-4`,
/// `X-6`, `R-ROOT`, `CYC-A` all qualify.
///
/// The rule describes **what a tetel id looks like**, not what some
/// language's identifiers look like. That distinction matters: this tool
/// documents systems in any language, and a check tuned to one of them
/// would be wrong everywhere else.
///
/// Ids in this system are minted `F1`/`C1`/`P1`, or come from a corpus
/// ledger as `S3-L17`, `E-4`, `X-6`, `R-ROOT`, `CYC-A`. Uppercase-led,
/// with a digit or a hyphen. Everything else between brackets is content.
///
/// Without this, bracketed code in a note is read as a citation. A real
/// note quotes `base_tree_hashes: &[String]`, and `check` duly reported
/// `cited but undefined: [String]`. Bracketed type and collection syntax
/// is ordinary content in a document about code — in Rust, in Python
/// annotations, in TypeScript, in Java generics — and a checker that
/// cannot tell it from a citation generates noise precisely where the
/// prose is most technical.
///
/// A first attempt using only "contains a digit" was wrong: it rejected
/// `R-ROOT` and `CYC-A`, real ids in this crate's own fixtures.
///
/// # Where this is imprecise, stated rather than assumed away
///
/// The failure mode is a false positive — content read as a citation,
/// surfacing as `cited but undefined` — never a missed citation, so the
/// cost is noise a reader can dismiss, not a check that silently passes.
///
/// A bracketed name that is uppercase-led *and* hyphenated collides. That
/// is impossible in C-family and Python-family identifiers but perfectly
/// ordinary in Lisp, Clojure and Scheme, where `-` is an identifier
/// character: a note about `[Some-Var]` in Clojure would be reported as
/// an unresolved citation. Likewise a bracketed CamelCase name carrying a
/// digit, like `[Utf8Error]`.
///
/// Both are accepted rather than engineered around. The alternative is
/// making the rule depend on which language a document is about, which
/// would require knowing that — and this tool deliberately does not.
fn is_citation_shaped(id: &str) -> bool {
    id.starts_with(|c: char| c.is_ascii_uppercase())
        && id.bytes().any(|b| b.is_ascii_digit() || b == b'-')
}

/// Scan one line for `[ID]` / `[!ID]` citations, returning each match's
/// byte offset of `[`, id, and refuted-stance flag. Shared by
/// `scan_citations` (body prose, where a line number is tracked) and
/// `citation_ids_in` (a row's own field text, where only the cited ids
/// matter, not a position to display).
fn scan_line(line: &str) -> Vec<(usize, String, bool)> {
    if let Some(trailer) = scan_cites_trailer(line) {
        return trailer;
    }
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            let stance = j < bytes.len() && bytes[j] == b'!';
            if stance {
                j += 1;
            }
            let id_start = j;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'-')
            {
                j += 1;
            }
            if j > id_start && j < bytes.len() && bytes[j] == b']' {
                let id = &line[id_start..j];
                if is_citation_shaped(id) {
                    out.push((i, id.to_string(), stance));
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// A whole-line `*cites: C1, C4*` trailer as [`compose`](crate::compose)
/// writes it, or `None` if this line is not one.
///
/// Deliberately strict: the line must be *only* the trailer, so that
/// ordinary prose mentioning the word "cites" between asterisks is never
/// silently reinterpreted as a citation. Each id gets the byte offset it
/// actually starts at, so a malformed trailer still reports a usable
/// column. There is no refuted stance here — the trailer form has no
/// `[!ID]` equivalent, because a block-level attribution says "this rests
/// on that", never "this refutes that".
fn scan_cites_trailer(line: &str) -> Option<Vec<(usize, String, bool)>> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix("*cites:")?.strip_suffix('*')?;
    // Where `inner` begins within the original line, so the offsets below
    // are relative to the line and not to the trimmed slice.
    let base = line.len() - line.trim_start().len() + "*cites:".len();

    let mut out = Vec::new();
    let mut offset = 0usize;
    for part in inner.split(',') {
        let id = part.trim();
        if !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            let within = part.len() - part.trim_start().len();
            out.push((base + offset + within, id.to_string(), false));
        }
        offset += part.len() + 1; // + 1 for the ',' that split consumed
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

pub fn scan_citations(body: &[String]) -> Vec<Citation> {
    let mut out = Vec::new();
    for (idx, line) in body.iter().enumerate() {
        let line_no = idx + 1;
        for (col, id, stance_refuted) in scan_line(line) {
            out.push(Citation {
                line: line_no,
                col,
                id,
                stance_refuted,
            });
        }
    }
    out
}

/// The citation ids (and their refuted-stance flags) found anywhere in an
/// arbitrary string — used for a row's own free-text fields (`claim`,
/// `note`), where only *which* rows are cited matters, not a line/column
/// to display. Feeds the row→row edges of the dependency-cascade check.
pub fn citation_ids_in(text: &str) -> Vec<(String, bool)> {
    scan_line(text)
        .into_iter()
        .map(|(_, id, stance)| (id, stance))
        .collect()
}

/// What was found immediately before a citation, for check 3.
pub enum AbuttingContext {
    /// A high-confidence literal-looking token separated from the
    /// citation by at most one space — "abutting distance" per the
    /// spec's own example (`…at 29s [S3-L17]`).
    Abutting(String),
    /// A literal-looking token present on the same line but not
    /// promoted to the failing tier: either separated from the citation
    /// by more than one space (other words in between), or a bare
    /// integer sitting right at abutting distance but not confident
    /// enough to fail on — a printed candidate, never a failure.
    Candidate(String),
    /// Nothing literal-shaped nearby.
    None,
}

/// Markdown/punctuation cruft that can wrap or trail a token without
/// being part of the literal it denotes: inline-code backticks, straight
/// quotes, parentheses, and brackets (once a token has already been
/// ruled out as a citation — see `is_citation_token`), plus the
/// punctuation that trails a word in a sentence.
const WRAP_CHARS: &str = "`'\"()[],.;:!?";

/// Strip the markdown/punctuation cruft in [`WRAP_CHARS`] from both ends
/// of a token. This is the one place every caller — classification,
/// comparison, and display — goes through to get a token's literal core.
fn strip_wrap(tok: &str) -> &str {
    tok.trim_matches(|c: char| WRAP_CHARS.contains(c))
}

/// True if `tok` is itself a citation (`[ID]` or `[!ID]`), possibly with
/// ordinary sentence punctuation still trailing it (`[E-4].`) or wrapped
/// in parens (`([E-4])`). Citations are never literals, no matter how
/// digit-heavy their id — a citation immediately preceding another
/// citation (`[E-4] [E-5]`) must not be misread as [E-5]'s asserted
/// value.
fn is_citation_token(tok: &str) -> bool {
    let t = tok.trim_matches(|c: char| "`'\"(),.;:!?".contains(c));
    let Some(bracketed) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return false;
    };
    let id = bracketed.strip_prefix('!').unwrap_or(bracketed);
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The literal-classification shape of a token, once citation syntax is
/// ruled out and markdown wrapping is accounted for.
enum Shape {
    /// Not literal-shaped at all.
    None,
    /// A bare, unadorned integer — digits only, never quoted, no unit
    /// suffix, no decimal point. This is also the shape of an ordinary
    /// cross-reference ("Table 3", "Appendix 2"), which sits immediately
    /// before a citation bracket far more often than an asserted value
    /// does. Literal-shaped, but not high-confidence: it can surface as
    /// a printed Candidate, never as a failing Abutting match.
    BareInteger,
    /// A quoted verbatim string, or any other token containing a digit
    /// (durations, decimals, exit codes) — high-confidence literal.
    Literal,
}

/// A token counts as a literal if it's a quoted string or contains a
/// digit — the shape of the values this format's `value` field holds
/// (durations, counts, exit codes, verbatim strings) — after citation
/// syntax and markdown/punctuation wrapping are stripped away.
fn classify(tok: &str) -> Shape {
    if is_citation_token(tok) {
        return Shape::None;
    }
    // Whether the token was deliberately wrapped in straight double
    // quotes — this format's way of marking a verbatim string value,
    // literal regardless of digits — has to be decided before the full
    // `strip_wrap` below removes the quotes. Only ordinary
    // sentence-trailing punctuation is trimmed first, mirroring how a
    // quoted phrase actually appears in prose (e.g. `"foo",`).
    let sentence_trimmed = tok.trim_matches(|c: char| ",.;:)!?".contains(c));
    let quoted = sentence_trimmed.len() >= 2
        && sentence_trimmed.starts_with('"')
        && sentence_trimmed.ends_with('"');

    let stripped = strip_wrap(tok);
    if stripped.is_empty() {
        return Shape::None;
    }
    if quoted {
        return Shape::Literal;
    }
    if stripped.chars().all(|c| c.is_ascii_digit()) {
        return Shape::BareInteger;
    }
    if stripped.chars().any(|c| c.is_ascii_digit()) {
        Shape::Literal
    } else {
        Shape::None
    }
}

/// Any literal-shaped token — including a bare integer — worth surfacing
/// at all (as at least a Candidate).
fn is_literal_token(tok: &str) -> bool {
    !matches!(classify(tok), Shape::None)
}

/// Only the high-confidence literal shapes that clear the bar for the
/// failing Abutting tier: quoted strings, decimals, unit-suffixed
/// numbers, and anything else with a digit — except a bare unadorned
/// integer, which reads as an ordinary cross-reference too often to
/// redden a run on its own.
fn is_high_confidence_literal(tok: &str) -> bool {
    matches!(classify(tok), Shape::Literal)
}

/// Strip the same markdown/punctuation cruft `is_literal_token` strips,
/// producing the canonical form of a literal used for both comparison
/// and display. Used for a citation's abutting token and for a row's
/// `value` field alike, so both sides of check 3's comparison are
/// normalised the same way.
pub fn normalize_literal(tok: &str) -> String {
    strip_wrap(tok).to_string()
}

/// `line` is the raw source line the citation was found on; `bracket_col`
/// is the byte offset of the citation's opening `[` on that line.
///
/// Only the token immediately before the citation (zero or one space of
/// gap) can produce a verdict — that is "abutting distance" per the
/// spec's own example (`…at 29s [S3-L17]`), and only if it's a
/// high-confidence literal shape. A literal-shaped token anywhere else
/// on the same line, further back — or a bare integer right at abutting
/// distance — is reported as a printed candidate: worth a human's
/// glance, never grounds for a failure.
pub fn abutting_context(line: &str, bracket_col: usize) -> AbuttingContext {
    let prefix = &line[..bracket_col];
    let trimmed = prefix.trim_end();
    let gap = prefix.len() - trimmed.len();
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();

    if let Some(&last) = tokens.last() {
        if gap <= 1 && is_high_confidence_literal(last) {
            return AbuttingContext::Abutting(last.to_string());
        }
    }
    if let Some(&tok) = tokens.iter().rev().find(|t| is_literal_token(t)) {
        return AbuttingContext::Candidate(tok.to_string());
    }
    AbuttingContext::None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression this module's doc comment describes: `render`
    /// writes `*cites: …*`, and before this was fixed `check` read that
    /// line as containing no citations at all.
    #[test]
    fn a_cites_trailer_is_scanned_as_citations() {
        let ids: Vec<String> = scan_citations(&["*cites: C1, C4*".to_string()])
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, vec!["C1", "C4"]);
    }

    #[test]
    fn a_single_id_trailer_and_a_leading_indent_both_scan() {
        let ids: Vec<String> = scan_citations(&["  *cites: C1*".to_string()])
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, vec!["C1"]);
    }

    /// Each id must report the column it really starts at, not the start
    /// of the trailer — `abutting_context` slices the line at that offset
    /// and would panic on a byte index past the end.
    #[test]
    fn trailer_columns_point_at_each_id() {
        let line = "*cites: C1, C40*".to_string();
        let cits = scan_citations(&[line.clone()]);
        for c in &cits {
            assert_eq!(&line[c.col..c.col + c.id.len()], c.id, "column was wrong");
        }
    }

    /// A trailer is a block-level attribution — "this rests on that" —
    /// so it has no refuted form to carry.
    #[test]
    fn a_trailer_citation_never_carries_a_refuted_stance() {
        assert!(scan_citations(&["*cites: C1*".to_string()])
            .iter()
            .all(|c| !c.stance_refuted));
    }

    /// Prose that merely talks about citing must not become a citation.
    /// The trailer form is recognised only when it is the entire line.
    #[test]
    fn prose_mentioning_cites_mid_sentence_is_not_a_trailer() {
        let body = vec!["The memo *cites: C1* in passing, mid-sentence.".to_string()];
        assert!(scan_citations(&body).is_empty());
    }

    /// The inline form keeps working exactly as before, including its
    /// refuted stance — the trailer scan is additive, not a replacement.
    /// Bracketed Rust in a note is content, not a citation. This fired
    /// for real: a FORK-94 fact quotes `base_tree_hashes: &[String]`,
    /// and check reported `cited but undefined: [String]`.
    #[test]
    fn bracketed_type_names_are_not_citations() {
        for line in ["base_tree_hashes: &[String]", "takes &[u8]", "a Vec<[T]> here", "&[i32]"] {
            assert!(
                scan_citations(&[line.to_string()]).is_empty(),
                "should not scan as a citation: {line}"
            );
        }
    }

    /// Ids without digits are real — this crate's own fixtures use them —
    /// and a first attempt at the rule above rejected them.
    #[test]
    fn hyphenated_ids_without_digits_still_scan() {
        let ids: Vec<String> = scan_citations(&["see [R-ROOT] and [CYC-A]".to_string()])
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, vec!["R-ROOT", "CYC-A"]);
    }

    #[test]
    fn inline_bracket_citations_still_scan_with_stance() {
        let cits = scan_citations(&["grounded in [C1] but [!C2] was refuted".to_string()]);
        let seen: Vec<(String, bool)> =
            cits.into_iter().map(|c| (c.id, c.stance_refuted)).collect();
        assert_eq!(
            seen,
            vec![("C1".to_string(), false), ("C2".to_string(), true)]
        );
    }
}
