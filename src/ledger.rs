//! Imports a memo's hand-maintained evidence ledger: a markdown table whose
//! header is `| ID | Proposition | Domain | Extent | Kind | Status |` (the
//! `Kind` column is sometimes absent — both shapes are accepted; see the
//! report). Import is read-only: nothing here ever writes to the memo.
//!
//! Cell text is carried byte-exact (after the one trim markdown table
//! syntax itself requires: the single space of padding either side of a
//! `|`). Bold, backticks, bracketed citations and everything else inside a
//! cell survive untouched — `brief` depends on that.

/// One row of the evidence ledger, parsed but unclassified: every field
/// stays exactly as written except `pin`, which is not a column at all —
/// it is stated once in prose above the table ("at pin `<hash>`") and
/// applies to every row beneath it unless a later such sentence overrides
/// it before the next table.
#[derive(Debug, Clone)]
pub struct Claim {
    /// 1-based source line the row starts on.
    pub line: usize,
    pub id: String,
    pub proposition: String,
    pub domain: String,
    pub extent: String,
    /// Absent when the table's header has no `Kind` column at all (seen in
    /// the wild — not every ledger marks RUN/READING).
    pub kind: Option<String>,
    pub status: String,
    pub pin: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LedgerImportError {
    pub line: usize,
    pub message: String,
}

impl LedgerImportError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        LedgerImportError {
            line,
            message: message.into(),
        }
    }
}

pub struct LedgerImport {
    pub claims: Vec<Claim>,
    pub errors: Vec<LedgerImportError>,
}

const HEADER_WITH_KIND: [&str; 6] = ["ID", "Proposition", "Domain", "Extent", "Kind", "Status"];
const HEADER_WITHOUT_KIND: [&str; 5] = ["ID", "Proposition", "Domain", "Extent", "Status"];

/// Sentinel `Domain`/`Extent` cell text `compose::render` writes for a
/// claim minted with `tetel claim`: the authoring model (`claims.rs`) has
/// no scope/domain field on a claim at all, and deriving one from the
/// facts a claim cites would recreate exactly the vacuity this project
/// measured and rejected once already — a field and its own check
/// answered by one act. Writing this exact, recognisable string instead
/// of a fabricated value is what lets `checks::claims_without_declared_scope`
/// name the gap plainly rather than let an empty-seeming cell pass as an
/// (unearned) claim of total coverage.
pub const NO_SCOPE_DECLARED: &str = "not declared (tetel's authoring model has no domain/extent field on a claim)";

/// Split one `|`-delimited table row into its cells. Respects a backtick
/// span (a `|` inside `` `...` `` is not a separator) and a `\|` escape,
/// and drops the empty leading/trailing cell markdown table syntax
/// produces from the row's bounding pipes.
fn split_row_cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut in_backtick = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                current.push('|');
                chars.next();
            }
            '`' => {
                in_backtick = !in_backtick;
                current.push('`');
            }
            '|' if !in_backtick => {
                cells.push(std::mem::take(&mut current).trim().to_string());
            }
            _ => current.push(c),
        }
    }
    cells.push(current.trim().to_string());
    if cells.first().is_some_and(String::is_empty) {
        cells.remove(0);
    }
    if cells.last().is_some_and(String::is_empty) {
        cells.pop();
    }
    cells
}

fn is_table_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// True if every cell of a row is made up only of `-`, `:` and spaces —
/// the header/body separator row markdown tables require.
fn is_separator_row(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells
            .iter()
            .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':'))
}

/// True if `word` occurs in `text` as a whole word (not as a substring of
/// a longer word) — used so "pin" doesn't fire on "pinned"/"unpinned"/etc.
fn contains_word(text: &str, word: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut start = 0;
    while let Some(rel) = text[start..].find(word) {
        let pos = start + rel;
        let before_ok = pos == 0 || !(bytes[pos - 1] as char).is_alphanumeric();
        let after = pos + word.len();
        let after_ok = after >= bytes.len() || !(bytes[after] as char).is_alphanumeric();
        if before_ok && after_ok {
            return Some(pos);
        }
        start = pos + word.len();
    }
    None
}

