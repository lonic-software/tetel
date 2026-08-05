//! `tetel claim` — assert, revise, or withdraw a proposition resting on
//! facts. Refuses without `--from` (a claim must cite at least one
//! fact) and refuses on an unknown fact id. Before accepting a new
//! claim, prints an overlap report: existing facts, not among those
//! cited, that share an overlap key (see `pending.rs`'s doc comment on
//! fix 2) with the ones being cited — this is how the tool surfaces
//! evidence the author may have forgotten they already had.
//!
//! `--revise`/`--withdraw` are append-only, mirroring `tclaim`: both
//! refuse without `--why`, and a revision keeps the old proposition in
//! the log rather than overwriting it.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::facts;
use crate::session::{self, AuthoringError, Kind};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum ClaimEvent {
    Create {
        id: String,
        prop: String,
        from: Vec<String>,
        timestamp: u64,
    },
    Revise {
        id: String,
        #[serde(default)]
        prop: Option<String>,
        #[serde(default)]
        from: Option<Vec<String>>,
        why: String,
        timestamp: u64,
    },
    Withdraw {
        id: String,
        why: String,
        timestamp: u64,
    },
}

pub struct Claim {
    pub id: String,
    pub prop: String,
    pub from: Vec<String>,
    pub withdrawn: bool,
    pub revisions: usize,
}

fn log_path(session_dir: &Path) -> PathBuf {
    session_dir.join("claims.jsonl")
}

pub fn load_all(session_dir: &Path) -> io::Result<Vec<Claim>> {
    let events: Vec<ClaimEvent> = session::read_jsonl(&log_path(session_dir))?;
    let mut by_id: std::collections::BTreeMap<String, Claim> = std::collections::BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for ev in events {
        match ev {
            ClaimEvent::Create { id, prop, from, .. } => {
                order.push(id.clone());
                by_id.insert(id.clone(), Claim { id, prop, from, withdrawn: false, revisions: 0 });
            }
            ClaimEvent::Revise { id, prop, from, .. } => {
                if let Some(c) = by_id.get_mut(&id) {
                    if let Some(p) = prop {
                        c.prop = p;
                    }
                    if let Some(f) = from {
                        c.from = f;
                    }
                    c.revisions += 1;
                }
            }
            ClaimEvent::Withdraw { id, .. } => {
                if let Some(c) = by_id.get_mut(&id) {
                    c.withdrawn = true;
                }
            }
        }
    }
    Ok(order.into_iter().filter_map(|id| by_id.remove(&id)).collect())
}

pub fn exists(session_dir: &Path, id: &str) -> io::Result<bool> {
    Ok(load_all(session_dir)?.iter().any(|c| c.id == id))
}

