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
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

/// A workspace's identity: a stable id and the moment it was created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    /// Unix seconds at creation. What makes "this pass began after the
    /// brief was issued" a checkable claim rather than an asserted one.
    pub born: u64,
}

/// Read an existing identity without creating one — for a snapshot
/// directory, where creating one would invent an author.
///
/// `dir` is `<memo>.tetel`, a fixed suffix on a caller-supplied memo
/// path — the same derivation `evidence::load`'s `<memo>.evidence.jsonl`
/// has, and in the same TET-79 class for the same reason: a FIFO living
/// at `<memo>.tetel/identity.json` would block this read on `check`'s,
/// `brief`'s or `verify-report`'s calling thread exactly as one at
/// `memo` itself would. Routed through [`read_caller_path`], the strict
/// (no-FIFO-exception) guard — this reads snapshot-internal metadata a
/// human never names with `@<(...)`, not a free-text carrier, so it
/// belongs with `evidence::load` rather than with
/// [`read_text_carrier_path`].
///
/// The refusal is not surfaced as distinct from "no identity recorded":
/// this function already collapses every other failure mode — a missing
/// file, unparseable JSON — into `None` via `.ok()`, so a guard refusal
/// joins that same collapse rather than getting new standing. Nothing
/// downstream (`AuthoringIdentity::NoSnapshot`/`SnapshotWithoutIdentity`,
/// or `verify.rs`'s pass-identity match) currently distinguishes those
/// cases from each other either; adding one only for this new failure
/// mode would be inconsistent with what the function already promises.
pub fn identity_of(dir: &Path) -> Option<String> {
    let raw = read_caller_path(&dir.join("identity.json")).ok()?;
    serde_json::from_str::<Identity>(&raw).ok().map(|i| i.id)
}

/// Read this workspace's identity, creating it on first use.
///
/// # Why identity is per-workspace and not per-session
///
/// The obvious unit is a session — the agent that made the observation.
/// It is the wrong one here. This corpus's dominant pattern is multi-day
/// continuation of one authoring effort, and keying identity to a session
/// would make every continuation *second-hand to its own earlier work* —
/// the tool fighting the way the work actually happens, which is the
/// condition under which annotation systems get abandoned.
///
/// A workspace is already the isolation unit: ids are workspace-relative
/// and collide freely across workspaces, and no cross-workspace citation
/// mechanism exists in the authoring path at all. So an author continuing
/// their own workspace tomorrow changes nothing, and an independent
/// grounding pass is simply *defined* as a fresh workspace. The property
/// that has to be checkable — "these grounds were captured by this pass,
/// not inherited from the author's" — falls out of where the facts live.
///
/// Written once and never rewritten: a workspace that already has an
/// identity keeps it, so nothing can quietly re-date a pass.
pub fn identity(workspace_dir: &Path) -> io::Result<Identity> {
    let path = workspace_dir.join("identity.json");
    if let Ok(raw) = fs::read_to_string(&path) {
        if let Ok(id) = serde_json::from_str::<Identity>(&raw) {
            return Ok(id);
        }
    }
    let born = now_unix();
    // Uniqueness, not unpredictability: this names a workspace, it does
    // not authenticate one. Nothing here resists a determined forger with
    // write access to the state directory, and nothing claims to.
    let seed = format!(
        "{}|{}|{}|{}",
        workspace_dir.display(),
        born,
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0)
    );
    let id = crate::evidence::sha256_hex(&seed)[..16].to_string();
    let identity = Identity { id, born };
    fs::write(&path, serde_json::to_string(&identity)?)?;
    Ok(identity)
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

/// The reason text both surfaces refuse `lines` + `grep` with.
///
/// Shared rather than written twice because the two surfaces used to
/// refuse this in different layers — clap on the CLI, an in-handler
/// branch on MCP — and a reader comparing two `refusals.log` files would
/// otherwise see the same refusal worded two ways and reasonably conclude
/// they were different refusals.
pub const LINES_WITH_GREP: &str = "`lines` and `grep` cannot be combined — a line range selects \
from one file, a grep searches for matches. Use one or the other.";