/// Look for a document-level "at pin `<hash>`" sentence in the prose
/// preceding a ledger table (the observed convention in every memo in the
/// corpus: "Rows are against `path` at pin `71c0fd1` unless stated."). The
/// search window is bounded by the nearest preceding markdown heading, so
/// an unrelated mention of "pin" earlier in the document can't leak in.
/// Known limitation: a per-row pin override ("unless stated") is not
/// parsed — none occurs in the corpus this was built against, and the
/// prose form such an override would take is not demonstrated anywhere to
/// build a parser against.
fn find_stated_pin(section_lines: &[&str]) -> Option<String> {
    let mut best: Option<String> = None;
    for line in section_lines {
        let mut search_from = 0usize;
        while let Some(word_pos) = contains_word(&line[search_from..], "pin") {
            let abs_pos = search_from + word_pos + "pin".len();
            let rest = &line[abs_pos..];
            if let Some(open) = rest.find('`') {
                if let Some(close_rel) = rest[open + 1..].find('`') {
                    best = Some(rest[open + 1..open + 1 + close_rel].to_string());
                }
            }
            search_from = abs_pos;
        }
    }
    best
}

pub fn import(body: &[String]) -> LedgerImport {
    let mut claims = Vec::new();
    let mut errors = Vec::new();
    let mut id_first_seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    let mut section_start = 0usize; // index into body of the line after the nearest preceding heading
    let mut i = 0usize;
    while i < body.len() {
        let line = &body[i];
        if line.trim_start().starts_with('#') {
            section_start = i + 1;
        }
        if !is_table_row(line) {
            i += 1;
            continue;
        }
        let header_cells = split_row_cells(line);
        let has_kind = header_cells == HEADER_WITH_KIND;
        let no_kind = header_cells == HEADER_WITHOUT_KIND;
        if !has_kind && !no_kind {
            // Some other table entirely (e.g. a spike ledger) — not ours.
            i += 1;
            continue;
        }
        let header_line = i;
        if i + 1 >= body.len() || !is_table_row(&body[i + 1]) || !is_separator_row(&split_row_cells(&body[i + 1])) {
            errors.push(LedgerImportError::new(
                header_line + 1,
                "evidence ledger header found with no valid separator row beneath it",
            ));
            i += 1;
            continue;
        }
        let expected_cols = header_cells.len();
        let pin = {
            let window: Vec<&str> = body[section_start..header_line].iter().map(String::as_str).collect();
            find_stated_pin(&window)
        };

        let mut j = i + 2;
        while j < body.len() && is_table_row(&body[j]) {
            let row_line = j + 1; // 1-based
            let cells = split_row_cells(&body[j]);
            if cells.len() != expected_cols {
                errors.push(LedgerImportError::new(
                    row_line,
                    format!(
                        "ledger row has {} cell(s), expected {expected_cols} (header: {})",
                        cells.len(),
                        header_cells.join(" | ")
                    ),
                ));
                j += 1;
                continue;
            }
            let (id, proposition, domain, extent, kind, status) = if has_kind {
                (
                    cells[0].clone(),
                    cells[1].clone(),
                    cells[2].clone(),
                    cells[3].clone(),
                    Some(cells[4].clone()),
                    cells[5].clone(),
                )
            } else {
                (
                    cells[0].clone(),
                    cells[1].clone(),
                    cells[2].clone(),
                    cells[3].clone(),
                    None,
                    cells[4].clone(),
                )
            };
            if id.is_empty() {
                errors.push(LedgerImportError::new(row_line, "ledger row has an empty id"));
                j += 1;
                continue;
            }
            if proposition.is_empty() {
                errors.push(LedgerImportError::new(
                    row_line,
                    format!("ledger row {id} has an empty proposition"),
                ));
                j += 1;
                continue;
            }
            if let Some(&first_line) = id_first_seen.get(&id) {
                errors.push(LedgerImportError::new(
                    row_line,
                    format!("duplicate ledger id `{id}` (first defined at line {first_line})"),
                ));
                j += 1;
                continue;
            }
            id_first_seen.insert(id.clone(), row_line);
            claims.push(Claim {
                line: row_line,
                id,
                proposition,
                domain,
                extent,
                kind,
                status,
                pin: pin.clone(),
            });
            j += 1;
        }
        i = j;
    }

    LedgerImport { claims, errors }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn parses_a_well_formed_row_with_kind_column() {
        let doc = lines(
            "\
## Evidence ledger

Rows are against `src/foo.rs` at pin `abc1234` unless stated.

| ID | Proposition | Domain | Extent | Kind | Status |
|---|---|---|---|---|---|
| X-1 | `foo` returns **4** always [X-2] | `foo`'s body | Opened in full | READING | **VERIFIED** |
",
        );
        let import = import(&doc);
        assert!(import.errors.is_empty(), "{:?}", import.errors);
        assert_eq!(import.claims.len(), 1);
        let c = &import.claims[0];
        assert_eq!(c.id, "X-1");
        assert_eq!(c.proposition, "`foo` returns **4** always [X-2]");
        assert_eq!(c.kind.as_deref(), Some("READING"));
        assert_eq!(c.status, "**VERIFIED**");
        assert_eq!(c.pin.as_deref(), Some("abc1234"));
    }

    #[test]
    fn parses_a_well_formed_row_without_kind_column() {
        let doc = lines(
            "\
## Evidence ledger

All rows are against `src/bar.rs` unless stated, at pin `deadbee`.

| ID | Proposition | Domain | Extent | Status |
|---|---|---|---|---|
| Y-1 | bar holds | bar's body | opened | **VERIFIED** |
",
        );
        let import = import(&doc);
        assert!(import.errors.is_empty(), "{:?}", import.errors);
        assert_eq!(import.claims.len(), 1);
        assert_eq!(import.claims[0].kind, None);
        assert_eq!(import.claims[0].pin.as_deref(), Some("deadbee"));
    }

    #[test]
    fn a_row_with_the_wrong_cell_count_is_reported_not_dropped() {
        let doc = lines(
            "\
| ID | Proposition | Domain | Extent | Kind | Status |
|---|---|---|---|---|---|
| Z-1 | missing a cell | domain only | extent only | READING |
",
        );
        let import = import(&doc);
        assert!(import.claims.is_empty());
        assert_eq!(import.errors.len(), 1);
        assert!(import.errors[0].message.contains("5 cell"));
    }

    #[test]
    fn a_pipe_inside_backticks_does_not_split_the_cell() {
        let doc = lines(
            "\
| ID | Proposition | Domain | Extent | Kind | Status |
|---|---|---|---|---|---|
| W-1 | `grep -E 'a|b'` matches | domain | extent | RUN | **VERIFIED** |
",
        );
        let import = import(&doc);
        assert!(import.errors.is_empty(), "{:?}", import.errors);
        assert_eq!(import.claims.len(), 1);
        assert_eq!(import.claims[0].proposition, "`grep -E 'a|b'` matches");
    }

    #[test]
    fn a_table_with_a_different_header_is_not_a_ledger() {
        let doc = lines(
            "\
| # | Claim | Disposition | Result |
|---|---|---|---|
| S1 | something | run | fine |
",
        );
        let import = import(&doc);
        assert!(import.claims.is_empty());
        assert!(import.errors.is_empty());
    }

    #[test]
    fn duplicate_ledger_id_is_reported() {
        let doc = lines(
            "\
| ID | Proposition | Domain | Extent | Kind | Status |
|---|---|---|---|---|---|
| D-1 | first | d | e | READING | **VERIFIED** |
| D-1 | second | d | e | READING | **VERIFIED** |
",
        );
        let import = import(&doc);
        assert_eq!(import.claims.len(), 1);
        assert_eq!(import.errors.len(), 1);
        assert!(import.errors[0].message.contains("duplicate ledger id"));
    }
}
