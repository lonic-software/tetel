//! The pending observation buffer: what `tetel look`/`tetel run` fill,
//! and what `tetel fact` mints from. Persisted as a single JSON file per
//! workspace so it survives between separate `tetel` invocations (each
//! subcommand is its own process) — but it is ephemeral state, not part
//! of the append-only record: it is cleared the moment a fact is
//! minted, and never itself gets a log entry.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ObservationKind {
    Path,
    GrepMatch,
    NoMatch,
    Proc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEntry {
    pub kind: ObservationKind,
    /// The overlap/dependency key — a resolved filesystem path when the
    /// designator names a file, or a literal command line when it
    /// doesn't. Fix 2 in the design memo: the prototype keyed every
    /// observation on the literal command string, so three different
    /// `sed -n` ranges of one file never overlapped each other or a
    /// plain read of it. `tetel look` (with or without `--lines`) and
    /// `tetel look --grep`'s per-file matches all key on the resolved
    /// path instead, so any two observations of the same file overlap
    /// regardless of what range or pattern produced them. `tetel run`
    /// stays keyed on its command line — a command names no single file
    /// in general, so it remains the one genuinely opaque case.
    pub key: String,
    /// The human-readable line shown in a fact's extent list.
    pub label: String,
    pub output: String,
    /// The working-tree state marker at the moment this observation was
    /// captured — see `worldstate.rs` and fix 1 in the design memo.
    pub world_state: String,
    /// Unix seconds at capture.
    ///
    /// Exists so `tetel fact` can say how old what it is folding is. A
    /// revision does not clear the buffer and a failed `look` does not
    /// add to it, so a `fact` can silently fold an observation from an
    /// earlier line of enquiry — which happened, producing a fact whose
    /// note described two files its extent never covered. Defaulted for
    /// entries written before this field existed.
    #[serde(default)]
    pub captured_at: u64,
}

fn path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("pending.json")
}

pub fn load(workspace_dir: &Path) -> io::Result<Vec<PendingEntry>> {
    let p = path(workspace_dir);
    match fs::read_to_string(&p) {
        Ok(s) if !s.trim().is_empty() => {
            serde_json::from_str(&s).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
        Ok(_) => Ok(Vec::new()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

pub fn save(workspace_dir: &Path, entries: &[PendingEntry]) -> io::Result<()> {
    fs::create_dir_all(workspace_dir)?;
    let json = serde_json::to_string_pretty(entries).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(path(workspace_dir), json)
}

pub fn clear(workspace_dir: &Path) -> io::Result<()> {
    save(workspace_dir, &[])
}
