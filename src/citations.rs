//! Scans a document's body prose (fenced regions already blanked out by
//! `parse::parse_document`) for `[ID]` / `[!ID]` citations, and classifies
//! what immediately precedes each citation for the abutting-literal check.

/// One `[ID]` or `[!ID]` citation found in body prose.
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

pub fn scan_citations(body: &[String]) -> Vec<Citation> {
    let mut out = Vec::new();
    for (idx, line) in body.iter().enumerate() {
        let line_no = idx + 1;
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
                    out.push(Citation {
                        line: line_no,
                        col: i,
                        id: line[id_start..j].to_string(),
                        stance_refuted: stance,
                    });
                    i = j + 1;
                    continue;
                }
            }
            i += 1;
        }
    }
    out
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