/// Every refusal recorded at or after `since`, verbatim, oldest first.
///
/// Returns the whole log when `since` is `None` — that is the
/// no-fact-yet case, where every refusal so far is still unaccounted
/// for. An unreadable or absent log yields nothing rather than an error:
/// this feeds a report printed beside a command that has already
/// succeeded, and a record that cannot be read must not turn a
/// successful mint into a failure.
///
/// The boundary is `>=`, not `>`. Timestamps are whole seconds, so a
/// refusal landing in the same second as the previous mint would
/// otherwise be dropped. Showing it twice is the safe direction; hiding
/// it once is the incident this exists for.
pub fn refusals_since(workspace_dir: &Path, since: Option<u64>) -> Vec<String> {
    let Ok(raw) = fs::read_to_string(workspace_dir.join("refusals.log")) else {
        return Vec::new();
    };
    raw.lines()
        .filter(|line| match since {
            None => true,
            Some(since) => line
                .split_once('\t')
                .and_then(|(ts, _)| ts.parse::<u64>().ok())
                .is_some_and(|ts| ts >= since),
        })
        .map(str::to_string)
        .collect()
}

/// What a caller-supplied path resolved to, when `fs::metadata` says it
/// is neither a regular file nor a directory. Only [`Fifo`](Self::Fifo)
/// gets treated differently from the rest anywhere in this module (see
/// [`read_text_carrier_path`]) — every other variant is always refused
/// outright, which is why they carry no data of their own beyond
/// [`label`](Self::label).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonRegularKind {
    Fifo,
    Socket,
    CharDevice,
    BlockDevice,
    /// Whatever this platform cannot name more precisely — including
    /// every non-regular, non-directory path on a non-Unix target, where
    /// `std::fs::FileType` has no vocabulary finer than "not a file, not
    /// a directory".
    Other,
}

impl NonRegularKind {
    fn label(self) -> &'static str {
        match self {
            NonRegularKind::Fifo => "named pipe (FIFO)",
            NonRegularKind::Socket => "socket",
            NonRegularKind::CharDevice => "character device",
            NonRegularKind::BlockDevice => "block device",
            NonRegularKind::Other => "not a regular file",
        }
    }
}

/// Classifies `meta` for every guard in this module, so a FIFO refused
/// through `look` and one refused through `check` are named the same
/// way.
///
/// `fs::metadata` follows symlinks (measured on this platform: a symlink
/// to a regular file reports `is_file() == true` here, while
/// `fs::symlink_metadata` on the same link reports `is_file() == false`
/// and `file_type().is_symlink() == true`), so a symlink whose target is
/// a regular file is invisible to this function — it classifies the
/// target, which is what lets that symlink pass every guard built on it.
///
/// Directories are deliberately excluded (returned as `None`, i.e. "not
/// flagged here"): `fs::read_to_string` already fails fast on one — no
/// blocking read to prevent — and `look_path` has its own richer
/// refusal for a directory that names the `--grep` alternative; folding
/// a directory into this generic message would either duplicate that or
/// blunt it.
#[cfg(unix)]
fn non_regular_kind(meta: &fs::Metadata) -> Option<NonRegularKind> {
    use std::os::unix::fs::FileTypeExt;
    if meta.is_file() || meta.is_dir() {
        return None;
    }
    let ft = meta.file_type();
    Some(if ft.is_fifo() {
        NonRegularKind::Fifo
    } else if ft.is_socket() {
        NonRegularKind::Socket
    } else if ft.is_char_device() {
        NonRegularKind::CharDevice
    } else if ft.is_block_device() {
        NonRegularKind::BlockDevice
    } else {
        NonRegularKind::Other
    })
}

#[cfg(not(unix))]
fn non_regular_kind(meta: &fs::Metadata) -> Option<NonRegularKind> {
    if meta.is_file() || meta.is_dir() { None } else { Some(NonRegularKind::Other) }
}

