//! The pending observation buffer: what `tetel look`/`tetel run` fill,
//! and what `tetel fact` mints from. Persisted as a single JSON file per
//! session so it survives between separate `tetel` invocations (each
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
}

fn path(session_dir: &Path) -> PathBuf {
    session_dir.join("pending.json")
}

pub fn load(session_dir: &Path) -> io::Result<Vec<PendingEntry>> {
    let p = path(session_dir);
    match fs::read_to_string(&p) {
        Ok(s) if !s.trim().is_empty() => {
            serde_json::from_str(&s).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
        Ok(_) => Ok(Vec::new()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

pub fn save(session_dir: &Path, entries: &[PendingEntry]) -> io::Result<()> {
    fs::create_dir_all(session_dir)?;
    let json = serde_json::to_string_pretty(entries).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(path(session_dir), json)
}

pub fn clear(session_dir: &Path) -> io::Result<()> {
    save(session_dir, &[])
}
