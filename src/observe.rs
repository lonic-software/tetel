//! `tetel look` and `tetel run` — the only two ways an authoring workspace
//! adds to its pending observation buffer (see `pending.rs`). Mirrors
//! the prototype's `tlook`/`trun`, with two changes: `--lines A:B` is
//! new (see the module's `look_path` doc comment), and every recorded
//! observation carries a working-tree marker alongside its content
//! (fix 1 in the design memo — see `worldstate.rs`).
//!
//! Each observation resolves its **own** marker, from what it touched: a
//! `look` from the file it opened, a `--grep` match from the file it
//! matched in, a bounded negative from the search root, and a `run` from
//! this process's working directory, which is where the command actually
//! ran. Resolving one marker per call and attaching it to everything was
//! the original shape and it described the wrong tree — see the measured
//! table in `worldstate.rs`.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::pending::{self, ObservationKind, PendingEntry};
use crate::workspace::{self, AuthoringError};
use crate::worldstate;

/// Resolve `path` to the key used for overlap matching: its canonical
/// absolute form when that succeeds, the path as given otherwise.
/// Canonicalization can fail on some platforms/filesystems even for a
/// path that exists (e.g. a permissions issue on an ancestor
/// directory); falling back keeps this infallible rather than losing
/// the observation over it.
fn resolve_key(path: &Path) -> String {
    fs::canonicalize(path).map(|p| p.display().to_string()).unwrap_or_else(|_| path.display().to_string())
}

pub struct LookOutcome {
    pub printed: String,
}

/// `tetel look <path> [--lines A:B]`.
///
/// `--lines A:B` (1-based, inclusive) is new in this port — the
/// prototype had no way to bound a read to a range, which forced
/// `tetel run sed -n '<range>p' <path>` as a workaround. That workaround
/// is exactly what produced fix 2's overlap blind spot (a `proc:`
/// observation keyed on the whole `sed` command line never overlapped
/// another range of the same file); giving `look` a first-class
/// `--lines` removes the reason to reach for `run` here at all, and its
/// key is the resolved path regardless of the range requested, so two
/// different ranges of one file always overlap each other.
///
/// A range entirely past the end of the file is not an error — it is
/// recorded as an explicit empty-range observation, the same "a bounded
/// negative must leave a trace" discipline as a zero-match grep.
pub fn look_path(workspace_dir: &Path, path: &str, lines: Option<(usize, usize)>) -> Result<LookOutcome, AuthoringError> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(workspace::refuse(workspace_dir, "look", format!("no such path: {path}")));
    }
    if p.is_dir() {
        return Err(workspace::refuse(
            workspace_dir,
            "look",
            format!(
                "{path} is a directory; `tetel look` opens files. Use `tetel look --grep <pattern> {path}` to search it."
            ),
        ));
    }
    let contents = fs::read_to_string(p).map_err(|e| AuthoringError::Io(e.to_string()))?;
    let key = resolve_key(p);
    // Resolved from the file being read, never from this process's working
    // directory — see `worldstate.rs` for the measurement that forced this.
    let world = worldstate::Session::new().for_path(p);

    let (shown, label) = match lines {
        None => (contents.clone(), path.to_string()),
        Some((a, b)) => {
            if a < 1 || a > b {
                return Err(workspace::refuse(workspace_dir, "look", format!("invalid --lines range {a}:{b}")));
            }
            let all: Vec<&str> = contents.lines().collect();
            let total = all.len();
            if a > total {
                (String::new(), format!("{path} lines {a}-{b} (file has {total} line(s); nothing in range)"))
            } else {
                let end = b.min(total);
                (all[a - 1..end].join("\n"), format!("{path} lines {a}-{end}"))
            }
        }
    };

    let mut printed = String::new();
    printed.push_str(&format!("==> {path} <==\n"));
    printed.push_str(&shown);
    if !shown.is_empty() && !shown.ends_with('\n') {
        printed.push('\n');
    }

    let entry = PendingEntry {
        kind: ObservationKind::Path,
        key,
        label,
        output: shown,
        world_root: world.root,
        world_state: world.state,
        captured_at: workspace::now_unix(),
    };
    let mut buf = pending::load(workspace_dir)?;
    buf.push(entry);
    pending::save(workspace_dir, &buf)?;

    Ok(LookOutcome { printed })
}

