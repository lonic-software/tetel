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
/// reads, the key overlap detection compares, and the world-tree marker
/// it was captured under (fix 1 in the design memo).
///
/// The marker is two fields, not one. `world_state` alone cannot be
/// compared across observations of different repositories, and half the
/// workspaces in the store on 2026-08-07 read more than one; only
/// "same `world_root`, different `world_state`" is a finding. See
/// `worldstate.rs`.
///
/// `world_root` is `#[serde(default)]` for entries minted before it
/// existed. An empty root is not "unknown repository" — it marks a state
/// that was resolved from the process's working directory rather than
/// from what the observation touched, and is therefore comparable with
/// nothing. `check` treats it that way rather than guessing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtentEntry {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub world_root: String,
    pub world_state: String,
    /// What kind of observation this entry came from, or `None` for one
    /// minted before the field existed.
    ///
    /// An observation's kind did not use to survive the fold — the extent
    /// kept key, label and marker, and what produced them was recoverable
    /// only by reading the label as prose. TET-28's census predicate has
    /// to distinguish a whole-search record from a per-file hit, so the
    /// kind is carried structurally.
    ///
    /// `None` means *not recorded*, never *some default kind*: a fact
    /// minted by an older build can therefore never satisfy a census
    /// requirement, and the remedy is the cheap one — run the grep again.
    #[serde(default)]
    pub kind: Option<crate::pending::ObservationKind>,
    /// The search pattern on a whole-search record, empty otherwise and
    /// on entries minted before the field existed. See
    /// [`crate::pending::PendingEntry::pattern`].
    #[serde(default)]
    pub pattern: String,
    /// How many bytes of the fact's joined `output` this observation
    /// contributed, or `None` for an entry minted before the field
    /// existed.
    ///
    /// A fact's output is the join of its observations' outputs and the
    /// boundaries were not stored, which is unsound for anything that
    /// quotes it: text straddling the seam between two captures is a
    /// substring of the join while never having been contiguous in
    /// anything the author actually saw. Recording each entry's length
    /// makes the boundaries reconstructible — see
    /// [`Fact::observation_outputs`].
    ///
    /// `None` means *not recorded*, never *zero*, and reads exactly like
    /// [`ExtentEntry::kind`]: a fact minted by an older build can never
    /// be quoted from, and the remedy is the cheap one — look again.
    #[serde(default)]
    pub out_len: Option<usize>,
}

impl ExtentEntry {
    /// Whether this entry is a whole-search record rooted at the tree it
    /// was taken against — TET-28's census predicate, in one place.
    ///
    /// Both halves of the comparison are tool-captured: the key by
    /// `observe::search_key`, the root by `worldstate`. No authoring
    /// surface accepts an extent, so neither side can be typed.
    ///
    /// The `Search`/`NoMatch` pair is deliberate. A search that found
    /// nothing is as complete a census as one that found everything —
    /// the predicate is about existence and rooting, never about what was
    /// found, so a symbol with no occurrences is censused by the grep
    /// that established it has none.
    pub fn censuses(&self, symbol: &str) -> bool {
        use crate::pending::ObservationKind::{NoMatch, Search};
        matches!(self.kind, Some(Search) | Some(NoMatch))
            && self.pattern == symbol
            && !self.world_root.is_empty()
            && self.world_root != crate::worldstate::NO_GIT
            && self.key == self.world_root
    }
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

impl Fact {
    /// Each observation's captured output, recovered as its own slice of
    /// the joined `output`.
    ///
    /// Returns `None` — never a partial or best-effort answer — when the
    /// boundaries cannot be trusted: an entry minted before `out_len`
    /// existed, lengths that do not account for exactly the bytes in
    /// `output`, or a boundary landing inside a UTF-8 character. Every
    /// one of those means a tampered or legacy record, and guessing at
    /// one would put text the author never saw in quotation marks.
    ///
    /// The zero-length skip mirrors [`mint`]: entries whose observation
    /// captured nothing contribute no bytes and no separator, so they
    /// take no place in the join.
    pub fn observation_outputs(&self) -> Option<Vec<&str>> {
        let mut out = Vec::new();
        let mut at = 0usize;
        for e in &self.extent {
            let len = e.out_len?;
            if len == 0 {
                continue;
            }
            if !out.is_empty() {
                // The separator `mint` joined with.
                at += 1;
            }
            let end = at.checked_add(len)?;
            out.push(self.output.get(at..end)?);
            at = end;
        }
        // Lengths that do not add up to the whole output describe a record
        // that is not the one in front of us.
        if at != self.output.len() {
            return None;
        }
        Some(out)
    }

    /// Whether `text` is a verbatim substring of a **single** observation's
    /// captured output — TET-29's quotation relation, in one place.
    ///
    /// Single, not the joined whole: text straddling the seam between two
    /// observations was never contiguous in anything anyone looked at, and
    /// admitting it would let an author assemble a sentence the source does
    /// not contain out of two that it does.
    ///
    /// Nothing here normalises. Whitespace, comment prefixes and line
    /// endings are compared exactly as captured, because every relation
    /// looser than byte equality reopens the channel this exists to close
    /// — and stripping comment syntax would require knowing the donor's
    /// language, the coupling [`crate::scope`] is built to avoid.
    pub fn quotes(&self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        self.observation_outputs()
            .is_some_and(|obs| obs.iter().any(|o| o.contains(text)))
    }
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
        .map(|e| ExtentEntry {
            key: e.key.clone(),
            label: e.label.clone(),
            world_root: e.world_root.clone(),
            world_state: e.world_state.clone(),
            kind: Some(e.kind),
            pattern: e.pattern.clone(),
            out_len: Some(e.output.len()),
        })
        .collect();
    let output = buf.iter().filter(|e| !e.output.is_empty()).map(|e| e.output.as_str()).collect::<Vec<_>>().join("\n");

    // The pin: a content fingerprint over every entry's label, output
    // and world-tree marker — the prototype's compute_pin hashed
    // extent labels and captured output; folding the marker in too means
    // a fact minted against a different tree state pins differently even
    // if its labels/output happen to coincide. Both halves of the marker
    // go in: two observations of the same path under the same state but
    // in two different checkouts are two different observations. The
    // marker itself is still carried separately in `extent` (see
    // `ExtentEntry`), since a hash alone can't be compared by a human
    // without recomputing it — fix 1 is about visibility, not just
    // detectability.
    let mut hash_input = String::new();
    for e in &buf {
        hash_input.push_str(&e.label);
        hash_input.push('\n');
        hash_input.push_str(&e.world_root);
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