/// The refusal wording for a caller-supplied path this crate will not
/// read outright — shared by every guard below so a FIFO refused on
/// `look` and one refused on `check` read identically.
///
/// "extent" echoes `look`'s own vocabulary for a captured read (see
/// `look_path`'s doc comment): the point of the message is that a FIFO
/// or device has none — reading it blocks until a writer closes it
/// (a FIFO with no writer: forever) or, for `/dev/zero`, never blocks
/// and never ends either. TET-79: `look` on a FIFO hung indefinitely on
/// `main`, unkillable by any bound already shipped, because the block
/// happens inside `fs::read_to_string` on the calling thread itself —
/// there is no child process for a timeout to signal.
fn non_regular_reason(display: &str, kind: NonRegularKind) -> String {
    format!(
        "{display} is a {}, not a regular file; a stream with no end is not an extent `tetel` can read.",
        kind.label()
    )
}

/// Guards a caller-supplied path before it reaches `fs::read_to_string`,
/// for the authoring commands that hold a workspace and must log a
/// refusal into it (via [`refuse`]) rather than merely return one.
///
/// Checks only — the caller still performs its own read afterward. This
/// stays a guard rather than a guard-and-read because `look_path`
/// interleaves it between its own existence and directory refusals and
/// needs the workspace-scoped [`refuse`] call to fire from exactly this
/// point, not from inside a read this function doesn't own.
///
/// Always the strict form — no FIFO exception, unlike
/// [`read_text_carrier_path`]. Everything that reaches this (`look`'s
/// plain-file read, and `look --grep`'s explicitly-named, non-directory
/// root) treats what it reads as an *extent*: a captured span that must
/// be re-readable later exactly as captured. A stream has no such span,
/// live writer or not, so there is nothing here worth waiting out.
pub fn guard_regular_file(workspace_dir: &Path, cmd: &str, display: &str, path: &Path) -> Result<(), AuthoringError> {
    let meta = fs::metadata(path).map_err(|e| AuthoringError::Io(e.to_string()))?;
    if let Some(kind) = non_regular_kind(&meta) {
        return Err(refuse(workspace_dir, cmd, non_regular_reason(display, kind)));
    }
    Ok(())
}

/// Reads `path` the strict way every workspace-less *extent* reader needs
/// to (`check`, `brief`, `record`'s two memo reads, `evidence::load`'s
/// derived `<memo>.evidence.jsonl`, and `identity_of`'s derived
/// `<snapshot>/identity.json`): refuse before the read rather than block
/// inside it, and refuse every non-regular kind including a FIFO — each
/// of these reads becomes part of a memo's evidence, ledger or identity
/// state, which — like `look`'s capture — has to be a fixed extent, not
/// a live stream a caller happens to have a writer for right now. These
/// callers have no workspace to log a refusal into — `check_file` and
/// `brief_file` open no workspace by design (see this module's own doc
/// comment on working state vs. a memo's provenance) — so the refusal is
/// an ordinary `io::Error` rather than a routed [`AuthoringError`].
///
/// See [`read_text_carrier_path`] for the sibling used by `--note`/
/// `--proposition`/…'s `@file` form and by `record`'s own `--input`
/// argument, which allows a FIFO a bounded wait because neither reads a
/// source that has to stay re-readable later — unlike this function's
/// callers, nothing here is captured as an extent.
///
/// A missing path still surfaces as `io::ErrorKind::NotFound`, unchanged
/// from a bare `fs::read_to_string`: the existence check happens inside
/// `fs::metadata` here rather than being skipped, so callers matching on
/// that error kind (`evidence::load`'s "no ledger yet" case) keep working
/// unmodified.
pub fn read_caller_path(path: &Path) -> io::Result<String> {
    let meta = fs::metadata(path)?;
    if let Some(kind) = non_regular_kind(&meta) {
        let display = path.display().to_string();
        return Err(io::Error::new(io::ErrorKind::InvalidInput, non_regular_reason(&display, kind)));
    }
    fs::read_to_string(path)
}