/// `tetel look --grep <pattern> <path-or-dir>`.
///
/// Shells out to the system `grep` (present on every platform this
/// crate targets), the same way `run_command` shells out to whatever
/// command it's given — reimplementing pattern matching wasn't worth
/// the risk of a subtly different regex dialect. A zero-match search
/// records an explicit no-match observation (a bounded negative that
/// leaves no trace was the specific defect a prior run surfaced); a
/// match records one entry per file that actually matched, keyed by
/// that file's resolved path (fix 2), not by the search root or the
/// grep command line.
pub fn look_grep(workspace_dir: &Path, pattern: &str, root: &str) -> Result<LookOutcome, AuthoringError> {
    let root_path = Path::new(root);
    if !root_path.exists() {
        return Err(workspace::refuse(workspace_dir, "look", format!("no such path: {root}")));
    }
    // One session for the whole search: a recursive grep can match in
    // dozens of files, and each matched file resolves its own marker
    // (a search root can span a submodule or a nested worktree), but
    // files sharing a directory resolve once between them.
    let mut world = worldstate::Session::new();

    let mut args: Vec<&str> = Vec::new();
    if root_path.is_dir() {
        args.push("-r");
    }
    args.push("-I");
    args.push("-n");
    args.push("-e");
    args.push(pattern);
    args.push(root);

    let output = Command::new("grep")
        .args(&args)
        .output()
        .map_err(|e| AuthoringError::Io(format!("could not run grep: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    let mut printed = String::new();
    let mut buf = pending::load(workspace_dir)?;

    if stdout.trim().is_empty() {
        // A bounded negative is about the tree it searched, so its marker
        // comes from the search root.
        let m = world.for_path(root_path);
        printed.push_str(&format!("no matches for '{pattern}' in {root}\n"));
        buf.push(PendingEntry {
            captured_at: workspace::now_unix(),
            kind: ObservationKind::NoMatch,
            key: resolve_key(root_path),
            label: format!("no-match: {pattern} in {root}"),
            output: String::new(),
            world_root: m.root,
            world_state: m.state,
        });
    } else {
        printed.push_str(&stdout);
        let mut by_file: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
        for line in stdout.lines() {
            if let Some((file, rest)) = line.split_once(':') {
                by_file.entry(file.to_string()).or_default().push(rest.to_string());
            }
        }
        for (file, matches) in by_file {
            let m = world.for_path(Path::new(&file));
            buf.push(PendingEntry {
                captured_at: workspace::now_unix(),
                kind: ObservationKind::GrepMatch,
                key: resolve_key(Path::new(&file)),
                label: format!("{file} (grep: {pattern})"),
                output: matches.join("\n"),
                world_root: m.root,
                world_state: m.state,
            });
        }
    }
    pending::save(workspace_dir, &buf)?;
    Ok(LookOutcome { printed })
}

/// What `tetel look` was asked to do — open a path, or search one with
/// `--grep`. The single shape both the CLI and the MCP server build and
/// pass to [`dispatch`], so the two front ends can never drift on which
/// combination of missing flags gets refused, or with what text.
pub enum LookRequest {
    /// `tetel look <path> [--lines A:B]`.
    Open { path: Option<String>, lines: Option<(usize, usize)> },
    /// `tetel look --grep <pattern> <path-or-dir>`.
    Grep { pattern: String, root: Option<String> },
}

/// Dispatches a [`LookRequest`] to [`look_path`] or [`look_grep`],
/// refusing with the exact text the CLI has always printed when the
/// path-or-dir a mode needs was never given — lifted here (rather than
/// left as a bare `eprintln!` in `main.rs`) so the MCP server's
/// structured refusals carry the same guidance the CLI does, from the
/// one place that decides it.
pub fn dispatch(workspace_dir: &Path, req: LookRequest) -> Result<LookOutcome, AuthoringError> {
    match req {
        LookRequest::Grep { pattern, root } => {
            let root = root
                .ok_or_else(|| workspace::refuse(workspace_dir, "look", "`look --grep <pattern>` requires a path-or-dir"))?;
            look_grep(workspace_dir, &pattern, &root)
        }
        LookRequest::Open { path, lines } => {
            let path = path.ok_or_else(|| {
                workspace::refuse(
                    workspace_dir,
                    "look",
                    "usage: tetel look <path> [--lines A:B] | tetel look --grep <pattern> <path-or-dir>",
                )
            })?;
            look_path(workspace_dir, &path, lines)
        }
    }
}

pub struct RunOutcome {
    pub printed: String,
    pub exit_code: i32,
}

/// `tetel run <command...>` — executes `argv` directly (never through a
/// shell), prints its combined stdout/stderr, and records `argv`'s
/// command line plus the captured output into the pending buffer.
/// `run`'s own exit code mirrors the executed command's.
///
/// stdout and stderr are captured on separate pipes, each drained on
/// its own thread into one shared buffer, which reproduces `2>&1`'s
/// merged stream closely but not exactly: chunks from the two streams
/// can interleave in a different order than a real shell redirection
/// would produce, since nothing here observes their true fd-level
/// arrival order. Documented, not silently assumed exact.
pub fn run_command(workspace_dir: &Path, argv: &[String]) -> Result<RunOutcome, AuthoringError> {
    if argv.is_empty() {
        return Err(workspace::refuse(workspace_dir, "run", "no command given"));
    }
    // `run` is the one case where the process's own working directory is
    // the right answer rather than a fallback: a command names no path in
    // general, and it genuinely executed here.
    let world = worldstate::Session::new().for_cwd();
    let cmdline = argv.join(" ");

    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AuthoringError::Io(format!("could not run `{cmdline}`: {e}")))?;

    let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
    let stdout_pipe = child.stdout.take().expect("stdout was piped");
    let stderr_pipe = child.stderr.take().expect("stderr was piped");

    fn drain<R: Read + Send + 'static>(mut pipe: R, buf: Arc<Mutex<Vec<u8>>>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                let n = pipe.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
        })
    }
    let t_out = drain(stdout_pipe, Arc::clone(&captured));
    let t_err = drain(stderr_pipe, Arc::clone(&captured));

    let status = child.wait().map_err(|e| AuthoringError::Io(e.to_string()))?;
    t_out.join().ok();
    t_err.join().ok();

    let bytes = captured.lock().unwrap().clone();
    let output_text = String::from_utf8_lossy(&bytes).into_owned();
    let exit_code = status.code().unwrap_or(-1);

    let entry = PendingEntry {
        captured_at: workspace::now_unix(),
        kind: ObservationKind::Proc,
        key: cmdline.clone(),
        label: format!("proc: {cmdline} (exit {exit_code})"),
        output: output_text.clone(),
        world_root: world.root,
        world_state: world.state,
    };
    let mut buf = pending::load(workspace_dir)?;
    buf.push(entry);
    pending::save(workspace_dir, &buf)?;

    Ok(RunOutcome { printed: output_text, exit_code })
}
