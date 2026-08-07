//! `tetel target <symbol> --cites F#` — declare a symbol this design
//! tells an implementer to modify, and point at the census that
//! establishes where it is used.
//!
//! # Why the trigger is a declaration
//!
//! The rule this implements was first written as "refuse a memo whose
//! recommendation names an uncensused symbol". That has no decidable
//! trigger. Nothing in [`crate::claims`] or [`crate::prose`] types a
//! record as a recommendation — a claim is a proposition with fact
//! citations, a prose block is text with claim citations — so "the
//! memo's recommendations" denotes no set a check can enumerate, and
//! deciding from English that a sentence tells an implementer to modify
//! something is precisely the heuristic read the project rules out as a
//! refusal trigger.
//!
//! The obvious repair — trigger on any symbol a claim's proposition
//! mentions — is not merely off-charter, it is already measured dead in
//! this crate. [`crate::scope`] tried string-matching symbol references
//! and rejected the detector: it flagged a legitimate quotation and
//! missed both real overreaches. Naming a symbol the code you read calls
//! is normal; concluding about it is the failure; string matching cannot
//! tell them apart, and a refusal wired to it would make honest memos
//! unwritable.
//!
//! So the trigger is a declaration, and the refusal never reads English
//! at all.
//!
//! # Where the refusal lives, and why here
//!
//! At declaration — the earliest surface that can decide it. `target`
//! refuses a symbol whose cited fact carries no qualifying census entry,
//! the same authoring-time shape as [`crate::facts`] refusing an empty
//! pending buffer.
//!
//! A fact's extent is immutable once minted and a target is immutable
//! once declared, so this one refusal makes *every declared target is
//! censused* a workspace invariant. `render` therefore needs no refusal
//! of its own: its job is to emit the section. What `render` cannot
//! vouch for — a hand-authored or tampered document that never passed
//! through this verb — [`crate::checks`] re-verifies against the
//! shipped snapshot.
//!
//! This is the report-versus-refuse discriminator from the other side
//! than [`crate::scope`] landed on. That check stayed human-owed because
//! only a person can tell context from conclusion. Here nothing is left
//! to tell: the remedy is singular. Run the workspace-rooted grep, mint
//! the fact, cite it — or withdraw the target.
//!
//! # What declaration cannot reach
//!
//! Under-declaration. Deciding that a memo recommended something it
//! never declared is the same heuristic read the trigger just rejected,
//! so no refusal can reach it and none is attempted. The answer is
//! visibility instead: the rendered section does not disappear when it
//! is empty, so a memo full of tell-the-implementer language with an
//! empty census section shows its hole to every reader — and `check`'s
//! standing non-coverage list says the blind spot is there.
//!
//! # Withdrawable, not revisable
//!
//! A changed symbol is a different census, so the honest edit is
//! withdraw and redeclare rather than revise. There is deliberately no
//! `Revise` variant: a revision that could change the symbol while
//! keeping the citation would break the invariant this module exists to
//! establish.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::facts;
use crate::workspace::{self, AuthoringError, Kind};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum TargetEvent {
    Declare {
        id: String,
        symbol: String,
        /// The fact whose extent censuses `symbol`. One fact, not a
        /// list: the predicate is about a single search, and a target
        /// resting on "one of these several facts" would leave which one
        /// undecided.
        from: String,
        timestamp: u64,
    },
    Withdraw {
        id: String,
        why: String,
        timestamp: u64,
    },
}

#[derive(Debug, Clone)]
pub struct Target {
    pub id: String,
    pub symbol: String,
    pub from: String,
    pub withdrawn: bool,
}

fn log_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("targets.jsonl")
}

pub fn load_all(workspace_dir: &Path) -> io::Result<Vec<Target>> {
    let events: Vec<TargetEvent> = workspace::read_jsonl(&log_path(workspace_dir))?;
    let mut by_id: std::collections::BTreeMap<String, Target> = std::collections::BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for ev in events {
        match ev {
            TargetEvent::Declare { id, symbol, from, .. } => {
                order.push(id.clone());
                by_id.insert(id.clone(), Target { id, symbol, from, withdrawn: false });
            }
            TargetEvent::Withdraw { id, .. } => {
                if let Some(t) = by_id.get_mut(&id) {
                    t.withdrawn = true;
                }
            }
        }
    }
    Ok(order.into_iter().filter_map(|id| by_id.remove(&id)).collect())
}