/// How long [`read_text_carrier_path`] waits for a FIFO with no writer
/// yet before giving up.
///
/// Bracketed by two failure modes, not tuned to either alone: too short
/// and a legitimately slow producer behind `@<(some slow command)` (bash
/// process substitution) is cut off before it has written anything,
/// turning a working `@file` into a flaky one; too long and a
/// writer-less FIFO — the actual TET-79 case, just relocated from `look`
/// to a free-text flag — holds an MCP session (which answers requests
/// serially) for that whole span while the caller has no idea anything
/// is stuck. 10s sits in the "seconds to tens of seconds" this trade
/// calls for: generous next to a shell command substitution, which
/// ordinarily produces its first byte in well under a second, and short
/// next to the minutes-scale budget `run.timeout_ms` gives a command the
/// author explicitly chose to execute — this is not that; nothing behind
/// `@file`/`--input` was invoked on the author's own authority to run
/// for a while, so it gets no comparable allowance.
const FIFO_READ_DEADLINE: Duration = Duration::from_secs(10);

/// Reads `path` on a separate thread and gives up after `deadline` —
/// the same poll-`is_finished`-to-a-deadline shape `observe.rs`'s
/// `join_with_deadline`/`reap_with_grace` use for `run`'s bound, and the
/// same reasoning: a thread parked in a blocking `read()` cannot be
/// cancelled from outside, so this can only either wait for it to finish
/// or give up on it. `join()` is called only once `is_finished()` has
/// already confirmed the thread is done — a near-instant call at that
/// point, fetching a result rather than waiting for one — and never
/// called at all on the timeout path below, where the handle is simply
/// dropped unjoined. Dropping it is safe because the thread holds
/// nothing but its own stack and the file handle: it either notices its
/// writer close the pipe eventually and exits on its own, or (a writer
/// that never arrives) stays parked for the life of the process, which
/// is a leaked thread per incident rather than a wedged call.
fn read_with_deadline(path: &Path, deadline: Duration) -> io::Result<String> {
    let owned = path.to_path_buf();
    let handle = thread::spawn(move || fs::read_to_string(&owned));
    let until = Instant::now() + deadline;
    let mut nap = Duration::from_millis(1);
    loop {
        if handle.is_finished() {
            return match handle.join() {
                Ok(result) => result,
                Err(_) => Err(io::Error::new(io::ErrorKind::Other, format!("reading {} panicked", path.display()))),
            };
        }
        if Instant::now() >= until {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{} did not finish within {}s — a FIFO with no writer blocks forever, so \
this is refused rather than waited out indefinitely",
                    path.display(),
                    deadline.as_secs()
                ),
            ));
        }
        thread::sleep(nap);
        nap = (nap * 2).min(Duration::from_millis(50));
    }
}

