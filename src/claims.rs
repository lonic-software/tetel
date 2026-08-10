//! `tetel claim` — assert, revise, or withdraw a proposition resting on
//! facts. Refuses without `--cites` (a claim must cite at least one
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
use crate::workspace::{self, AuthoringError, Kind};

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

fn log_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("claims.jsonl")
}

pub fn load_all(workspace_dir: &Path) -> io::Result<Vec<Claim>> {
    let events: Vec<ClaimEvent> = workspace::read_jsonl(&log_path(workspace_dir))?;
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

pub fn exists(workspace_dir: &Path, id: &str) -> io::Result<bool> {
    Ok(load_all(workspace_dir)?.iter().any(|c| c.id == id))
}

fn parse_ids(csv: &str) -> Vec<String> {
    csv.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

pub struct CreateOutcome {
    pub claim: Claim,
    /// `(fact id, shared extent key(s))` for every non-cited fact sharing
    /// a designator with the cited ones — not the fact's note. The note
    /// can run to thousands of bytes and re-ships on every later create
    /// that overlaps the same fact; the key(s) say *why* the fact
    /// overlapped without repeating text the reader can already get from
    /// `tetel query facts`.
    pub overlap: Vec<(String, Vec<String>)>,
}

/// `tetel claim --proposition <text> --cites F1,F3`.
pub fn create(workspace_dir: &Path, prop: &str, from_csv: &str) -> Result<CreateOutcome, AuthoringError> {
    let ids = parse_ids(from_csv);
    if ids.is_empty() {
        return Err(workspace::refuse(
            workspace_dir,
            "claim",
            "a claim must rest on at least one fact (--cites was missing or empty); try `tetel query facts` to find one, or mint a new fact with `tetel look`/`tetel run` + `tetel fact`",
        ));
    }
    let mut missing = Vec::new();
    for id in &ids {
        if !facts::exists(workspace_dir, id)? {
            missing.push(id.clone());
        }
    }
    if !missing.is_empty() {
        return Err(workspace::refuse(
            workspace_dir,
            "claim",
            format!("cited fact(s) do not exist: {}; try `tetel query facts` to see what exists", missing.join(", ")),
        ));
    }
    if prop.trim().is_empty() {
        return Err(workspace::refuse(workspace_dir, "claim", "no --proposition given; a claim needs a proposition"));
    }

    let overlap = overlap_report(workspace_dir, &ids)?;

    let id = workspace::next_id(workspace_dir, Kind::Claim)?;
    let event = ClaimEvent::Create { id: id.clone(), prop: prop.to_string(), from: ids.clone(), timestamp: workspace::now_unix() };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;

    Ok(CreateOutcome { claim: Claim { id, prop: prop.to_string(), from: ids, withdrawn: false, revisions: 0 }, overlap })
}

/// Every fact not among `cited_ids` that shares an extent key with the
/// union of `cited_ids`'s own extents (fix 2: the key is a resolved
/// path where a designator names a file, so three different line-ranges
/// of one file, and a plain read of it, all overlap each other now,
/// where the prototype's literal-command-string key never caught this).
fn overlap_report(workspace_dir: &Path, cited_ids: &[String]) -> io::Result<Vec<(String, Vec<String>)>> {
    let all = facts::load_all(workspace_dir)?;
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
        // Dedup and sort: a fact can have several extent entries on the
        // same key (e.g. two `look --lines` ranges of one file), and the
        // report names each shared designator once.
        let shared: BTreeSet<&str> =
            f.extent.iter().map(|e| e.key.as_str()).filter(|k| union_keys.contains(k)).collect();
        if !shared.is_empty() {
            out.push((f.id.clone(), shared.into_iter().map(String::from).collect()));
        }
    }
    Ok(out)
}

/// `tetel claim --revise <id> --why <text> [--proposition <text>] [--cites ...]`.
/// At least one of `new_prop`/`new_from_csv` must be given — a revision
/// that changes nothing isn't a revision.
pub fn revise(
    workspace_dir: &Path,
    id: &str,
    new_prop: Option<&str>,
    new_from_csv: Option<&str>,
    why: &str,
) -> Result<(), AuthoringError> {
    if !exists(workspace_dir, id)? {
        return Err(workspace::refuse(workspace_dir, "claim", format!("no such claim: {id}")));
    }
    if why.trim().is_empty() {
        return Err(workspace::refuse(workspace_dir, "claim", "--revise requires --why (revisions must explain themselves)"));
    }
    let from = match new_from_csv {
        Some(csv) => {
            let ids = parse_ids(csv);
            let mut missing = Vec::new();
            for fid in &ids {
                if !facts::exists(workspace_dir, fid)? {
                    missing.push(fid.clone());
                }
            }
            if !missing.is_empty() {
                return Err(workspace::refuse(workspace_dir, "claim", format!("cited fact(s) do not exist: {}", missing.join(", "))));
            }
            Some(ids)
        }
        None => None,
    };
    let prop = new_prop.map(str::to_string);
    if prop.is_none() && from.is_none() {
        return Err(workspace::refuse(workspace_dir, "claim", "--revise requires a new --proposition and/or --cites"));
    }
    let event = ClaimEvent::Revise { id: id.to_string(), prop, from, why: why.to_string(), timestamp: workspace::now_unix() };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(())
}

/// `tetel claim --withdraw <id> --why <text>`.
pub fn withdraw(workspace_dir: &Path, id: &str, why: &str) -> Result<(), AuthoringError> {
    if !exists(workspace_dir, id)? {
        return Err(workspace::refuse(workspace_dir, "claim", format!("no such claim: {id}")));
    }
    if why.trim().is_empty() {
        return Err(workspace::refuse(workspace_dir, "claim", "--withdraw requires --why"));
    }
    let event = ClaimEvent::Withdraw { id: id.to_string(), why: why.to_string(), timestamp: workspace::now_unix() };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(())
}

/// What `tetel claim` was asked to do — create, revise or withdraw. The
/// single shape both the CLI and the MCP server build and pass to
/// [`dispatch`], so the two front ends can never drift on which
/// combination of missing flags gets refused, or with what text.
pub enum ClaimRequest {
    Create { prop: Option<String>, from: Option<String> },
    Revise { id: String, prop: Option<String>, from: Option<String>, why: Option<String> },
    Withdraw { id: String, why: Option<String> },
}

pub enum ClaimOutcome {
    Created(CreateOutcome),
    Revised { id: String },
    Withdrawn { id: String },
}

/// Dispatches a [`ClaimRequest`] to [`create`], [`revise`] or
/// [`withdraw`], refusing with the exact text the CLI has always printed
/// for a flag a mode required but didn't get — lifted here (rather than
/// left as a bare `eprintln!` in `main.rs`) so the MCP server's
/// structured refusals carry the same guidance the CLI does, from the
/// one place that decides it.
pub fn dispatch(workspace_dir: &Path, req: ClaimRequest) -> Result<ClaimOutcome, AuthoringError> {
    match req {
        ClaimRequest::Withdraw { id, why } => {
            let why = why.ok_or_else(|| workspace::refuse(workspace_dir, "claim", "claim --withdraw requires --why"))?;
            withdraw(workspace_dir, &id, &why).map(|()| ClaimOutcome::Withdrawn { id })
        }
        ClaimRequest::Revise { id, prop, from, why } => {
            let why = why.ok_or_else(|| workspace::refuse(workspace_dir, "claim", "claim --revise requires --why"))?;
            revise(workspace_dir, &id, prop.as_deref(), from.as_deref(), &why).map(|()| ClaimOutcome::Revised { id })
        }
        ClaimRequest::Create { prop, from } => {
            let prop = prop.ok_or_else(|| workspace::refuse(workspace_dir, "claim", "claim requires --proposition"))?;
            let from = from.ok_or_else(|| workspace::refuse(workspace_dir, "claim", "claim requires --cites"))?;
            create(workspace_dir, &prop, &from).map(ClaimOutcome::Created)
        }
    }
}
