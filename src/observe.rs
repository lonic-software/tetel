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
use std::time::{Duration, Instant};

use crate::config;
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

/// The key for a whole-search record: the resolved search root, spelled
/// the way its own world marker spells the tree root **when the two name
/// the same directory**.
///
/// TET-28's census predicate asks whether a search was rooted at the
/// worktree, and answers it by comparing this key against the entry's own
/// `world_root`. Both are captured, but by two different tools —
/// `fs::canonicalize` here, `git rev-parse --show-toplevel` there — and
/// nothing guarantees two tools spell one directory identically. Rather
/// than have the predicate reason about that, the divergence is resolved
/// once, here, where both spellings are in hand.
///
/// The comparison is on canonicalized forms, so it is about *which
/// directory*, never about how either tool wrote it down. When they name
/// different directories — a search rooted at a subdirectory, which is
/// exactly the case the census exists to catch — the resolved root is
/// kept verbatim and the predicate's byte comparison fails, correctly.
fn search_key(root: &Path, world_root: &str) -> String {
    let resolved = resolve_key(root);
    if world_root.is_empty() {
        return resolved;
    }
    let same = fs::canonicalize(root)
        .ok()
        .zip(fs::canonicalize(world_root).ok())
        .map(|(a, b)| a == b)
        .unwrap_or(false);
    if same { world_root.to_string() } else { resolved }
}

pub struct LookOutcome {
    pub printed: String,
}

/// Does `root` name a path inside tetel's own output?
///
/// One component test, and it is the *entire* root predicate. The obvious
/// design has three, one per artifact; the other two — an evidence ledger
/// and a rendered memo — are **files**, so a root naming either has
/// already taken [`look_grep`]'s not-a-directory branch and never reaches
/// a content test at all.
fn names_tetel_output(root: &Path) -> bool {
    root.components().any(|c| c.as_os_str().to_str().is_some_and(|s| s.ends_with(".tetel")))
}

/// Every rendered memo under `root`, spelled the way `root` was spelled.
///
/// A file `X` is a rendered memo **iff `X.tetel/` exists as a directory**:
/// `snapshot::snapshot_path` appends `.tetel` to the whole filename, so a
/// memo carries no marker of its own and is identified by its sibling
/// snapshot. That needs no naming convention and no `docs/design/`
/// hardcoding, and it stays correct wherever someone renders a memo.
///
/// The spelling matters and is the reason this walks from `root` as given
/// rather than canonicalizing: the paths must line up with the ones grep
/// prints, both for the `--exclude` patterns (on the greps where those
/// anchor at all) and for [`drop_excluded`], which compares against
/// grep's own output. An absolute pattern against a relative root matches
/// neither. Walking from `root` yields paths already carrying its
/// spelling.
///
/// Two bounds, stated rather than engineered around. An unreadable
/// directory is skipped, which matches what the platform's grep does with
/// one and is already in `check`'s standing non-coverage. And symlinked
/// directories are not descended, because [`fs::DirEntry::file_type`] does
/// not follow them — consistent with the `grep -r` this feeds, which needs
/// `-R` to follow, but never measured against a tree that has one.
fn rendered_memos(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                stack.push(path);
                continue;
            };
            // `strip_suffix`, never `trim_end_matches`: the latter strips
            // every repetition, so `a.tetel.tetel` would yield `a`.
            if let Some(stem) = name.strip_suffix(".tetel") {
                let memo = path.with_file_name(stem);
                if memo.is_file() {
                    found.push(memo.display().to_string());
                }
                // Nothing inside a snapshot is itself a memo.
                continue;
            }
            stack.push(path);
        }
    }
    found.sort();
    found
}

