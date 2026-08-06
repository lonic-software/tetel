//! `tetel prose` — compose the document's prose, one block at a time.
//! Creation is deliberately not gated (mirrors the prototype: prose is
//! where the author talks, not where they're audited — citing a
//! nonexistent claim isn't even checked here). Revision is gated the
//! same way `claims.rs`'s is: `--why` is required, and the superseded
//! text stays in the append-only log — `tetel render` only ever shows a
//! block's current text.
//!
//! # Document order, and why `--before` exists
//!
//! Insertion was dropped in this port on the grounds that none of the
//! four prototype runs ever used it — every block was composed and left
//! in append order. That reasoning had a selection effect in it, spotted
//! later by an agent writing with the tool: **those runs never needed
//! insertion because they wrote all their prose at the end**, which is
//! precisely the pattern the rhythm brief exists to prevent. The evidence
//! for removing the feature came from runs exhibiting the behaviour the
//! tool now tries to stop.
//!
//! Without insertion, document order is authoring order. An author
//! following the brief — writing a paragraph the moment a claim exists to
//! say something about — gets a document in discovery order, which reads
//! badly. The only way to get a well-ordered document is to defer prose
//! to the end. So the tool's shape rewarded the anti-pattern, and no
//! amount of brief text could outweigh that: a prompt asks, a shape
//! decides.
//!
//! `create` therefore takes an optional `before`, and document order is
//! the insertion order the log records rather than the order blocks were
//! appended. `tmove`-style reordering of existing blocks stays unported:
//! nothing has needed it, and unlike insertion its absence creates no
//! incentive to write badly.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::workspace::{self, AuthoringError, Kind};

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
        /// The block this one was inserted before, if any. Absent — the
        /// shape of every Create written before this field existed —
        /// means "append", so an old log replays exactly as it did.
        #[serde(default)]
        before: Option<String>,
        timestamp: u64,
    },
    Revise {
        id: String,
        text: String,
        why: String,
        /// Citations this revision sets, replacing whatever the block
        /// carried. Absent — the shape every revision written before this
        /// field existed has — leaves them untouched, so an old log
        /// replays exactly as it did.
        #[serde(default)]
        cite: Option<Vec<String>>,
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

fn log_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("prose.jsonl")
}

/// Every block, in creation order — which is also document order, since
/// this port has no reordering (see the module doc comment).
pub fn load_all(workspace_dir: &Path) -> io::Result<Vec<Block>> {
    let events: Vec<ProseEvent> = workspace::read_jsonl(&log_path(workspace_dir))?;
    let mut by_id: std::collections::BTreeMap<String, Block> = std::collections::BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for ev in events {
        match ev {
            ProseEvent::Create { id, heading, level, text, cite, before, .. } => {
                // Insert at the anchor if it is still present; otherwise
                // append. A dangling anchor cannot reorder silently — the
                // block lands at the end, where its absence is visible.
                match before.as_deref().and_then(|b| order.iter().position(|o| o == b)) {
                    Some(at) => order.insert(at, id.clone()),
                    None => order.push(id.clone()),
                }
                by_id.insert(id.clone(), Block { id, heading, level, text, cite, revisions: 0 });
            }
            ProseEvent::Revise { id, text, cite, .. } => {
                if let Some(b) = by_id.get_mut(&id) {
                    b.text = text;
                    // Absent means "unchanged", not "cleared": a
                    // revision written before this field existed must
                    // replay identically.
                    if let Some(c) = cite {
                        b.cite = c;
                    }
                    b.revisions += 1;
                }
            }
        }
    }
    Ok(order.into_iter().filter_map(|id| by_id.remove(&id)).collect())
}

pub fn exists(workspace_dir: &Path, id: &str) -> io::Result<bool> {
    Ok(load_all(workspace_dir)?.iter().any(|b| b.id == id))
}

