//! Extracts ```tetel fenced blocks from a markdown file, splits each block
//! into rows (row groups separated by a blank line), parses each row's
//! `key: value` fields, and returns everything the checks need: the parsed
//! rows, the grammar errors found along the way, and the document's body
//! text (fenced regions blanked out, line numbers preserved) for citation
//! scanning.

use crate::model::{Designator, Kind, Row, Status, ALLOWED_FIELDS};

#[derive(Debug, Clone)]
pub struct GrammarError {
    /// 1-based source line, when the error is line-specific.
    pub line: usize,
    pub message: String,
}

impl GrammarError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        GrammarError {
            line,
            message: message.into(),
        }
    }
}

pub struct Document {
    pub rows: Vec<Row>,
    pub grammar_errors: Vec<GrammarError>,
    /// The document with every fenced-code region (```tetel or otherwise)
    /// replaced by blank lines, so line numbers still line up for citation
    /// scanning while code spans never contribute false citations.
    pub body: Vec<String>,
    /// How many row groups were found inside ```tetel fences, valid or
    /// not. Zero means "no tetel rows found in this file" (a distinct exit
    /// code from clean), regardless of how the rest of the checks would
    /// have gone.
    pub row_groups_found: usize,
}

fn fence_open(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let backticks = trimmed.chars().take_while(|&c| c == '`').count();
    if backticks < 3 {
        return None;
    }
    let lang = trimmed[backticks..].trim();
    Some((backticks, lang.to_string()))
}

fn fence_close(line: &str, min_backticks: usize) -> bool {
    let trimmed = line.trim();
    let backticks = trimmed.chars().take_while(|&c| c == '`').count();
    backticks >= min_backticks && trimmed.chars().all(|c| c == '`')
}

/// One row group's raw field lines plus the 1-based line the group starts on.
struct RawGroup {
    start_line: usize,
    fields: Vec<(String, String, usize)>, // key, value, line number
    unparsed: Vec<(usize, String)>,       // lines that aren't `key: value`
}

fn split_into_groups(block_lines: &[&str], block_start_line: usize) -> Vec<RawGroup> {
    let mut groups = Vec::new();
    let mut current: Option<RawGroup> = None;
    for (offset, line) in block_lines.iter().enumerate() {
        let line_no = block_start_line + offset;
        if line.trim().is_empty() {
            if let Some(g) = current.take() {
                groups.push(g);
            }
            continue;
        }
        let group = current.get_or_insert_with(|| RawGroup {
            start_line: line_no,
            fields: Vec::new(),
            unparsed: Vec::new(),
        });
        match line.split_once(':') {
            Some((key, value)) if !key.trim().is_empty() && key.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                group
                    .fields
                    .push((key.trim().to_string(), value.trim().to_string(), line_no));
            }
            _ => group.unparsed.push((line_no, line.to_string())),
        }
    }
    if let Some(g) = current.take() {
        groups.push(g);
    }
    groups
}

/// Last value for a field, if present more than once (last one wins; no
/// failure category for an in-row repeated field in this slice).
fn field<'a>(fields: &'a [(String, String, usize)], key: &str) -> Option<(&'a str, usize)> {
    fields
        .iter()
        .rev()
        .find(|(k, _, _)| k == key)
        .map(|(_, v, l)| (v.as_str(), *l))
}