/// Everything `git` is told to ignore under `root`, spelled the way `root`
/// was spelled, as `(is_directory, path)`.
///
/// Asks **git** rather than parsing `.gitignore`. Gitignore semantics —
/// negation, precedence between nested files, `core.excludesFile`, the
/// repository's own `info/exclude` — are not safely reimplementable, and
/// git is the authoritative oracle for a question that is otherwise a
/// judgement. `--directory` collapses a wholly-ignored directory into one
/// entry, so the list stays short: two entries in this repository.
///
/// The trailing slash git emits on a directory is **stripped**, because
/// `--exclude-dir=build/` matches nothing at all on the grep this feeds —
/// silently, which is the worst way for an exclusion to fail. Measured on
/// BSD grep 2.6.0-FreeBSD alongside the two forms that do work *there*: a
/// root-spelled path excludes exactly its own directory, and a bare
/// basename over-excludes every directory of that name anywhere in the
/// tree, which is the same trap [`rendered_memos`] documents.
///
/// That measurement covered one grep. GNU grep matches these patterns
/// against a base name when recursing, so the root-spelled form excludes
/// nothing there at all — see [`drop_excluded`], which is what now makes
/// the exclusion hold regardless. The spelling is kept because it is the
/// form that lets a grep skip the tree instead of reading it.
///
/// A root outside any repository, or a machine with no `git`, yields
/// nothing and the search is simply unfiltered by this rule. That is
/// reported rather than assumed — see [`exclusion_note`].
fn ignored_paths(root: &Path) -> Vec<(bool, String)> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--others", "--ignored", "--exclude-standard", "--directory"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let is_dir = line.ends_with('/');
        let rel = line.strip_suffix('/').unwrap_or(line);
        found.push((is_dir, root.join(rel).display().to_string()));
    }
    found.sort();
    found
}

/// What exclusions a search was given, and — when it was given none — why.
///
/// Both are recorded, because silence about an exclusion is ambiguous
/// between "nothing was hidden" and "nobody said". This text goes into the
/// whole-search record's `label`, which is the only record whose job is to
/// describe the search rather than what it hit, and which two carriers
/// already built pick up for free: the memo's Facts table renders labels,
/// and the pin hashes them, so two searches with different exclusion sets
/// fingerprint differently even when the bytes they matched are identical.
///
/// The memo paths are listed rather than counted, deliberately. A count
/// would let two searches over different memo sets of the same size pin
/// identically, which is the property this text exists to provide.
fn exclusion_note(exclusions: &Exclusions) -> String {
    match exclusions {
        Exclusions::None { why } => format!("no exclusions: {why}"),
        Exclusions::Applied { memos, ignored } => {
            let mut s = String::from("skipped tetel's own output: *.tetel/ directories, *.evidence.jsonl");
            if memos.is_empty() {
                s.push_str(", and no rendered memo");
            } else {
                s.push_str(&format!(
                    ", and {} rendered memo{} ({})",
                    memos.len(),
                    if memos.len() == 1 { "" } else { "s" },
                    memos.join(", ")
                ));
            }
            // Reported even when empty, and distinctly from "none found",
            // because "git ignores nothing here" and "this is not a
            // repository" are different facts about what was searched.
            if ignored.is_empty() {
                s.push_str("; git-ignored paths: none, or the root is outside a repository");
            } else {
                let names: Vec<&str> = ignored.iter().map(|(_, p)| p.as_str()).collect();
                s.push_str(&format!(
                    "; and {} git-ignored path{} ({})",
                    names.len(),
                    if names.len() == 1 { "" } else { "s" },
                    names.join(", ")
                ));
            }
            s
        }
    }
}

/// Whether a search was filtered, decided once from the root alone.
enum Exclusions {
    /// Withheld — and in both cases this is **forced, not chosen**. A file
    /// root is the caller naming one thing. A root inside tetel's own
    /// output must be searched unfiltered because grep suppresses a
    /// directory named as the recursion root when `--exclude-dir` matches
    /// it, so passing the flags there would return nothing at all.
    None { why: &'static str },
    Applied { memos: Vec<String>, ignored: Vec<(bool, String)> },
}

impl Exclusions {
    /// The rule, in one place: exclusions apply only to a directory root
    /// that does not itself name tetel's output.
    ///
    /// The git-ignored set rides the *same* decision rather than getting
    /// its own, and deliberately so. Both answer one question — is this
    /// content the tooling generated, or content someone wrote? — and a
    /// caller who names a path has, in both cases, said they want it. A
    /// second predicate would mean a root could be explicit for one kind
    /// of exclusion and not the other, which is a distinction nobody
    /// asking for it would predict.
    fn decide(root: &Path) -> Self {
        if !root.is_dir() {
            Exclusions::None { why: "the caller named one file" }
        } else if names_tetel_output(root) {
            Exclusions::None { why: "the caller named tetel's own output" }
        } else {
            Exclusions::Applied { memos: rendered_memos(root), ignored: ignored_paths(root) }
        }
    }