fn parse_ids(csv: &str) -> Vec<String> {
    csv.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

pub struct CreateOutcome {
    pub claim: Claim,
    /// `(fact id, current note)` for every non-cited fact sharing a
    /// designator with the cited ones.
    pub overlap: Vec<(String, String)>,
}

/// `tetel claim --prop <text> --from F1,F3`.
pub fn create(session_dir: &Path, prop: &str, from_csv: &str) -> Result<CreateOutcome, AuthoringError> {
    let ids = parse_ids(from_csv);
    if ids.is_empty() {
        return Err(session::refuse(
            session_dir,
            "claim",
            "a claim must rest on at least one fact (--from was missing or empty); try `tetel query facts` to find one, or mint a new fact with `tetel look`/`tetel run` + `tetel fact`",
        ));
    }
    let mut missing = Vec::new();
    for id in &ids {
        if !facts::exists(session_dir, id)? {
            missing.push(id.clone());
        }
    }
    if !missing.is_empty() {
        return Err(session::refuse(
            session_dir,
            "claim",
            format!("cited fact(s) do not exist: {}; try `tetel query facts` to see what exists", missing.join(", ")),
        ));
    }
    if prop.trim().is_empty() {
        return Err(session::refuse(session_dir, "claim", "no --prop given; a claim needs a proposition"));
    }

    let overlap = overlap_report(session_dir, &ids)?;

    let id = session::next_id(session_dir, Kind::Claim)?;
    let event = ClaimEvent::Create { id: id.clone(), prop: prop.to_string(), from: ids.clone(), timestamp: session::now_unix() };
    session::append_jsonl(&log_path(session_dir), &event)?;

    Ok(CreateOutcome { claim: Claim { id, prop: prop.to_string(), from: ids, withdrawn: false, revisions: 0 }, overlap })
}

/// Every fact not among `cited_ids` that shares an extent key with the
/// union of `cited_ids`'s own extents (fix 2: the key is a resolved
/// path where a designator names a file, so three different line-ranges
/// of one file, and a plain read of it, all overlap each other now,
/// where the prototype's literal-command-string key never caught this).
fn overlap_report(session_dir: &Path, cited_ids: &[String]) -> io::Result<Vec<(String, String)>> {
    let all = facts::load_all(session_dir)?;
    let cited: BTreeSet<&str> = cited_ids.iter().map(String::as_str).collect();
    let mut union_keys: BTreeSet<&str> = BTreeSet::new();
    for f in &all {
        if cited.contains(f.id.as_str()) {
            for e in &f.extent {
                union_keys.insert(e.key.as_str());
            }
        }
    }
    let mut out = Vec::new();
    for f in &all {
        if cited.contains(f.id.as_str()) {
            continue;
        }
        if f.extent.iter().any(|e| union_keys.contains(e.key.as_str())) {
            out.push((f.id.clone(), f.note.clone()));
        }
    }
    Ok(out)
}

/// `tetel claim --revise <id> --why <text> [--prop <text>] [--from ...]`.
/// At least one of `new_prop`/`new_from_csv` must be given — a revision
/// that changes nothing isn't a revision.
pub fn revise(
    session_dir: &Path,
    id: &str,
    new_prop: Option<&str>,
    new_from_csv: Option<&str>,
    why: &str,
) -> Result<(), AuthoringError> {
    if !exists(session_dir, id)? {
        return Err(session::refuse(session_dir, "claim", format!("no such claim: {id}")));
    }
    if why.trim().is_empty() {
        return Err(session::refuse(session_dir, "claim", "--revise requires --why (revisions must explain themselves)"));
    }
    let from = match new_from_csv {
        Some(csv) => {
            let ids = parse_ids(csv);
            let mut missing = Vec::new();
            for fid in &ids {
                if !facts::exists(session_dir, fid)? {
                    missing.push(fid.clone());
                }
            }
            if !missing.is_empty() {
                return Err(session::refuse(session_dir, "claim", format!("cited fact(s) do not exist: {}", missing.join(", "))));
            }
            Some(ids)
        }
        None => None,
    };
    let prop = new_prop.map(str::to_string);
    if prop.is_none() && from.is_none() {
        return Err(session::refuse(session_dir, "claim", "--revise requires a new --prop and/or --from"));
    }
    let event = ClaimEvent::Revise { id: id.to_string(), prop, from, why: why.to_string(), timestamp: session::now_unix() };
    session::append_jsonl(&log_path(session_dir), &event)?;
    Ok(())
}

/// `tetel claim --withdraw <id> --why <text>`.
pub fn withdraw(session_dir: &Path, id: &str, why: &str) -> Result<(), AuthoringError> {
    if !exists(session_dir, id)? {
        return Err(session::refuse(session_dir, "claim", format!("no such claim: {id}")));
    }
    if why.trim().is_empty() {
        return Err(session::refuse(session_dir, "claim", "--withdraw requires --why"));
    }
    let event = ClaimEvent::Withdraw { id: id.to_string(), why: why.to_string(), timestamp: session::now_unix() };
    session::append_jsonl(&log_path(session_dir), &event)?;
    Ok(())
}