fn parse_ids(csv: &str) -> Vec<String> {
    csv.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// Mints a paragraph block (`heading_level = None`) or a heading block
/// at the given depth. `text` must already be resolved (see
/// `workspace::resolve_text_value`/`resolve_text_or_stdin`) by the caller.
pub fn create(
    workspace_dir: &Path,
    text: &str,
    cite_csv: Option<&str>,
    heading_level: Option<u8>,
    before: Option<&str>,
) -> Result<Block, AuthoringError> {
    if let Some(anchor) = before {
        if !exists(workspace_dir, anchor)? {
            return Err(workspace::refuse(
                workspace_dir,
                "prose",
                format!("no such prose block to insert before: {anchor}; try `tetel query prose`"),
            ));
        }
    }
    if let Some(level) = heading_level {
        if !(1..=6).contains(&level) {
            return Err(workspace::refuse(workspace_dir, "prose", format!("--level must be 1..=6 (a markdown heading depth), got {level}")));
        }
    }
    let cite = cite_csv.map(parse_ids).unwrap_or_default();
    let id = workspace::next_id(workspace_dir, Kind::Prose)?;
    let event = ProseEvent::Create {
        id: id.clone(),
        heading: heading_level.is_some(),
        level: heading_level,
        text: text.to_string(),
        cite: cite.clone(),
        before: before.map(str::to_string),
        timestamp: workspace::now_unix(),
    };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(Block { id, heading: heading_level.is_some(), level: heading_level, text: text.to_string(), cite, revisions: 0 })
}

/// `tetel prose --revise <id> --why <text> [new text, from stdin by default]`.
/// Revise a block's text, and optionally its citations.
///
/// `cite: None` leaves citations untouched. Until this parameter existed,
/// a paragraph created without a citation could never be given one —
/// `--cites` was accepted alongside `--revise` and silently discarded —
/// which made `tetel review`'s own instruction ("mint the missing claim
/// and cite it") unreachable for existing prose.
pub fn revise(
    workspace_dir: &Path,
    id: &str,
    new_text: &str,
    why: &str,
    cite: Option<Vec<String>>,
) -> Result<(), AuthoringError> {
    if !exists(workspace_dir, id)? {
        return Err(workspace::refuse(workspace_dir, "prose", format!("no such prose block: {id}")));
    }
    if why.trim().is_empty() {
        return Err(workspace::refuse(workspace_dir, "prose", "--revise requires --why (revisions must explain themselves)"));
    }
    let event = ProseEvent::Revise {
        id: id.to_string(),
        text: new_text.to_string(),
        why: why.to_string(),
        cite,
        timestamp: workspace::now_unix(),
    };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(())
}

/// What `tetel prose` was asked to do — append a paragraph, append a
/// heading, or revise an existing block. `text` arrives already resolved
/// (stdin/`@file`/literal, or the heading/paragraph body) by the caller;
/// only `--why`/`--level`, which the CLI never falls back to stdin for,
/// stay optional here. The single shape both the CLI and the MCP server
/// build and pass to [`dispatch`], so the two front ends can never drift
/// on which combination of missing flags gets refused, or with what text.
pub enum ProseRequest {
    Revise { id: String, text: String, why: Option<String>, cite: Option<String> },
    Heading { text: String, level: Option<u8>, before: Option<String> },
    Paragraph { text: String, cite: Option<String>, before: Option<String> },
}

pub enum ProseOutcome {
    Revised { id: String },
    Created(Block),
}

/// Dispatches a [`ProseRequest`] to [`create`] or [`revise`], refusing
/// with the exact text the CLI has always printed for a flag a mode
/// required but didn't get — lifted here (rather than left as a bare
/// `eprintln!` in `main.rs`) so the MCP server's structured refusals
/// carry the same guidance the CLI does, from the one place that decides
/// it.
pub fn dispatch(workspace_dir: &Path, req: ProseRequest) -> Result<ProseOutcome, AuthoringError> {
    match req {
        ProseRequest::Revise { id, text, why, cite } => {
            let why = why.ok_or_else(|| workspace::refuse(workspace_dir, "prose", "prose --revise requires --why"))?;
            let cite = cite.as_deref().map(parse_ids);
            revise(workspace_dir, &id, &text, &why, cite).map(|()| ProseOutcome::Revised { id })
        }
        ProseRequest::Heading { text, level, before } => {
            let level = level.ok_or_else(|| workspace::refuse(workspace_dir, "prose", "prose --heading requires --level"))?;
            create(workspace_dir, &text, None, Some(level), before.as_deref())
                .map(ProseOutcome::Created)
        }
        ProseRequest::Paragraph { text, cite, before } => {
            create(workspace_dir, &text, cite.as_deref(), None, before.as_deref())
                .map(ProseOutcome::Created)
        }
    }
}
