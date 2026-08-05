//! `tetel` — a checker for markdown design documents whose factual claims
//! carry executable evidence. This is the first slice: `tetel check <file>`
//! only. It never writes a file, never executes a command from the
//! document, and makes no network calls.

pub mod checks;
pub mod citations;
pub mod ledger;
pub mod model;
pub mod parse;
pub mod report;

use std::path::Path;

pub use report::{EXIT_CHECK_FAILED, EXIT_CLEAN, EXIT_NO_ROWS};

/// Runs `check` against already-loaded source text. `display_path` is used
/// only in the no-rows-found message.
pub fn check_str(display_path: &str, source: &str) -> (i32, String) {
    let doc = parse::parse_document(source);
    let findings = checks::analyze(&doc);
    report::render(display_path, &doc, &findings)
}

/// Runs `check` against a file on disk.
pub fn check_file(path: &Path) -> std::io::Result<(i32, String)> {
    let source = std::fs::read_to_string(path)?;
    Ok(check_str(&path.display().to_string(), &source))
}
