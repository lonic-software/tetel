//! Workspace state for tetel's authoring commands (`look`, `run`, `fact`,
//! `claim`, `prose`, `render`, `query`): the pending observation buffer,
//! and the append-only fact/claim/prose logs. Unlike `check`/`brief`/
//! `record`, which read state back out of a document already on disk,
//! authoring has no document to read from — it has to keep its own
//! state between one `tetel` invocation and the next, since each
//! subcommand is its own process.
//!
//! A workspace is not the document: alongside the append-only fact/
//! claim/prose logs a document is rendered from, it also holds the
//! pending observation buffer, the refusals log, and the id counters —
//! none of which `render` ever reads. It is where one authoring effort's
//! working state lives; a document is one output of it, not the whole of
//! it.
//!
//! Ids minted here (`F1`, `C1`, `P1`, ...) are workspace-relative: they
//! are unique only within the workspace that minted them, and collide
//! freely across workspaces (`F1` in one workspace names nothing in
//! another). Nothing in this crate may promise cross-workspace id
//! stability or uniqueness — a caller that needs to tell two workspaces'
//! ids apart must qualify them itself.
//!
//! # Where workspace state lives
//!
//! Working state never lives inside a repository. Two separate arguments
//! support that, and it is worth keeping them apart, because an earlier
//! version of this comment ran them together and got the right answer for
//! a fuzzy reason.
//!
//! The first is an **observer effect**, and it applies specifically to the
//! tree under design. `look` and `run` capture what they find there —
//! including `git status --porcelain` and a world-state pin derived from
//! that tree (see [`crate::worldstate`]). Writing tetel's own state into
//! that tree changes what tetel then observes about it: the instrument
//! would be recording its own presence, and the pin would move because
//! tetel was there. That is a correctness failure, not untidiness.
//!
//! The second is **temporal**, and it is the one that generalises to every
//! repository, including the one a memo is written into. Working state
//! churns on every command, exists before any memo does, and carries
//! half-consumed intermediates — a pending buffer of observations not yet
//! minted into facts, a refusals log written whenever a command is
//! refused. None of that is a record anyone should be committing.
//!
//! So the distinction that matters is **working state versus shipped
//! record**, not planning repository versus code repository. Working state
//! lives here, outside every repository. A finished document's provenance
//! is a different artifact with different rules — written once, from a
//! workspace whose pending buffer is empty, immutable thereafter, and
//! sitting beside the memo under the same `<memo>.<suffix>` convention
//! `<memo>.evidence.jsonl` already established. See [`crate::snapshot`],
//! which owns that half and explains why a rendered document is not
//! self-contained without it.
//!
//! Instead, workspace state lives under a state-home directory, resolved
//! in this order:
//!   1. `$TETEL_STATE_HOME`, if set — an escape hatch, and how the test
//!      suite points every run at a private temporary directory rather
//!      than a real user's state.
//!   2. `$XDG_STATE_HOME/tetel`, if `XDG_STATE_HOME` is set.
//!   3. `$HOME/.local/state/tetel` — the XDG default location, applied
//!      by hand rather than pulling in a platform-directories dependency
//!      for one path.
//!
//! Under that root, one directory per named workspace — `workspaces/<name>`,
//! `<name>` defaulting to `default` and overridable with the CLI's
//! global `--workspace` flag — so two documents being authored
//! concurrently don't share one pending buffer or one set of ids.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const DEFAULT_WORKSPACE: &str = "default";

/// The state-home root — see the module doc comment for resolution order.
pub fn state_home() -> PathBuf {
    if let Ok(p) = env::var("TETEL_STATE_HOME") {
        return PathBuf::from(p);
    }
    if let Ok(p) = env::var("XDG_STATE_HOME") {
        return PathBuf::from(p).join("tetel");
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local").join("state").join("tetel")
}

/// The directory one named workspace's state lives in.
pub fn workspace_dir(name: &str) -> PathBuf {
    state_home().join("workspaces").join(name)
}

pub fn ensure(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

/// One workspace on disk, with enough shape to tell which is which.
///
/// The counts are what an author actually needs to recognise a workspace
/// they left days ago — a name alone does not distinguish an empty
/// scratch workspace from a finished memo's.
pub struct WorkspaceSummary {
    pub name: String,
    pub facts: usize,
    pub claims: usize,
    pub prose: usize,
}

/// Every workspace under [`state_home`], name-sorted.
///
/// Returns an empty list when no workspace has ever been created — that
/// is an ordinary state, not an error, so this never creates the root
/// directory as a side effect of being asked what exists. Counts are line
/// counts of the append-only logs, so a revision or a withdrawal counts as
/// its own event; this is a "which workspace is this" aid, not a census of
/// live ids (`tetel query` answers that).
pub fn list() -> io::Result<Vec<WorkspaceSummary>> {
    let root = state_home().join("workspaces");
    let entries = match fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let count = |dir: &Path, file: &str| -> usize {
        fs::read_to_string(dir.join(file))
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    };

    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        out.push(WorkspaceSummary {
            name,
            facts: count(&dir, "facts.jsonl"),
            claims: count(&dir, "claims.jsonl"),
            prose: count(&dir, "prose.jsonl"),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Resolve `name` to its workspace directory and ensure it exists on
/// disk — the two-step sequence every authoring command (`look`, `run`,
/// `fact`, `claim`, `prose`) needs before touching any state. Lifted here
/// so the CLI and the MCP server share the exact same resolution rather
/// than each re-deriving it.
pub fn open(name: &str) -> io::Result<PathBuf> {
    let dir = workspace_dir(name);
    ensure(&dir)?;
    Ok(dir)
}

/// Every authoring command either succeeds or refuses (see [`refuse`]);
/// `Io` is reserved for genuine I/O failure (disk full, permissions),
/// never for a semantic refusal.
#[derive(Debug)]
pub enum AuthoringError {
    Refused(String),
    Io(String),
}

impl std::fmt::Display for AuthoringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthoringError::Refused(s) => write!(f, "refused: {s}"),
            AuthoringError::Io(s) => write!(f, "{s}"),
        }
    }
}

impl From<io::Error> for AuthoringError {
    fn from(e: io::Error) -> Self {
        AuthoringError::Io(e.to_string())
    }
}

fn ts() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}", now.as_secs())
}

pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Append one line to `refusals.log` and return the corresponding
/// error. Every refusal in every authoring command goes through here —
/// the single choke point, mirrored from the prototype's `refuse()` in
/// `common.sh`: no refusal branch anywhere calls its own error path
/// directly instead of this one.
pub fn refuse(workspace_dir: &Path, cmd: &str, reason: impl Into<String>) -> AuthoringError {
    let reason = reason.into();
    let _ = ensure(workspace_dir);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(workspace_dir.join("refusals.log")) {
        let _ = writeln!(f, "{}\t{}\t{}", ts(), cmd, reason);
    }
    AuthoringError::Refused(reason)
}

/// Resolves a `text|-|@file` CLI value: `-` reads all of stdin; `@path`
/// reads the named file, byte-exact; anything else is used as the
/// literal text. Every free-text flag (`--note`, `--proposition`, `--why`,
/// prose's `--text`/`--heading`) goes through this — see fix 3 in the
/// design memo: shell interpolation has corrupted backtick- and
/// quote-bearing text passed as a raw argument more than once, and the
/// `-`/`@file` forms never go through that interpolation at all. The
/// literal-text branch cannot undo shell damage already done before this
/// process started, so it can only warn (see [`warn_on_inline_backtick`])
/// when a backtick still made it through — the first real design memo
/// authored with this tool hit exactly this on every inline attempt,
/// falling back to `@file` each time only after a failed call.
pub fn resolve_text_value(raw: &str) -> io::Result<String> {
    if raw == "-" {
        read_stdin()
    } else if let Some(path) = raw.strip_prefix('@') {
        fs::read_to_string(path)
    } else {
        warn_on_inline_backtick(raw);
        Ok(raw.to_string())
    }
}

/// Warns to stderr, never refuses, when a value arrived as a literal
/// command-line argument and contains a backtick: the shell has already
/// evaluated command substitution by the time this process sees its
/// argv, so a backtick surviving into `raw` either wasn't the one the
/// author meant (the shell ate a matching one and ran its contents) or
/// is a rarer, legitimate lone backtick — this can't tell which, so it
/// names the risk rather than blocking a value that might be entirely
/// intentional. Never called for `-`/`@file`, since neither passes
/// through a shell's command-substitution step at all.
fn warn_on_inline_backtick(raw: &str) {
    if raw.contains('`') {
        eprintln!(
            "tetel: warning: this text was given inline on the command line and contains a backtick — \
the shell may already have run it as command substitution before tetel ever saw the result; \
pass text with backticks via stdin (`-`) or `@file` instead."
        );
    }
}

/// Same as [`resolve_text_value`], but when the flag is entirely absent
/// falls back to stdin — the documented default for prose's body text,
/// the one field in this crate's authoring surface with no flag of its
/// own at all (see `prose.rs`). Never used for `--why` or any other
/// secondary field: a forgotten secondary flag must refuse immediately
/// with a clear message, not hang the terminal waiting on stdin that
/// was never coming.
pub fn resolve_text_or_stdin(raw: Option<&str>) -> io::Result<String> {
    match raw {
        Some(v) => resolve_text_value(v),
        None => read_stdin(),
    }
}

fn read_stdin() -> io::Result<String> {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Counters {
    #[serde(default)]
    fact: u64,
    #[serde(default)]
    claim: u64,
    #[serde(default)]
    prose: u64,
}

fn counters_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("counters.json")
}

fn load_counters(workspace_dir: &Path) -> io::Result<Counters> {
    match fs::read_to_string(counters_path(workspace_dir)) {
        Ok(s) => Ok(serde_json::from_str(&s).unwrap_or_default()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Counters::default()),
        Err(e) => Err(e),
    }
}

fn save_counters(workspace_dir: &Path, c: &Counters) -> io::Result<()> {
    ensure(workspace_dir)?;
    let json = serde_json::to_string_pretty(c).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(counters_path(workspace_dir), json)
}

pub enum Kind {
    Fact,
    Claim,
    Prose,
}

/// Allocates and persists the next id for `kind`, returning e.g. `F3`.
/// See the module doc comment: this id is workspace-relative and gives
/// no promise of uniqueness or stability outside `workspace_dir`.
pub fn next_id(workspace_dir: &Path, kind: Kind) -> io::Result<String> {
    let mut c = load_counters(workspace_dir)?;
    let (n, prefix) = match kind {
        Kind::Fact => {
            c.fact += 1;
            (c.fact, "F")
        }
        Kind::Claim => {
            c.claim += 1;
            (c.claim, "C")
        }
        Kind::Prose => {
            c.prose += 1;
            (c.prose, "P")
        }
    };
    save_counters(workspace_dir, &c)?;
    Ok(format!("{prefix}{n}"))
}

/// Append one JSON value as its own line to a `.jsonl` log — the
/// append-only ledger for facts/claims/prose, mirroring
/// `evidence::record`'s one-line, never-rewritten append.
pub fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    ensure(path.parent().unwrap_or_else(|| Path::new(".")))?;
    let line = serde_json::to_string(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")
}

/// Read every line of a `.jsonl` log. A missing file means nothing has
/// been recorded yet — not an error.
pub fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Vec<T>> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?);
    }
    Ok(out)
}
