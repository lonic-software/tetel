//! `tetel render` — assemble the session's current prose state into
//! markdown on stdout. Mirrors `trender`: the only authoring command
//! that produces the finished document. Every other authoring command
//! writes its own record; none of them writes anything that resembles
//! the assembled piece.
//!
//! Headings render at their own declared depth (`#` * level), not a
//! single fixed depth — see `prose.rs`'s doc comment on why that's a
//! fix, not a stylistic choice. A revised block renders only its
//! current text; superseded text lives in `prose.jsonl`'s history but
//! never here.

use std::io;
use std::path::Path;

use crate::prose;

pub fn render(session_dir: &Path) -> io::Result<String> {
    let blocks = prose::load_all(session_dir)?;
    let mut out = String::new();
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if b.heading {
            let level = b.level.unwrap_or(2).clamp(1, 6);
            out.push_str(&"#".repeat(level as usize));
            out.push(' ');
            out.push_str(&b.text);
            out.push('\n');
        } else {
            out.push_str(&b.text);
            out.push('\n');
            if !b.cite.is_empty() {
                out.push('\n');
                out.push_str(&format!("*cites: {}*\n", b.cite.join(", ")));
            }
        }
    }
    Ok(out)
}
