//! `tetel fact` — mints a fact from the pending observation buffer (see
//! `pending.rs`). Refuses on an empty buffer. There is deliberately no
//! flag anywhere on this module's CLI surface that lets a caller supply
//! the extent or captured output themselves — that absence is the
//! guarantee that a fact was actually backed by a `look`/`run`, not a
//! rule enforced after the fact.
//!
//! `tetel fact --revise <id> --why <text>` rewrites the note only,
//! append-only: extent, output and pin are set once at mint time and
//! never appear in a revision event at all, so there is no code path
//! that could change them later either.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::evidence::sha256_hex;
use crate::pending;
use crate::session::{self, AuthoringError, Kind};

/// One observation folded into a fact's extent — the label a human
/// reads, the key overlap detection compares, and the world-state
/// marker it was captured under (fix 1 in the design memo).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtentEntry {
    pub key: String,
    pub label: String,
    pub world_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum FactEvent {
    Create {
        id: String,
        note: String,
        extent: Vec<ExtentEntry>,
        output: String,
        pin: String,
        timestamp: u64,
    },
    /// Append-only note revision. No `extent`, `output` or `pin` field
    /// exists on this variant at all — see the module doc comment.
    Revise {
        id: String,
        note: String,
        why: String,
        timestamp: u64,
    },
}

/// A fact's current view, replayed from its event log: the original
/// extent/output/pin (never revisable) plus whichever note is current.
pub struct Fact {
    pub id: String,
    pub note: String,
    pub extent: Vec<ExtentEntry>,
    pub output: String,
    pub pin: String,
    pub revisions: usize,
}

fn log_path(session_dir: &Path) -> PathBuf {
    session_dir.join("facts.jsonl")
}

pub fn load_all(session_dir: &Path) -> io::Result<Vec<Fact>> {
    let events: Vec<FactEvent> = session::read_jsonl(&log_path(session_dir))?;
    let mut by_id: std::collections::BTreeMap<String, Fact> = std::collections::BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for ev in events {
        match ev {
            FactEvent::Create { id, note, extent, output, pin, .. } => {
                order.push(id.clone());
                by_id.insert(id.clone(), Fact { id, note, extent, output, pin, revisions: 0 });
            }
            FactEvent::Revise { id, note, .. } => {
                if let Some(f) = by_id.get_mut(&id) {
                    f.note = note;
                    f.revisions += 1;
                }
            }
        }
    }
    Ok(order.into_iter().filter_map(|id| by_id.remove(&id)).collect())
}

pub fn exists(session_dir: &Path, id: &str) -> io::Result<bool> {
    Ok(load_all(session_dir)?.iter().any(|f| f.id == id))
}

pub fn get(session_dir: &Path, id: &str) -> io::Result<Option<Fact>> {
    Ok(load_all(session_dir)?.into_iter().find(|f| f.id == id))
}

/// Mint a fact from the current pending buffer. Refuses if the buffer
/// is empty (a fact needs a preceding `look`/`run`) or the note text is
/// empty. Clears the buffer on success, exactly once, after the log
/// append succeeds — never before.
pub fn mint(session_dir: &Path, note: &str) -> Result<Fact, AuthoringError> {
    if note.trim().is_empty() {
        return Err(session::refuse(session_dir, "fact", "note text is empty"));
    }
    let buf = pending::load(session_dir)?;
    if buf.is_empty() {
        return Err(session::refuse(
            session_dir,
            "fact",
            "pending observation buffer is empty; run `tetel look` or `tetel run` first, then `tetel fact`",
        ));
    }

    let extent: Vec<ExtentEntry> = buf
        .iter()
        .map(|e| ExtentEntry { key: e.key.clone(), label: e.label.clone(), world_state: e.world_state.clone() })
        .collect();
    let output = buf.iter().filter(|e| !e.output.is_empty()).map(|e| e.output.as_str()).collect::<Vec<_>>().join("\n");

    // The pin: a content fingerprint over every entry's label, output
    // and world-state marker — the prototype's compute_pin hashed
    // extent labels and captured output; folding the world-state marker
    // in too means a fact minted against a different tree state pins
    // differently even if its labels/output happen to coincide. The
    // marker itself is still carried separately in `extent` (see
    // `ExtentEntry`), since a hash alone can't be compared by a human
    // without recomputing it — fix 1 is about visibility, not just
    // detectability.
    let mut hash_input = String::new();
    for e in &buf {
        hash_input.push_str(&e.label);
        hash_input.push('\n');
        hash_input.push_str(&e.world_state);
        hash_input.push('\n');
        hash_input.push_str(&e.output);
        hash_input.push('\n');
    }
    let pin = format!("sha256:{}", sha256_hex(&hash_input));

    let id = session::next_id(session_dir, Kind::Fact)?;
    let event = FactEvent::Create {
        id: id.clone(),
        note: note.to_string(),
        extent: extent.clone(),
        output: output.clone(),
        pin: pin.clone(),
        timestamp: session::now_unix(),
    };
    session::append_jsonl(&log_path(session_dir), &event)?;
    pending::clear(session_dir)?;

    Ok(Fact { id, note: note.to_string(), extent, output, pin, revisions: 0 })
}

/// `tetel fact --revise <id> --note <new-text> --why <text>`.
pub fn revise(session_dir: &Path, id: &str, new_note: &str, why: &str) -> Result<(), AuthoringError> {
    if !exists(session_dir, id)? {
        return Err(session::refuse(session_dir, "fact", format!("no such fact: {id}")));
    }
    if why.trim().is_empty() {
        return Err(session::refuse(session_dir, "fact", "--revise requires --why (revisions must explain themselves)"));
    }
    if new_note.trim().is_empty() {
        return Err(session::refuse(session_dir, "fact", "no new --note text given"));
    }
    let event =
        FactEvent::Revise { id: id.to_string(), note: new_note.to_string(), why: why.to_string(), timestamp: session::now_unix() };
    session::append_jsonl(&log_path(session_dir), &event)?;
    Ok(())
}