fn build_row(group: &RawGroup, errors: &mut Vec<GrammarError>) -> Option<Row> {
    for (key, _, line) in &group.fields {
        if !ALLOWED_FIELDS.contains(&key.as_str()) {
            errors.push(GrammarError::new(*line, format!("unknown field `{key}`")));
        }
    }
    for (line, text) in &group.unparsed {
        errors.push(GrammarError::new(
            *line,
            format!("not a `key: value` line: {text:?}"),
        ));
    }

    let id = field(&group.fields, "id");
    let claim = field(&group.fields, "claim");
    let domain_raw = field(&group.fields, "domain");
    let extent_raw = field(&group.fields, "extent");
    let pin = field(&group.fields, "pin");
    let kind_raw = field(&group.fields, "kind");
    let run = field(&group.fields, "run");
    let value = field(&group.fields, "value");
    let date = field(&group.fields, "date");
    let status_raw = field(&group.fields, "status");
    let note = field(&group.fields, "note");

    for (name, present) in [
        ("id", id.is_some()),
        ("claim", claim.is_some()),
        ("domain", domain_raw.is_some()),
        ("extent", extent_raw.is_some()),
        ("pin", pin.is_some()),
        ("kind", kind_raw.is_some()),
        ("status", status_raw.is_some()),
    ] {
        if !present {
            errors.push(GrammarError::new(
                group.start_line,
                format!("missing required field `{name}`"),
            ));
        }
    }

    let kind = kind_raw.and_then(|(v, line)| {
        let parsed = Kind::parse(v);
        if parsed.is_none() {
            errors.push(GrammarError::new(
                line,
                format!("invalid `kind` value `{v}` (expected RUN, READING, OBSERVED or ATTESTED)"),
            ));
        }
        parsed
    });

    let status = status_raw.and_then(|(v, line)| {
        let parsed = Status::parse(v);
        if parsed.is_none() {
            errors.push(GrammarError::new(
                line,
                format!("invalid `status` value `{v}` (expected VERIFIED, OWED or REFUTED)"),
            ));
        }
        parsed
    });

    let domain = domain_raw.and_then(|(v, line)| match Designator::parse_list(v) {
        Ok(d) => Some(d),
        Err(errs) => {
            for e in errs {
                errors.push(GrammarError::new(line, format!("malformed `domain`: {e}")));
            }
            None
        }
    });

    let extent = extent_raw.and_then(|(v, line)| match Designator::parse_list(v) {
        Ok(d) => Some(d),
        Err(errs) => {
            for e in errs {
                errors.push(GrammarError::new(line, format!("malformed `extent`: {e}")));
            }
            None
        }
    });

    // Field consistency: which of run/value/date are required vs forbidden
    // depends on kind. `date` is not addressed explicitly for non-ATTESTED
    // rows by the row grammar as specified; treated here as forbidden
    // outside ATTESTED, symmetric with run/value on READING. See report.
    if let Some(k) = kind {
        match k {
            Kind::Reading => {
                if let Some((_, line)) = run {
                    errors.push(GrammarError::new(line, "field `run` is forbidden on a READING row"));
                }
                if let Some((_, line)) = value {
                    errors.push(GrammarError::new(line, "field `value` is forbidden on a READING row"));
                }
                if let Some((_, line)) = date {
                    errors.push(GrammarError::new(line, "field `date` is forbidden on a READING row"));
                }
            }
            Kind::Run | Kind::Observed | Kind::Attested => {
                if run.is_none() {
                    errors.push(GrammarError::new(
                        group.start_line,
                        format!("missing required field `run` for kind {k}"),
                    ));
                }
                if value.is_none() {
                    errors.push(GrammarError::new(
                        group.start_line,
                        format!("missing required field `value` for kind {k}"),
                    ));
                }
                if k == Kind::Attested {
                    if date.is_none() {
                        errors.push(GrammarError::new(
                            group.start_line,
                            "missing required field `date` for an ATTESTED row",
                        ));
                    }
                } else if let Some((_, line)) = date {
                    errors.push(GrammarError::new(
                        line,
                        format!("field `date` is forbidden on a {k} row (required iff ATTESTED)"),
                    ));
                }
            }
        }
    }

    // Build the Row only when the essentials needed by the checks are all
    // present and valid; a row missing these can't be reasoned about
    // downstream and is left out (its id, if any, still counts for
    // duplicate-id detection and for "cited but undefined" reporting).
    let (Some((id, _)), Some((claim, _)), Some(domain), Some(extent), Some((pin, _)), Some(kind), Some(status)) =
        (id, claim, domain, extent, pin, kind, status)
    else {
        return None;
    };

    Some(Row {
        line: group.start_line,
        id: id.to_string(),
        claim: claim.to_string(),
        domain,
        extent,
        pin: pin.to_string(),
        kind,
        run: run.map(|(v, _)| v.to_string()),
        value: value.map(|(v, _)| v.to_string()),
        date: date.map(|(v, _)| v.to_string()),
        status,
        note: note.map(|(v, _)| v.to_string()),
    })
}

pub fn parse_document(source: &str) -> Document {
    let lines: Vec<&str> = source.lines().collect();
    let mut body: Vec<String> = Vec::with_capacity(lines.len());
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    let mut row_groups_found = 0usize;
    let mut id_first_seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    let mut i = 0usize;
    while i < lines.len() {
        if let Some((backticks, lang)) = fence_open(lines[i]) {
            let open_line = i;
            let mut j = i + 1;
            while j < lines.len() && !fence_close(lines[j], backticks) {
                j += 1;
            }
            // j is the closing line, or lines.len() if unterminated.
            let content_end = j.min(lines.len());
            if lang == "tetel" {
                let block_lines = &lines[open_line + 1..content_end];
                let block_start_line = open_line + 2; // 1-based line of first content line
                let groups = split_into_groups(block_lines, block_start_line);
                row_groups_found += groups.len();
                for g in &groups {
                    if let Some((id_val, id_line)) = field(&g.fields, "id") {
                        if let Some(&first_line) = id_first_seen.get(id_val) {
                            errors.push(GrammarError::new(
                                id_line,
                                format!(
                                    "duplicate id `{id_val}` (first defined at line {first_line})"
                                ),
                            ));
                        } else {
                            id_first_seen.insert(id_val.to_string(), id_line);
                        }
                    }
                }
                for g in &groups {
                    if let Some(row) = build_row(g, &mut errors) {
                        rows.push(row);
                    }
                }
            }
            let last_line = if j < lines.len() { j } else { content_end.saturating_sub(1) };
            for _ in open_line..=last_line.min(lines.len().saturating_sub(1)) {
                body.push(String::new());
            }
            i = if j < lines.len() { j + 1 } else { lines.len() };
        } else {
            body.push(lines[i].to_string());
            i += 1;
        }
    }

    Document {
        rows,
        grammar_errors: errors,
        body,
        row_groups_found,
    }
}