    /// Whether a path grep reported is one this search excluded.
    ///
    /// A directory excludes itself and everything beneath it, tested on a
    /// `/` boundary so `./build` never takes `./keep/build` — the whole
    /// point of spelling exclusions from the root rather than by basename.
    fn covers(&self, file: &str) -> bool {
        let Exclusions::Applied { memos, ignored } = self else {
            return false;
        };
        let under = |dir: &str| file == dir || file.starts_with(&format!("{dir}/"));
        memos.iter().any(|m| m == file)
            || ignored.iter().any(|(is_dir, p)| if *is_dir { under(p) } else { p == file })
    }
}

/// Drop the lines naming a path the search excluded.
///
/// **This is what makes an exclusion true; the flags are only an
/// optimisation.** `--exclude`/`--exclude-dir` do not mean the same thing
/// on every grep. BSD grep 2.6.0-FreeBSD and ugrep match a root-spelled
/// pattern against the path, which is what this tool emits and what its
/// exclusions were originally measured against. GNU grep matches the
/// pattern against a **base name** when recursing, so every root-spelled
/// pattern matches nothing there, silently — a Linux census returned
/// tetel's own memos and the whole of a git-ignored `target/` while the
/// label went on saying they were skipped.
///
/// Both bare-basename spellings that GNU *does* honour are wrong for a
/// different reason: `--exclude-dir=build` takes `keep/build` too, hiding
/// source under the banner of hiding generated output. There is no flag
/// spelling that anchors on both implementations, so the anchoring is done
/// here, once, over the one thing every grep agrees on — the paths it
/// printed.
///
/// The flags are still passed. Where they work they keep grep from reading
/// the excluded tree at all, which on a repository with a large `target/`
/// is the difference this filter cannot recover; where they do not, they
/// match nothing and cost nothing. Correctness no longer depends on which.
fn drop_excluded(stdout: &str, exclusions: &Exclusions) -> String {
    if !matches!(exclusions, Exclusions::Applied { .. }) {
        return stdout.to_string();
    }
    let mut out = String::new();
    for line in stdout.lines() {
        // The same split the per-file records are built from, so the
        // returned bytes and the captured ones can never disagree about
        // which files this search covered.
        let file = line.split_once(':').map(|(f, _)| f).unwrap_or(line);
        if exclusions.covers(file) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
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
        pattern: String::new(),
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
    // `-H` unconditionally, because grep only prints the filename when it
    // was given more than one file to search. Without it a search of a
    // *single* file returns `<line>:<match>`, and the `split_once(':')`
    // below then read the line number as the filename: every such
    // observation was keyed on a bare integer, overlapped nothing, and —
    // since a bare integer has no parent directory — resolved its
    // world-tree marker from this process's working directory, which is
    // precisely the defect TET-5 removed everywhere else. Found by the
    // grounding pass over TET-5's own design memo, in that memo's own
    // extent.
    args.push("-H");

    // Tetel's own output is skipped, so that a census does not read back
    // what tetel previously wrote. This is a decision about *whether* to
    // pass exclusion flags, not about which — see [`Exclusions::decide`],
    // and see [`rendered_memos`] for why the patterns must carry `root`'s
    // own spelling.
    //
    // The flags are a fast path, not the guarantee. Only the two basename
    // globs below mean the same thing on every grep; the root-spelled
    // patterns anchor on some and match nothing on others. What actually
    // holds the exclusion is [`drop_excluded`], over the output.
    //
    // The exception the ticket wanted — "an explicitly named path is still
    // read, only traversal skips" — is **not** something grep provides.
    // Measured: `--exclude` skips a file named on the command line, and
    // `--exclude-dir` skips a directory named as the recursion root. So
    // the exception is implemented here, above grep, by withholding.
    let exclusions = Exclusions::decide(root_path);
    let mut owned: Vec<String> = Vec::new();
    if let Exclusions::Applied { memos, ignored } = &exclusions {
        owned.push("--exclude-dir=*.tetel".to_string());
        owned.push("--exclude=*.evidence.jsonl".to_string());
        for memo in memos {
            owned.push(format!("--exclude={memo}"));
        }
        // Directories and files need different flags, and both patterns
        // carry `root`'s spelling for the reason `rendered_memos` gives.
        for (is_dir, path) in ignored {
            owned.push(if *is_dir { format!("--exclude-dir={path}") } else { format!("--exclude={path}") });
        }
    }
    for flag in &owned {
        args.push(flag);
    }

    args.push("-e");
    args.push(pattern);
    args.push(root);

    let output = Command::new("grep")
        .args(&args)
        .output()
        .map_err(|e| AuthoringError::Io(format!("could not run grep: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    // Before anything reads it. `printed` and the per-file entries are both
    // built from this string, so filtering once here is what keeps the
    // return and the capture describing the same search.
    let stdout = drop_excluded(&stdout, &exclusions);

    let note = exclusion_note(&exclusions);
    let mut printed = String::new();
    let mut buf = pending::load(workspace_dir)?;

    if stdout.trim().is_empty() {
        // A bounded negative is about the tree it searched, so its marker
        // comes from the search root.
        let m = world.for_path(root_path);
        printed.push_str(&format!("no matches for '{pattern}' in {root}\n"));
        printed.push_str(&format!("({note})\n"));
        buf.push(PendingEntry {
            captured_at: workspace::now_unix(),
            kind: ObservationKind::NoMatch,
            key: search_key(root_path, &m.root),
            label: format!("no-match: {pattern} in {root} — {note}"),
            output: String::new(),
            world_root: m.root,
            world_state: m.state,
            pattern: pattern.to_string(),
        });
    } else {
        printed.push_str(&stdout);
        printed.push_str(&format!("({note})\n"));
        let mut by_file: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
        for line in stdout.lines() {
            if let Some((file, rest)) = line.split_once(':') {
                by_file.entry(file.to_string()).or_default().push(rest.to_string());
            }
        }
        // One whole-search record beside the per-file hits. Without it a
        // search that matched says only which files it hit, and where it
        // was rooted — the thing a census turns on — is unrecoverable.
        // Its marker comes from the search root, exactly as the
        // zero-match record's always has.
        let m = world.for_path(root_path);
        let files = by_file.len();
        buf.push(PendingEntry {
            captured_at: workspace::now_unix(),
            kind: ObservationKind::Search,
            key: search_key(root_path, &m.root),
            label: format!(
                "search: {root} (grep: {pattern}) — {files} file{} matched — {note}",
                if files == 1 { "" } else { "s" }
            ),
            output: String::new(),
            world_root: m.root,
            world_state: m.state,
            pattern: pattern.to_string(),
        });
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
                pattern: pattern.to_string(),
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

/// Kills the process group led by `pid`.
///
/// A *group* rather than a process, and that is the whole mechanism: `run`
/// executes `argv` directly, so `argv[0]` is frequently a shell or a
/// wrapper and the thing that will not stop is its child. The runaway that
/// produced this bound was `bash -c python …` — killing bash alone leaves
/// python holding both pipes, the drain threads reading a stream that
/// never ends, and the call hung exactly as it was before.
///
/// **Unix only.** Elsewhere this is inert and a `run` is unbounded, as it
/// was everywhere before. A signal to a negative pid is not something
/// `std` can express, and no portable equivalent reaches a grandchild.
#[cfg(unix)]
fn stop_process_group(pid: u32) {
    // SAFETY: `kill` takes no pointers and is async-signal-safe. The
    // negative pid names the group led by `pid`, which exists because the
    // child was spawned with `process_group(0)`; `pid` has not been reaped
    // yet at either call site, so it cannot have been recycled onto some
    // other process. SIGKILL rather than SIGTERM because this fires only
    // after the command has already ignored its entire budget, and a
    // process that traps SIGTERM is precisely the case a bound must
    // survive.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn stop_process_group(_pid: u32) {}

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

    let budget = Duration::from_millis(config::run_timeout_ms(Some(workspace_dir)));
    let started = Instant::now();

    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        // Null, never inherited. `run` passes `argv` and nothing else — there
        // is no way for an author to supply input — so a child reading stdin
        // can only block, and on the MCP surface it blocks on the **JSON-RPC
        // transport**: the server's stdin is its request channel. Because that
        // server answers requests serially, one such child stalls every later
        // call rather than only its own, which is how a `tetel run tee …` cost
        // a grounding pass 92 minutes and looked like the design loop being
        // slow. Null makes those commands fail fast and empty instead.
        //
        // Note this leaves a wider hole open: any command that blocks for some
        // *other* reason (a lock, a network call, an interactive prompt) still
        // takes the whole server hostage, because nothing here times out.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The child leads its own process group, which is what makes the bound
    // below reach a *command* rather than a process. `run` executes `argv`
    // directly, so `argv[0]` is frequently a shell or a wrapper and the
    // thing actually burning the CPU is its child: the runaway that cost
    // 1h55m was `bash -c python …`, where killing bash alone would have
    // left python holding both pipes and the drain threads reading from a
    // stream that never ends.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
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

    // Polled on this thread rather than watched from another one, and the
    // reason is a race rather than taste. A watchdog thread has to decide
    // to kill *after* the main thread may already have reaped the child,
    // and a reaped pid can be recycled onto an unrelated process — so the
    // signal that bounds one command can land on something else entirely.
    // Here the kill happens before anything is reaped, so `child.id()`
    // still names this command and nothing else, and there is no shared
    // state to get the ordering wrong.
    //
    // The nap backs off because both ends matter: an `echo` must not pay
    // 50ms of polling latency, and a five-minute command must not wake
    // this thread twelve thousand times.
    //
    // The clock starts only now, with both pipes already draining, so a
    // command cannot spend its budget blocked on a full pipe buffer that
    // nothing is reading yet.
    let mut timed_out = false;
    let mut nap = Duration::from_millis(1);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| AuthoringError::Io(e.to_string()))? {
            break status;
        }
        if started.elapsed() >= budget {
            stop_process_group(child.id());
            timed_out = true;
            break child.wait().map_err(|e| AuthoringError::Io(e.to_string()))?;
        }
        thread::sleep(nap);
        nap = (nap * 2).min(Duration::from_millis(50));
    };
    t_out.join().ok();
    t_err.join().ok();

    // Refused rather than recorded, and the captured bytes are dropped on
    // the floor. A killed command produced an indeterminate prefix of what
    // it would have produced, and a prefix folded into a fact is
    // indistinguishable from complete output ever after — the extent is
    // immutable and it ships in the snapshot. Discarding loses nothing a
    // reader could have trusted, and the refusal is not lost either:
    // `workspace::refuse` appends it to `refusals.log`, which `check`
    // already reports for a fact's own mint window.
    if timed_out {
        let secs = started.elapsed().as_secs();
        return Err(workspace::refuse(
            workspace_dir,
            "run",
            format!(
                "`{cmdline}` did not finish within {}s and was killed, along with anything it \
                 started; nothing was captured. A command that never returns does not fail alone \
                 here — `run` waits for its child, so on the MCP surface every later call queues \
                 behind it. Raise the bound with `tetel config {} <milliseconds>` if this command \
                 honestly takes longer (elapsed: {secs}s).",
                budget.as_secs(),
                config::KEY_RUN_TIMEOUT_MS,
            ),
        ));
    }

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
        pattern: String::new(),
    };
    let mut buf = pending::load(workspace_dir)?;
    buf.push(entry);
    pending::save(workspace_dir, &buf)?;

    Ok(RunOutcome { printed: output_text, exit_code })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reconciliation in [`search_key`] exists because two tools spell
    /// the search root: `fs::canonicalize` here and `git rev-parse
    /// --show-toplevel` in `worldstate`. On the machines this has been run
    /// on they agree, so an end-to-end test cannot reach this branch —
    /// removing the reconciliation entirely leaves the CLI tests green.
    /// A symlinked directory forces the disagreement deterministically.
    #[test]
    fn a_search_root_reached_through_a_symlink_is_keyed_the_way_its_marker_spells_it() {
        let base = std::env::temp_dir().join(format!("tetel-search-key-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let real = base.join("real");
        fs::create_dir_all(&real).unwrap();
        let link = base.join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();

        // The divergence that matters runs this way round. `resolve_key`
        // canonicalizes, so the search side is always canonical; it is
        // *git* that can hand back a spelling with a symlink component,
        // because it answers from the path it was invoked through. So the
        // marker carries the link spelling and the search root is the real
        // directory — one directory, two names.
        //
        // Building `world_root` by canonicalizing instead would make the
        // two sides identical by construction and the assertion vacuous:
        // an earlier draft of this test did exactly that and passed with
        // the reconciliation deleted.
        let world_root = link.display().to_string();

        // Without reconciliation the census predicate — a byte comparison —
        // would refuse an honest worktree-rooted search over a difference
        // that means nothing.
        let key = search_key(&real, &world_root);
        assert_eq!(key, world_root, "same directory, so the marker's spelling wins");

        // A genuinely different directory keeps its own spelling, so the
        // predicate still fails — this is the case the census exists for.
        let sub = real.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let sub_key = search_key(&sub, &world_root);
        assert_ne!(sub_key, world_root, "a subdirectory must not be reconciled into its parent");

        // An unresolvable marker is left alone rather than guessed about.
        assert_eq!(search_key(&real, ""), fs::canonicalize(&real).unwrap().display().to_string());

        let _ = fs::remove_dir_all(&base);
    }

    /// The grep flags cannot be trusted to have done this, so the filter is
    /// tested against output that still contains everything they were meant
    /// to remove — which is exactly what GNU grep hands back.
    fn applied() -> Exclusions {
        Exclusions::Applied {
            memos: vec!["./docs/memo.md".to_string()],
            ignored: vec![(true, "./build".to_string()), (false, "./gen.log".to_string())],
        }
    }

    #[test]
    fn an_excluded_path_is_dropped_even_when_grep_returned_it() {
        // Verbatim shape of the Ubuntu CI failure: every root-spelled
        // pattern was passed and every one of them matched nothing.
        let stdout = "./src/thing.rs:1:fn f() { T(); }\n\
                      ./docs/memo.md:1:prose about T\n\
                      ./other/memo.md:1:unrelated file, same basename, T\n\
                      ./build/gen.txt:1:T generated\n\
                      ./build/sub/deep.txt:1:T nested\n\
                      ./keep/build/real.txt:1:T legitimate\n\
                      ./gen.log:1:T logged\n";
        let kept = drop_excluded(stdout, &applied());
        let lines: Vec<&str> = kept.lines().collect();

        assert!(lines.iter().any(|l| l.starts_with("./src/thing.rs:")), "source must survive");

        assert!(!lines.iter().any(|l| l.starts_with("./docs/memo.md:")), "a rendered memo must go");
        assert!(!lines.iter().any(|l| l.starts_with("./build/")), "an ignored dir must go");
        assert!(!lines.iter().any(|l| l.starts_with("./gen.log:")), "an ignored file must go");

        // The two traps a bare-basename pattern falls into, and the only
        // reason the exclusions are spelled from the root at all.
        assert!(
            lines.iter().any(|l| l.starts_with("./other/memo.md:")),
            "a file sharing a memo's basename with no snapshot is not a memo"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("./keep/build/real.txt:")),
            "`/build` is anchored, so keep/build is not ignored"
        );
    }

    #[test]
    fn a_directory_exclusion_stops_at_a_path_boundary() {
        let e = applied();
        assert!(e.covers("./build"), "the directory itself");
        assert!(e.covers("./build/gen.txt"));
        assert!(e.covers("./build/sub/deep.txt"), "and below its top level");
        // `./buildings` shares a prefix and not a path component.
        assert!(!e.covers("./buildings/x.txt"), "a prefix is not a parent");
        assert!(!e.covers("./keep/build/real.txt"));
    }

    #[test]
    fn a_search_that_excluded_nothing_is_passed_through_untouched() {
        // `None` is forced, not chosen — a file root, or a root inside
        // tetel's own output — and there the caller asked for what they
        // named. Filtering it would withhold what they explicitly wanted.
        let stdout = "./docs/memo.md:1:prose about T\n";
        let e = Exclusions::None { why: "the caller named one file" };
        assert_eq!(drop_excluded(stdout, &e), stdout);
    }
}