/// `tetel target <symbol> --cites F1`.
///
/// Refuses unless the cited fact's captured extent contains a
/// whole-search record whose pattern byte-equals `symbol` and which was
/// rooted at the worktree it was taken against — see
/// [`facts::ExtentEntry::censuses`].
pub fn declare(workspace_dir: &Path, symbol: &str, from: &str) -> Result<Target, AuthoringError> {
    let symbol = symbol.trim();
    if symbol.is_empty() {
        return Err(workspace::refuse(workspace_dir, "target", "no symbol given; a modification target names one symbol"));
    }
    let from = from.trim();
    if from.is_empty() {
        return Err(workspace::refuse(
            workspace_dir,
            "target",
            format!(
                "a modification target must cite the fact that censuses it (--cites was missing); capture one with `tetel look --grep {symbol} <worktree root>` then `tetel fact`"
            ),
        ));
    }

    let all = facts::load_all(workspace_dir)?;
    let Some(fact) = all.iter().find(|f| f.id == from) else {
        return Err(workspace::refuse(
            workspace_dir,
            "target",
            format!("cited fact does not exist: {from}; try `tetel query facts` to see what exists"),
        ));
    };

    if !fact.extent.iter().any(|e| e.censuses(symbol)) {
        // Say which of the three ways it failed, because the remedies
        // differ and a bare refusal would send the author guessing.
        let why = diagnose(fact, symbol);
        return Err(workspace::refuse(
            workspace_dir,
            "target",
            format!(
                "{from} does not census `{symbol}`: {why}\n\
                 a census is one search of the whole worktree for the symbol itself:\n\
                 \x20   tetel look --grep {symbol} <worktree root>\n\
                 \x20   tetel fact --note \"...\"\n\
                 then cite that fact. The refusal is about the search's existence and where it was rooted, never about what it found."
            ),
        ));
    }

    let id = workspace::next_id(workspace_dir, Kind::Target)?;
    let event = TargetEvent::Declare {
        id: id.clone(),
        symbol: symbol.to_string(),
        from: from.to_string(),
        timestamp: workspace::now_unix(),
    };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(Target { id, symbol: symbol.to_string(), from: from.to_string(), withdrawn: false })
}

/// Which half of the predicate the cited fact failed, in the author's
/// terms. Never a score and never a guess — each branch reports a
/// property that was read off the record.
fn diagnose(fact: &facts::Fact, symbol: &str) -> String {
    use crate::pending::ObservationKind::{NoMatch, Search};
    let searches: Vec<&facts::ExtentEntry> =
        fact.extent.iter().filter(|e| matches!(e.kind, Some(Search) | Some(NoMatch))).collect();

    if searches.is_empty() {
        if fact.extent.iter().any(|e| e.kind.is_none()) {
            return "its extent was minted before searches recorded where they were rooted, so it cannot be read as a census — re-run the grep".to_string();
        }
        return "its extent contains no search at all — it was read or run, not grepped".to_string();
    }
    let patterns: Vec<&str> = searches.iter().map(|e| e.pattern.as_str()).filter(|p| !p.is_empty()).collect();
    if !patterns.iter().any(|p| *p == symbol) {
        return format!(
            "it searched for {} — a census must search for the symbol itself, byte for byte",
            patterns.iter().map(|p| format!("`{p}`")).collect::<Vec<_>>().join(", ")
        );
    }
    for e in &searches {
        if e.pattern == symbol {
            if e.world_root.is_empty() || e.world_root == crate::worldstate::NO_GIT {
                return "its search was not taken against a git worktree, so `rooted at the worktree` has no answer".to_string();
            }
            return format!(
                "its search was rooted at {}, not at the worktree root {} — so it cannot support a statement about every use of the symbol",
                e.key, e.world_root
            );
        }
    }
    unreachable!("a matching pattern was found above")
}

/// `tetel target --withdraw <id> --why <text>`.
pub fn withdraw(workspace_dir: &Path, id: &str, why: &str) -> Result<Target, AuthoringError> {
    if why.trim().is_empty() {
        return Err(workspace::refuse(workspace_dir, "target", "--withdraw requires --why"));
    }
    let all = load_all(workspace_dir)?;
    let Some(t) = all.iter().find(|t| t.id == id) else {
        return Err(workspace::refuse(workspace_dir, "target", format!("no target `{id}` in this workspace")));
    };
    if t.withdrawn {
        return Err(workspace::refuse(workspace_dir, "target", format!("{id} is already withdrawn")));
    }
    let event = TargetEvent::Withdraw { id: id.to_string(), why: why.to_string(), timestamp: workspace::now_unix() };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(Target { id: t.id.clone(), symbol: t.symbol.clone(), from: t.from.clone(), withdrawn: true })
}

/// What `tetel target` was asked to do. The single shape both the CLI
/// and the MCP server build and pass to [`dispatch`], so the two front
/// ends cannot drift on which act they performed — this render path has
/// already recorded one CLI/MCP drift on a duplicated act.
pub enum TargetRequest {
    Declare { symbol: Option<String>, from: Option<String> },
    Withdraw { id: String, why: Option<String> },
}

pub enum TargetOutcome {
    Declared(Target),
    Withdrawn { id: String },
}

pub fn dispatch(workspace_dir: &Path, req: TargetRequest) -> Result<TargetOutcome, AuthoringError> {
    match req {
        TargetRequest::Declare { symbol, from } => {
            let symbol = symbol.unwrap_or_default();
            let from = from.unwrap_or_default();
            declare(workspace_dir, &symbol, &from).map(TargetOutcome::Declared)
        }
        TargetRequest::Withdraw { id, why } => {
            let why = why.unwrap_or_default();
            withdraw(workspace_dir, &id, &why).map(|t| TargetOutcome::Withdrawn { id: t.id })
        }
    }
}
