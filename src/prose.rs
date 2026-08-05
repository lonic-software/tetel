//! `tetel prose` — compose the document's prose, one block at a time.
//! Creation is deliberately not gated (mirrors the prototype: prose is
//! where the author talks, not where they're audited — citing a
//! nonexistent claim isn't even checked here). Revision is gated the
//! same way `claims.rs`'s is: `--why` is required, and the superseded
//! text stays in the append-only log — `tetel render` only ever shows a
//! block's current text.
//!
//! **Not ported**: `--before`/`--after` insertion and `tmove`
//! reordering. No session across four prototype runs ever exercised
//! either — every block was composed and left in append order — so this
//! port keeps document order identical to creation order and drops the
//! reordering machinery rather than carry code nothing has exercised.
//! A block's text can still be revised in place with `--revise`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session::{self, AuthoringError, Kind};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum ProseEvent {
    Create {
        id: String,
        heading: bool,
        /// A markdown heading depth, 1..=6. `None` for a paragraph.
        /// Fix in this port: the prototype rendered every heading at one
        /// fixed depth (`##`), which two runs independently reported as
        /// a defect.
        #[serde(default)]
        level: Option<u8>,
        text: String,
        #[serde(default)]
        cite: Vec<String>,
        timestamp: u64,
    },
    Revise {
        id: String,
        text: String,
        why: String,
        timestamp: u64,
    },
}

pub struct Block {
    pub id: String,
    pub heading: bool,
    pub level: Option<u8>,
    pub text: String,
    pub cite: Vec<String>,
    pub revisions: usize,
}

fn log_path(session_dir: &Path) -> PathBuf {
    session_dir.join("prose.jsonl")
}

/// Every block, in creation order — which is also document order, since
/// this port has no reordering (see the module doc comment).
pub fn load_all(session_dir: &Path) -> io::Result<Vec<Block>> {
    let events: Vec<ProseEvent> = session::read_jsonl(&log_path(session_dir))?;
    let mut by_id: std::collections::BTreeMap<String, Block> = std::collections::BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for ev in events {
        match ev {
            ProseEvent::Create { id, heading, level, text, cite, .. } => {
                order.push(id.clone());
                by_id.insert(id.clone(), Block { id, heading, level, text, cite, revisions: 0 });
            }
            ProseEvent::Revise { id, text, .. } => {
                if let Some(b) = by_id.get_mut(&id) {
                    b.text = text;
                    b.revisions += 1;
                }
            }
        }
    }
    Ok(order.into_iter().filter_map(|id| by_id.remove(&id)).collect())
}

pub fn exists(session_dir: &Path, id: &str) -> io::Result<bool> {
    Ok(load_all(session_dir)?.iter().any(|b| b.id == id))
}

fn parse_ids(csv: &str) -> Vec<String> {
    csv.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// Mints a paragraph block (`heading_level = None`) or a heading block
/// at the given depth. `text` must already be resolved (see
/// `session::resolve_text_value`/`resolve_text_or_stdin`) by the caller.
pub fn create(session_dir: &Path, text: &str, cite_csv: Option<&str>, heading_level: Option<u8>) -> Result<Block, AuthoringError> {
    if let Some(level) = heading_level {
        if !(1..=6).contains(&level) {
            return Err(session::refuse(session_dir, "prose", format!("--level must be 1..=6 (a markdown heading depth), got {level}")));
        }
    }
    let cite = cite_csv.map(parse_ids).unwrap_or_default();
    let id = session::next_id(session_dir, Kind::Prose)?;
    let event = ProseEvent::Create {
        id: id.clone(),
        heading: heading_level.is_some(),
        level: heading_level,
        text: text.to_string(),
        cite: cite.clone(),
        timestamp: session::now_unix(),
    };
    session::append_jsonl(&log_path(session_dir), &event)?;
    Ok(Block { id, heading: heading_level.is_some(), level: heading_level, text: text.to_string(), cite, revisions: 0 })
}

/// `tetel prose --revise <id> --why <text> [new text, from stdin by default]`.
pub fn revise(session_dir: &Path, id: &str, new_text: &str, why: &str) -> Result<(), AuthoringError> {
    if !exists(session_dir, id)? {
        return Err(session::refuse(session_dir, "prose", format!("no such prose block: {id}")));
    }
    if why.trim().is_empty() {
        return Err(session::refuse(session_dir, "prose", "--revise requires --why (revisions must explain themselves)"));
    }
    let event = ProseEvent::Revise { id: id.to_string(), text: new_text.to_string(), why: why.to_string(), timestamp: session::now_unix() };
    session::append_jsonl(&log_path(session_dir), &event)?;
    Ok(())
}