/// Reads `path` the way `resolve_text_value`'s `@path` branch and
/// `record`'s `--input` argument both need to: refuse a socket,
/// character device or block device outright — `/dev/zero` never
/// reaches EOF no matter how long this waits, so a deadline on a device
/// would only turn a hang into a slow refusal after exhausting memory
/// first — but give a FIFO a bounded wait ([`FIFO_READ_DEADLINE`])
/// rather than refusing it outright, because `@<(some command)` (bash
/// process substitution) and `/dev/stdin` when stdin is piped (measured
/// on this platform: piped stdin makes `/dev/stdin` — itself a symlink
/// to `fd/0` — resolve to a FIFO) are both FIFOs with an actual writer,
/// and both worked on `main`. Neither caller reads a source that has to
/// stay re-readable later the way `look`'s capture or a memo's evidence
/// log does — see [`read_caller_path`] for the strict sibling used by
/// those.
pub fn read_text_carrier_path(path: &Path) -> io::Result<String> {
    let meta = fs::metadata(path)?;
    match non_regular_kind(&meta) {
        None => fs::read_to_string(path),
        Some(NonRegularKind::Fifo) => read_with_deadline(path, FIFO_READ_DEADLINE),
        Some(kind) => {
            let display = path.display().to_string();
            Err(io::Error::new(io::ErrorKind::InvalidInput, non_regular_reason(&display, kind)))
        }
    }
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
///
/// `@path`'s read goes through [`read_text_carrier_path`], not a bare
/// `fs::read_to_string`: a socket or device is refused outright (TET-79),
/// worded the same way `look`'s refusal is, but a FIFO gets a bounded
/// wait instead of `look`'s outright refusal — `@<(command)` is bash
/// process substitution, a FIFO with a real writer, and `--note
/// @<(...)` worked on `main`; refusing every FIFO here would have
/// silently broken it.
pub fn resolve_text_value(raw: &str) -> io::Result<String> {
    if raw == "-" {
        read_stdin()
    } else if let Some(path) = raw.strip_prefix('@') {
        read_text_carrier_path(Path::new(path))
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
    #[serde(default)]
    target: u64,
    #[serde(default)]
    transplant: u64,
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
    Target,
    /// Prefixed `X`, not `T`, which [`Kind::Target`] already holds.
    Transplant,
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
        Kind::Target => {
            c.target += 1;
            (c.target, "T")
        }
        Kind::Transplant => {
            c.transplant += 1;
            (c.transplant, "X")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tetel-ws-test-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_log(dir: &Path, lines: &[&str]) {
        fs::write(dir.join("refusals.log"), format!("{}\n", lines.join("\n"))).unwrap();
    }

    /// The window boundary, tested where the seconds can be set rather
    /// than raced. An end-to-end run cannot distinguish a working window
    /// from a test that finished inside one second.
    #[test]
    fn refusals_since_selects_the_window_inclusively() {
        let dir = scratch("refusals-window");
        write_log(
            &dir,
            &[
                "100\tlook\tbefore the mint",
                "200\tlook\texactly at the mint",
                "300\tlook\tafter the mint",
            ],
        );

        let got = refusals_since(&dir, Some(200));
        assert_eq!(
            got.len(),
            2,
            "the boundary is >=, so a refusal in the same second as the mint is shown \
rather than dropped — showing twice is the safe direction, hiding once is the incident: {got:?}"
        );
        assert!(got[0].contains("exactly at the mint"));
        assert!(got[1].contains("after the mint"));

        // No fact yet: every refusal so far is still unaccounted for.
        assert_eq!(refusals_since(&dir, None).len(), 3);

        let _ = fs::remove_dir_all(&dir);
    }

    /// Verbatim or nothing. The line is the shipped record's own text,
    /// not a re-rendering of it — a reader comparing a replay against
    /// `refusals.log` must see the same bytes.
    #[test]
    fn refusals_are_replayed_verbatim() {
        let dir = scratch("refusals-verbatim");
        let line = "150\tlook\tinvalid --lines \"1-2\" for a.rs: expected `A:B`";
        write_log(&dir, &[line]);
        assert_eq!(refusals_since(&dir, Some(100)), vec![line.to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A workspace with no refusals has no log, and a mint must not fail
    /// because a report could not be computed.
    #[test]
    fn a_missing_or_unreadable_log_yields_nothing_rather_than_failing() {
        let dir = scratch("refusals-absent");
        assert!(refusals_since(&dir, None).is_empty());
        assert!(refusals_since(Path::new("/nonexistent/tetel/ws"), None).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A malformed line has no timestamp to compare, so it cannot be
    /// placed in a window. Dropped rather than shown everywhere: the
    /// alternative is one unattributable line reappearing on every mint
    /// forever.
    #[test]
    fn a_line_without_a_parsable_timestamp_is_not_attributed_to_a_window() {
        let dir = scratch("refusals-malformed");
        write_log(&dir, &["garbage with no tab", "200\tlook\treal one"]);
        let got = refusals_since(&dir, Some(100));
        assert_eq!(got.len(), 1, "got: {got:?}");
        assert!(got[0].contains("real one"));
        // With no boundary there is nothing to compare against, so
        // everything is shown — including what cannot be placed.
        assert_eq!(refusals_since(&dir, None).len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
