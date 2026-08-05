//! `tetel look` and `tetel run` — the only two ways an authoring session
//! adds to its pending observation buffer (see `pending.rs`). Mirrors
//! the prototype's `tlook`/`trun`, with two changes: `--lines A:B` is
//! new (see the module's `look_path` doc comment), and every recorded
//! observation carries a `world_state` marker alongside its content
//! (fix 1 in the design memo — see `worldstate.rs`).

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::pending::{self, ObservationKind, PendingEntry};
use crate::session::{self, AuthoringError};
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
pub fn look_path(session_dir: &Path, path: &str, lines: Option<(usize, usize)>) -> Result<LookOutcome, AuthoringError> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(session::refuse(session_dir, "look", format!("no such path: {path}")));
    }
    if p.is_dir() {
        return Err(session::refuse(
            session_dir,
            "look",
            format!(
                "{path} is a directory; `tetel look` opens files. Use `tetel look --grep <pattern> {path}` to search it."
            ),
        ));
    }
    let contents = fs::read_to_string(p).map_err(|e| AuthoringError::Io(e.to_string()))?;
    let key = resolve_key(p);
    let world_state = worldstate::compute();

    let (shown, label) = match lines {
        None => (contents.clone(), path.to_string()),
        Some((a, b)) => {
            if a < 1 || a > b {
                return Err(session::refuse(session_dir, "look", format!("invalid --lines range {a}:{b}")));
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

    let entry = PendingEntry { kind: ObservationKind::Path, key, label, output: shown, world_state };
    let mut buf = pending::load(session_dir)?;
    buf.push(entry);
    pending::save(session_dir, &buf)?;

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
pub fn look_grep(session_dir: &Path, pattern: &str, root: &str) -> Result<LookOutcome, AuthoringError> {
    let root_path = Path::new(root);
    if !root_path.exists() {
        return Err(session::refuse(session_dir, "look", format!("no such path: {root}")));
    }
    let world_state = worldstate::compute();

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
    let mut buf = pending::load(session_dir)?;

    if stdout.trim().is_empty() {
        printed.push_str(&format!("no matches for '{pattern}' in {root}\n"));
        buf.push(PendingEntry {
            kind: ObservationKind::NoMatch,
            key: resolve_key(root_path),
            label: format!("no-match: {pattern} in {root}"),
            output: String::new(),
            world_state,
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
            buf.push(PendingEntry {
                kind: ObservationKind::GrepMatch,
                key: resolve_key(Path::new(&file)),
                label: format!("{file} (grep: {pattern})"),
                output: matches.join("\n"),
                world_state: world_state.clone(),
            });
        }
    }
    pending::save(session_dir, &buf)?;
    Ok(LookOutcome { printed })
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
pub fn run_command(session_dir: &Path, argv: &[String]) -> Result<RunOutcome, AuthoringError> {
    if argv.is_empty() {
        return Err(session::refuse(session_dir, "run", "no command given"));
    }
    let world_state = worldstate::compute();
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
        kind: ObservationKind::Proc,
        key: cmdline.clone(),
        label: format!("proc: {cmdline} (exit {exit_code})"),
        output: output_text.clone(),
        world_state,
    };
    let mut buf = pending::load(session_dir)?;
    buf.push(entry);
    pending::save(session_dir, &buf)?;

    Ok(RunOutcome { printed: output_text, exit_code })
}
