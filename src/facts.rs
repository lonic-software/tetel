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
use crate::workspace::{self, AuthoringError, Kind};

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

fn log_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("facts.jsonl")
}

pub fn load_all(workspace_dir: &Path) -> io::Result<Vec<Fact>> {
    let events: Vec<FactEvent> = workspace::read_jsonl(&log_path(workspace_dir))?;
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

pub fn exists(workspace_dir: &Path, id: &str) -> io::Result<bool> {
    Ok(load_all(workspace_dir)?.iter().any(|f| f.id == id))
}

pub fn get(workspace_dir: &Path, id: &str) -> io::Result<Option<Fact>> {
    Ok(load_all(workspace_dir)?.into_iter().find(|f| f.id == id))
}

/// How old each observation in the buffer is, at the moment it is folded.
///
/// `fact` prints this because a buffer is not necessarily what you just
/// looked at. A revision does not clear it and a failed `look` does not
/// add to it, so a mint can fold an observation from an earlier line of
/// enquiry — which happened, silently, producing a fact whose note
/// described two source files its extent never covered.
///
/// Reported rather than refused, and without a staleness threshold: there
/// is no age at which folding an earlier observation becomes wrong, and a
/// number chosen here would be arbitrary. What was missing was not a rule
/// but the fact being visible at all — an author who expected two file
/// reads and is told they folded one four-minute-old command has what
/// they need.
pub fn describe_buffer(entries: &[crate::pending::PendingEntry], now: u64) -> Vec<String> {
    entries
        .iter()
        .map(|e| {
            let age = now.saturating_sub(e.captured_at);
            let when = if e.captured_at == 0 {
                "age unknown".to_string()
            } else if age < 60 {
                format!("{age}s ago")
            } else {
                format!("{}m ago", age / 60)
            };
            format!("{} ({when})", e.label)
        })
        .collect()
}

/// When this workspace last minted a fact, or `None` if it never has.
///
/// Reads the raw event log rather than [`load_all`], which discards
/// `Create`'s timestamp when building its in-memory view. The timestamp
/// is in the shipped file the whole time; only the view drops it.
pub fn last_mint_timestamp(workspace_dir: &Path) -> Option<u64> {
    let events: Vec<FactEvent> = workspace::read_jsonl(&log_path(workspace_dir)).ok()?;
    events
        .iter()
        .filter_map(|e| match e {
            FactEvent::Create { timestamp, .. } => Some(*timestamp),
            FactEvent::Revise { .. } => None,
        })
        .max()
}

/// Every refusal this workspace recorded since its last mint, verbatim.
///
/// # Why a mint reports what it could not do
///
/// [`describe_buffer`] says what a mint *folded*. It cannot say why what
/// the author expected to fold is missing — and that gap is the whole
/// incident this exists for: two `look` calls were refused for a
/// malformed line range, each left the pending buffer untouched, and the
/// next mint folded a leftover observation from an earlier line of
/// enquiry. The author saw a folding line with an age on it and nothing
/// else. The refusals had happened, and nothing the tool ever consults
/// had recorded them.
///
/// So the two halves are complementary and both are needed: what was
/// folded, and what was refused in the window that produced it. Neither
/// is a refusal itself, neither is counted, and neither carries a
/// threshold — a mint following a refusal is often entirely correct.
///
/// One helper, called by both surfaces, so CLI and MCP cannot drift on
/// which refusals a mint reports.
pub fn refusals_since_last_mint(workspace_dir: &Path) -> Vec<String> {
    workspace::refusals_since(workspace_dir, last_mint_timestamp(workspace_dir))
}

/// One fact's mint window: the refusals recorded between the previous
/// mint and this one.
pub struct MintWindow {
    pub fact_id: String,
    pub refusals: Vec<String>,
    /// The first fact in the workspace, whose window has no previous mint
    /// to open at and so runs from the beginning of the log.
    pub is_first: bool,
    /// At least one of these refusals shares a second with a mint
    /// boundary, so it cannot be placed on one side of it. Such a refusal
    /// appears in both adjacent windows — the same `>=` choice the
    /// mint-time replay makes, for the same reason: showing twice is
    /// recoverable, hiding once is the incident this exists for.
    pub straddles_a_boundary: bool,
}

/// Every fact whose mint window contains a refusal, in mint order.
///
/// # Why this exists beside the mint-time line
///
/// [`refusals_since_last_mint`] prints at the moment of minting, and that
/// is the only moment the defect is *preventable* rather than merely
/// discoverable — a fact's extent is unrevisable, so nothing later can
/// correct the evidence, only the note.
///
/// But a line printed at a terminal works only if it is read in the
/// moment, and a memo built on a bad fact otherwise renders and checks
/// clean. This recovers the same signal at grading time, when nobody is
/// relying on the author's attention: `FactEvent::Create` carries a
/// timestamp that ships in `facts.jsonl`, and the snapshot copies
/// `refusals.log` beside it, so the windows are derivable from files
/// every snapshot already contains. No schema change, nothing new stored.
///
/// Human-owed and verbatim: a mint following a refusal is frequently
/// correct, and which of them matters is a reader's call. Nothing here
/// counts, scores or ranks.
///
/// The listing is only ever as complete as the log beneath it — a
/// snapshot written by a build that did not record surface-layer
/// refusals simply has fewer lines, and this is silent about them. That
/// is not a completeness claim in either direction: a verbatim listing
/// asserts only what was recorded.
pub fn mint_windows(snapshot_dir: &Path) -> Vec<MintWindow> {
    let Ok(events) = workspace::read_jsonl::<FactEvent>(&log_path(snapshot_dir)) else {
        return Vec::new();
    };
    let mints: Vec<(String, u64)> = events
        .iter()
        .filter_map(|e| match e {
            FactEvent::Create { id, timestamp, .. } => Some((id.clone(), *timestamp)),
            FactEvent::Revise { .. } => None,
        })
        .collect();

    let mut out = Vec::new();
    for (i, (id, minted_at)) in mints.iter().enumerate() {
        // The window opens at the previous mint — or at the beginning of
        // the log for the first fact, which owns everything refused
        // before it.
        let opened = if i == 0 { None } else { Some(mints[i - 1].1) };
        let stamped: Vec<(u64, String)> = workspace::refusals_since(snapshot_dir, opened)
            .into_iter()
            .filter_map(|line| {
                let ts = line.split_once('\t').and_then(|(ts, _)| ts.parse::<u64>().ok())?;
                (ts <= *minted_at).then_some((ts, line))
            })
            .collect();
        if !stamped.is_empty() {
            let straddles_a_boundary = stamped
                .iter()
                .any(|(ts, _)| *ts == *minted_at || opened.is_some_and(|o| *ts == o));
            out.push(MintWindow {
                fact_id: id.clone(),
                refusals: stamped.into_iter().map(|(_, line)| line).collect(),
                is_first: i == 0,
                straddles_a_boundary,
            });
        }
    }
    out
}

/// Mint a fact from the current pending buffer. Refuses if the buffer
/// is empty (a fact needs a preceding `look`/`run`) or the note text is
/// empty. Clears the buffer on success, exactly once, after the log
/// append succeeds — never before.
pub fn mint(workspace_dir: &Path, note: &str) -> Result<Fact, AuthoringError> {
    if note.trim().is_empty() {
        return Err(workspace::refuse(workspace_dir, "fact", "note text is empty"));
    }
    let buf = pending::load(workspace_dir)?;
    if buf.is_empty() {
        return Err(workspace::refuse(
            workspace_dir,
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

    let id = workspace::next_id(workspace_dir, Kind::Fact)?;
    let event = FactEvent::Create {
        id: id.clone(),
        note: note.to_string(),
        extent: extent.clone(),
        output: output.clone(),
        pin: pin.clone(),
        timestamp: workspace::now_unix(),
    };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    pending::clear(workspace_dir)?;

    Ok(Fact { id, note: note.to_string(), extent, output, pin, revisions: 0 })
}

/// `tetel fact --revise <id> --note <new-text> --why <text>`.
pub fn revise(workspace_dir: &Path, id: &str, new_note: &str, why: &str) -> Result<(), AuthoringError> {
    if !exists(workspace_dir, id)? {
        return Err(workspace::refuse(workspace_dir, "fact", format!("no such fact: {id}")));
    }
    if why.trim().is_empty() {
        return Err(workspace::refuse(workspace_dir, "fact", "--revise requires --why (revisions must explain themselves)"));
    }
    if new_note.trim().is_empty() {
        return Err(workspace::refuse(workspace_dir, "fact", "no new --note text given"));
    }
    let event =
        FactEvent::Revise { id: id.to_string(), note: new_note.to_string(), why: why.to_string(), timestamp: workspace::now_unix() };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(())
}

/// What `tetel fact` was asked to do — mint a new fact from the pending
/// buffer, or revise an existing one's note. The single shape both the
/// CLI and the MCP server build and pass to [`dispatch`], so the two
/// front ends can never drift on which combination of missing flags gets
/// refused, or with what text.
pub enum FactRequest {
    Mint { note: Option<String> },
    Revise { id: String, note: Option<String>, why: Option<String> },
}

pub enum FactOutcome {
    Minted(Fact),
    Revised { id: String },
}

/// Dispatches a [`FactRequest`] to [`mint`] or [`revise`], refusing with
/// the exact text the CLI has always printed for a flag a mode required
/// but didn't get — lifted here (rather than left as a bare `eprintln!`
/// in `main.rs`) so the MCP server's structured refusals carry the same
/// guidance the CLI does, from the one place that decides it.
pub fn dispatch(workspace_dir: &Path, req: FactRequest) -> Result<FactOutcome, AuthoringError> {
    match req {
        FactRequest::Mint { note } => {
            let note = note.ok_or_else(|| workspace::refuse(workspace_dir, "fact", "fact requires --note"))?;
            mint(workspace_dir, &note).map(FactOutcome::Minted)
        }
        FactRequest::Revise { id, note, why } => {
            let why = why.ok_or_else(|| workspace::refuse(workspace_dir, "fact", "fact --revise requires --why"))?;
            let note = note.ok_or_else(|| {
                workspace::refuse(workspace_dir, "fact", "fact --revise requires --note (the new note text)")
            })?;
            revise(workspace_dir, &id, &note, &why).map(|()| FactOutcome::Revised { id })
        }
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tetel-window-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Facts as raw `Create` events at chosen timestamps, plus a refusal
    /// log at chosen timestamps — the two files a snapshot ships. Written
    /// by hand because the whole point is controlling the seconds, which
    /// an end-to-end run cannot do.
    fn seed(dir: &Path, mints: &[(&str, u64)], refusals: &[(u64, &str)]) {
        let facts: String = mints
            .iter()
            .map(|(id, ts)| {
                format!(
                    r#"{{"event":"Create","id":"{id}","note":"n","extent":[],"output":"","pin":"p","timestamp":{ts}}}"#
                ) + "\n"
            })
            .collect();
        std::fs::write(dir.join("facts.jsonl"), facts).unwrap();
        let log: String =
            refusals.iter().map(|(ts, r)| format!("{ts}\tlook\t{r}\n")).collect();
        std::fs::write(dir.join("refusals.log"), log).unwrap();
    }

    #[test]
    fn each_refusal_lands_in_the_window_of_the_mint_that_followed_it() {
        let dir = scratch("attribution");
        seed(&dir, &[("F1", 100), ("F2", 200)], &[(50, "before F1"), (150, "between F1 and F2")]);

        let w = mint_windows(&dir);
        assert_eq!(w.len(), 2, "both facts have a refusal in their window");
        assert_eq!(w[0].fact_id, "F1");
        assert!(w[0].is_first, "F1 has no previous mint to open at");
        assert!(w[0].refusals[0].contains("before F1"));
        assert_eq!(w[0].refusals.len(), 1, "F1 must not claim a refusal that followed it");
        assert_eq!(w[1].fact_id, "F2");
        assert!(!w[1].is_first);
        assert!(w[1].refusals[0].contains("between F1 and F2"));
        assert_eq!(w[1].refusals.len(), 1);
        assert!(!w[0].straddles_a_boundary && !w[1].straddles_a_boundary, "no shared seconds here");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A refusal sharing a second with a mint cannot be placed on one
    /// side of it. It is listed under both adjacent facts and the report
    /// says why, rather than a guess being made silently — the same
    /// choice the mint-time replay makes.
    #[test]
    fn a_refusal_sharing_a_second_with_a_mint_is_flagged_not_guessed() {
        let dir = scratch("straddle");
        seed(&dir, &[("F1", 100), ("F2", 200)], &[(100, "same second as F1")]);

        let w = mint_windows(&dir);
        assert_eq!(w.len(), 2, "listed under both, since which side it fell on is unknowable");
        assert!(w[0].straddles_a_boundary, "F1's window closes at this second");
        assert!(w[1].straddles_a_boundary, "F2's window opens at it");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fact with nothing refused in its window is not listed at all. An
    /// entry per fact saying "none" is noise, and noise is what makes a
    /// real one invisible.
    #[test]
    fn a_fact_with_a_clean_window_is_absent_rather_than_reported_empty() {
        let dir = scratch("clean");
        seed(&dir, &[("F1", 100), ("F2", 200)], &[(150, "only in F2's window")]);
        let w = mint_windows(&dir);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].fact_id, "F2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A snapshot from a build that never recorded surface-layer
    /// refusals has no log; the listing is silent rather than absent-
    /// meaning-clean. A verbatim listing asserts only what was recorded.
    #[test]
    fn a_snapshot_with_no_refusal_log_yields_no_windows() {
        let dir = scratch("nolog");
        seed(&dir, &[("F1", 100)], &[]);
        std::fs::remove_file(dir.join("refusals.log")).unwrap();
        assert!(mint_windows(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
