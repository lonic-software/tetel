//! A marker of the actual working-tree state at the moment an
//! observation (`tetel look`/`tetel run`) was captured.
//!
//! This is distinct from a fact's `pin` (see `facts.rs`), which is a
//! content fingerprint over what was captured. Two observations can
//! carry different pins yet still have been taken with the tracked tree
//! in two different, unrecorded states around them (mid-edit, across a
//! `git checkout`, a rebase in flight) — a prototype run produced two
//! facts requiring opposite tree states under one author-typed pin, with
//! nothing in the record to tell them apart short of trusting the note.
//! This marker exists so that comparison never has to rest on note
//! honesty alone.
//!
//! # A marker names a tree, and says which one
//!
//! A [`Marker`] is a pair: the **root** of the working tree it describes
//! and the **state** that tree was in. Both halves are load-bearing, and
//! the second was learned by measurement rather than assumed.
//!
//! The state alone is what this module recorded first, and it does not
//! survive contact with real workspaces: **9 of the 23 workspaces in the
//! store on 2026-08-07 observed more than one git repository** — five of
//! seven adversarial-review passes read a second repository alongside the
//! one under review, and four grounding passes also read a separate
//! worktree. Comparing bare state hashes across those observations
//! reports a difference on every one of them, and the difference means
//! nothing: two repositories are *supposed* to be in different states.
//! Only "one root, two states"
//! is a finding, and recovering that needs the root recorded.
//!
//! # It must describe the tree that was read, not the one you stood in
//!
//! The original implementation ran `git` in the process's own working
//! directory and attached the answer to every observation regardless of
//! what that observation touched. Measured directly on 2026-08-07 with
//! two throwaway repositories, A as the working directory and B as the
//! thing being read:
//!
//! | observation | A | B | marker |
//! |---|---|---|---|
//! | read B | clean | clean | `git:e30fbb1e…` |
//! | read A | clean | clean | `git:e30fbb1e…` |
//! | read B | **dirty** | clean | `git:3a6655a3…` |
//! | read B | dirty | **dirty** | `git:3a6655a3…` |
//!
//! The marker moved when a repository nobody had read changed, and stayed
//! put when the one actually being read changed. On the MCP surface that
//! is worse than useless, because the server's working directory is
//! wherever the client happened to start it — a directory the caller
//! cannot see, set, or reason about. So the marker is resolved **per
//! observation, from what that observation touched**.
//!
//! `run` is the exception, and honestly so: a command names no path in
//! general, and it genuinely executed in this process's working
//! directory, so [`Session::for_cwd`] is the right answer there rather
//! than a fallback. Counting extent entries whose label begins `proc: `,
//! it was **123 of 978 — 12.6%** across the store on 2026-08-07, so a
//! reader should know that some markers describe the directory a command
//! ran in and not a file anyone opened. (The grounding pass, measuring
//! the same thing minutes earlier against a store still being written to,
//! got 119 of 967. Two honest counts of a moving population; the method
//! is stated so either can be re-derived rather than believed.)
//!
//! Read that answer with its ceiling in view: over MCP the working
//! directory is the *server's*, which is wherever the client started it
//! and need not be the repository under design. Three `tetel mcp`
//! servers were running on this machine at once with three unrelated
//! working directories. The marker is then still accurate about where the
//! command ran; it is simply less informative than a reader might assume,
//! and that is a property of `run` over MCP rather than of the marker.
//!
//! An earlier draft of this comment put the share at "106 of 130 in one
//! grounding workspace", roughly seven times the true figure. The 106
//! were not `proc:` entries: 92 of them were grep matches whose keys had
//! been mangled into bare line numbers by a missing `-H` (fixed in
//! `look_grep`, found by the grounding pass over this module's own design
//! memo). Both the count and its cause were wrong.
//!
//! # Implementation choice
//!
//! A hash over `git rev-parse HEAD` and `git diff HEAD --binary`, which
//! together determine every tracked file's content: `HEAD` names the
//! base, the diff names the delta from it. This was picked over "the
//! worktree id" (e.g. the id `git worktree list` assigns) because a
//! worktree id only distinguishes *which* checkout you're in, not whether
//! it's dirty — and the defect this fixes is exactly two facts taken
//! against the same nominal checkout with opposite uncommitted content.
//! The root is `git rev-parse --show-toplevel`, so two checkouts of one
//! repository are correctly two roots.
//!
//! Untracked files are not included, matching the spec's own "tracked
//! files' contents" framing.
//!
//! # Why the cache is per-[`Session`] and not per-process
//!
//! Resolving a marker costs two `git` invocations, and a single
//! `look --grep` across a tree can produce dozens of observations in one
//! call. They are cached — but on a [`Session`] created fresh for each
//! dispatch, never in a `static`. A process-level cache would be a bug
//! rather than an optimisation on the MCP surface, where one server lives
//! for hours across many edits: it would pin the first answer it ever
//! computed and report a tree state that had long since stopped being
//! true, which is the same shape of defect as TET-31's stale binary.
//!
//! # Known limitation
//!
//! Requires a `git` binary on `PATH` and a git repository with at least
//! one commit. Outside of one — or if `git` itself can't be run — the
//! marker degrades to [`NO_GIT`] in both halves. That degraded marker
//! still lets a git-backed observation be told apart from a non-git one,
//! but it cannot distinguish two non-git observations from each other,
//! and grouping by root correctly puts them all in one bucket where no
//! comparison is possible. This is surfaced, not silently masked: every
//! fact's extent carries its marker verbatim (see `facts.rs`), so a
//! [`NO_GIT`] marker is visible for what it is rather than read as a
//! positive guarantee.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::evidence::sha256_hex;

/// Both halves of a marker for an observation outside any git worktree.
pub const NO_GIT: &str = "no-git-worktree";

/// The working tree an observation was taken against: which one, and what
/// state it was in.
///
/// Serializable because a witnessed evidence record carries the markers of
/// the fact it rests on (see `evidence::record_from_fact`) — kept as a
/// structured pair rather than a joined string so no separator has to be
/// reserved out of a filesystem path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Marker {
    /// The worktree root — `git rev-parse --show-toplevel`, or [`NO_GIT`].
    /// Two observations may only have their states compared when this
    /// matches; see the module doc comment.
    pub root: String,
    /// A fingerprint of that tree's tracked content, or [`NO_GIT`].
    pub state: String,
}

impl Marker {
    fn no_git() -> Self {
        Marker { root: NO_GIT.to_string(), state: NO_GIT.to_string() }
    }

    /// Whether this marker says anything a comparison can use.
    pub fn is_git_backed(&self) -> bool {
        self.root != NO_GIT
    }
}

fn run_git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

fn compute(dir: &Path) -> Marker {
    let Some(root) = run_git(dir, &["rev-parse", "--show-toplevel"]) else {
        return Marker::no_git();
    };
    match (run_git(dir, &["rev-parse", "HEAD"]), run_git(dir, &["diff", "HEAD", "--binary"])) {
        (Some(head), Some(diff)) => Marker {
            root: root.trim().to_string(),
            state: format!("git:{}", sha256_hex(&format!("{}\u{0}{}", head.trim(), diff))),
        },
        // A repository with no commits yet: `--show-toplevel` answers but
        // `HEAD` does not. Reported as ungradable rather than as a root
        // with an empty state, which would compare equal to nothing.
        _ => Marker::no_git(),
    }
}

/// One dispatch's worth of marker resolution, with its cache.
///
/// Create one per `look`/`run` call and drop it. See the module doc
/// comment on why this must not become a `static`.
#[derive(Default)]
pub struct Session {
    by_dir: HashMap<PathBuf, Marker>,
}

impl Session {
    pub fn new() -> Self {
        Session::default()
    }

    /// The marker for the tree containing `path` — its own directory when
    /// it is one, its parent otherwise.
    pub fn for_path(&mut self, path: &Path) -> Marker {
        let dir = if path.is_dir() { path.to_path_buf() } else {
            path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
        };
        // Canonicalize so `/tmp/x` and `/private/tmp/x` do not become two
        // roots that never compare, which would silently disable the
        // check on macOS.
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        self.by_dir.entry(dir.clone()).or_insert_with(|| compute(&dir)).clone()
    }

    /// The marker for this process's own working directory — the right
    /// answer for `run`, which names no path but did execute somewhere.
    pub fn for_cwd(&mut self) -> Marker {
        match std::env::current_dir() {
            Ok(dir) => self.for_path(&dir),
            Err(_) => Marker::no_git(),
        }
    }
}

/// One working tree that a memo's facts saw in more than one state.
pub struct RootStates {
    pub root: String,
    /// One entry per distinct state, in first-seen order, each carrying
    /// the ids of the facts that observed the tree in it. Always at least
    /// two entries long — one state is nothing to tell apart.
    pub states: Vec<(String, Vec<String>)>,
}

pub struct TreeReport {
    pub divergent: Vec<RootStates>,
    /// Facts carrying at least one extent entry minted before markers
    /// named their root. Their state was resolved from the process's
    /// working directory rather than from what they read, so it is
    /// comparable with nothing.
    pub ungradable_facts: Vec<String>,
    /// TET-42's fourth promise, first half: every distinct `world_root`
    /// behind at least one extent entry whose label was actually computed
    /// root-relative (`ExtentEntry::root_relative`), in first-seen order.
    ///
    /// A per-entry rule cannot see this on its own — every entry can
    /// relativize correctly against its own root while the roots
    /// disagree with each other, which happens for three shapes capture
    /// time cannot close: a repository elsewhere on the machine observed
    /// directly, a repository nested beneath the shipping worktree, and a
    /// working directory that reached another repository through a
    /// symlink. Zero or one root is the ordinary case; more than one means
    /// this memo's relative labels do not all resolve against the tree a
    /// reader who clones it once will have.
    pub relative_label_roots: Vec<String>,
    /// TET-42's fourth promise, second half: `(fact id, labels)` — every
    /// plain single-file label (a `Path` or `GrepMatch` observation) on
    /// that fact that reads relative — does not begin with `/` — yet
    /// carries no `root_relative` marker, grouped one row per fact rather
    /// than one row per entry.
    ///
    /// This is the residue TET-42 explicitly declined to absolutize: a
    /// caller who spelled a path relative and failed the caller-spelling
    /// condition keeps that relative label verbatim, and a relative label
    /// this design's rule never touched is shape-identical, once
    /// rendered, to one it minted — the marker that tells them apart is a
    /// snapshot field, and neither render site prints it. Reported here
    /// rather than fixed, because fixing it means absolutizing a spelling
    /// this ticket deliberately leaves alone.
    ///
    /// **One entry per fact, not per extent entry** — the same aggregation
    /// `out_of_proof`/`superseded_evidence` already apply to a claim's
    /// stale records, for the analogous reason: a `look --grep` with an
    /// absolute root through a symlinked ancestor fails condition three
    /// for every file it matched, and all of those land on one fact's
    /// extent once `tetel fact` folds them, so an unaggregated list turns
    /// one 200-file search into 200 lines here for a single finding.
    pub unmarked_relative_labels: Vec<(String, Vec<String>)>,
}

/// Which working trees this memo's facts saw in more than one state.
///
/// **This is a record, not a warning.** Two facts observing one
/// repository in two states is the normal condition of any design that
/// runs longer than an edit, and nothing here is a defect to discharge.
/// What it answers is the question the ledger could not answer at all
/// before: *were these two facts taken against the same tree?* A reader
/// looking at two facts that assert opposite things about the same code
/// — the case this whole marker exists for — can now see whether they
/// disagree about the world or about the same world.
///
/// Grouping by root is load-bearing rather than tidy. Half the
/// workspaces in the store read more than one repository, and comparing
/// states across two of them reports a difference on every single one:
/// two repositories are supposed to be in different states.
pub fn tree_report(facts: &[crate::facts::Fact]) -> TreeReport {
    let mut order: Vec<String> = Vec::new();
    let mut by_root: HashMap<String, Vec<(String, Vec<String>)>> = HashMap::new();
    let mut ungradable: Vec<String> = Vec::new();
    let mut relative_label_roots: Vec<String> = Vec::new();
    let mut unmarked_relative_labels: Vec<(String, Vec<String>)> = Vec::new();

    for fact in facts {
        let mut ungraded_here = false;
        for entry in &fact.extent {
            // TET-42, first half: every root behind an actually-relativized
            // label — independent of the divergence/ungradable bookkeeping
            // below, which is about `world_state`, not about labels.
            if entry.root_relative && !entry.world_root.is_empty() && !relative_label_roots.contains(&entry.world_root)
            {
                relative_label_roots.push(entry.world_root.clone());
            }
            // TET-42, second half: a plain single-file label reading
            // relative with no marker — restricted to `Path`/`GrepMatch`,
            // whose whole label *is* the path (plus an optional line-range
            // suffix). A `Search`/`NoMatch` label is a sentence embedding a
            // root, not a bare path, and would misreport as "relative"
            // merely for not opening with `/`; a `Proc` label is a command
            // line, to which relative/absolute does not apply at all.
            if !entry.root_relative
                && matches!(
                    entry.kind,
                    Some(crate::pending::ObservationKind::Path) | Some(crate::pending::ObservationKind::GrepMatch)
                )
                && !entry.label.is_empty()
                && !entry.label.starts_with('/')
                && !Path::new(&entry.label).is_absolute()
            {
                // One row per fact, not per entry — see this field's
                // own doc comment for why an unaggregated list is unsound
                // at the volume a single big search can produce.
                match unmarked_relative_labels.iter_mut().find(|(id, _)| *id == fact.id) {
                    Some((_, labels)) => labels.push(entry.label.clone()),
                    None => unmarked_relative_labels.push((fact.id.clone(), vec![entry.label.clone()])),
                }
            }
            if entry.world_root.is_empty() {
                if !ungraded_here {
                    ungradable.push(fact.id.clone());
                    ungraded_here = true;
                }
                continue;
            }
            if entry.world_root == NO_GIT {
                continue;
            }
            if !by_root.contains_key(&entry.world_root) {
                order.push(entry.world_root.clone());
            }
            let states = by_root.entry(entry.world_root.clone()).or_default();
            match states.iter_mut().find(|(s, _)| *s == entry.world_state) {
                Some((_, ids)) => {
                    if !ids.contains(&fact.id) {
                        ids.push(fact.id.clone());
                    }
                }
                None => states.push((entry.world_state.clone(), vec![fact.id.clone()])),
            }
        }
    }

    let divergent = order
        .into_iter()
        .filter_map(|root| {
            let states = by_root.remove(&root)?;
            (states.len() > 1).then_some(RootStates { root, states })
        })
        .collect();

    TreeReport { divergent, ungradable_facts: ungradable, relative_label_roots, unmarked_relative_labels }
}

/// One fact whose captured extent no longer matches the tree it was read
/// from — one row per fact, the aggregation `unmarked_relative_labels` uses.
pub struct StaleExtent {
    pub fact_id: String,
    /// One line per subject: its label, what happened to it, and the tree
    /// state it was captured at.
    pub subjects: Vec<String>,
}

/// What [`extent_drift`] found. A fact can appear in both lists: its extent
/// is judged per observation.
#[derive(Default)]
pub struct ExtentDrift {
    pub stale: Vec<StaleExtent>,
    /// `(why it could not be compared, fact ids)`, in first-seen order.
    /// Unknown is never stale: a memo read on a machine without the tree
    /// it describes has every fact here, and that says nothing about them.
    pub unknown: Vec<(String, Vec<String>)>,
}

/// What a `Path` observation captured, read back off its label. The label is
/// the only record of it, so a file really named `x lines 1-2` is misread; the
/// report's standing non-coverage says so. A field recording the shape would
/// still need this parser for every fact already minted, and two paths are not
/// worth it for a file name that rare.
#[derive(Debug, PartialEq)]
enum PathCapture {
    Whole,
    Range,
    /// One over-long line, cut to its first bytes.
    Cut,
    /// A range starting past the end of the file, at this line.
    NothingInRange(usize),
}

fn path_capture(label: &str) -> PathCapture {
    let Some(at) = label.rfind(" lines ") else { return PathCapture::Whole };
    let rest = &label[at + " lines ".len()..];
    let digits = |s: &str| s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let first = digits(rest);
    let Some(after_dash) = rest[first..].strip_prefix('-') else { return PathCapture::Whole };
    let second = digits(after_dash);
    if first == 0 || second == 0 {
        return PathCapture::Whole;
    }
    match &after_dash[second..] {
        "" => PathCapture::Range,
        tail if tail.starts_with(", cut to its first ") => PathCapture::Cut,
        tail if tail.starts_with(" (file has ") => {
            rest[..first].parse().map_or(PathCapture::Whole, PathCapture::NothingInRange)
        }
        _ => PathCapture::Whole,
    }
}

/// Whether what `entry` captured still stands in `current`, the file's text
/// now. A range matches wherever its lines still sit together: an insertion
/// above it moves the subject without changing it, and reporting that would
/// bury the real rows under every edit to the file.
fn still_matches(kind: crate::pending::ObservationKind, label: &str, captured: &str, current: &str) -> bool {
    use crate::pending::ObservationKind::GrepMatch;
    if kind == GrepMatch {
        // `split_grep_line` left each hit as `<line number>:<text>`. Every
        // hit must survive, duplicates counted: `dup` captured twice needs two
        // lines still. A new hit in this file is the search's question.
        let mut left: HashMap<&str, usize> = HashMap::new();
        for line in current.lines() {
            *left.entry(line).or_default() += 1;
        }
        return captured.lines().all(|hit| {
            let text = hit.split_once(':').map_or(hit, |(_, text)| text);
            match left.get_mut(text) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    true
                }
                _ => false,
            }
        });
    }
    match path_capture(label) {
        PathCapture::Whole => current == captured,
        PathCapture::Cut => current.contains(captured),
        PathCapture::NothingInRange(first) => current.lines().count() < first,
        PathCapture::Range => {
            // An empty capture in range is one blank line, a range past the end
            // having its own label, or an empty file whose own name reads as
            // a range. An empty file today stands for both: the second must
            // not read as changed when nothing touched it.
            if captured.is_empty() && current.is_empty() {
                return true;
            }
            let want: Vec<&str> = if captured.is_empty() { vec![""] } else { captured.lines().collect() };
            let have: Vec<&str> = current.lines().collect();
            have.windows(want.len()).any(|w| w == want.as_slice())
        }
    }
}

/// A memo chooses the paths `check` reads, and may have just been received, so
/// the read is bounded. A special file that reports as regular and never ends
/// can still block it.
const MAX_COMPARED_BYTES: u64 = 64 * 1024 * 1024;

/// A whole capture equals the file byte for byte, so a length that differs
/// settles it without a read.
fn whole_capture_cannot_match(kind: crate::pending::ObservationKind, label: &str, captured: &str, len: u64) -> bool {
    kind == crate::pending::ObservationKind::Path && path_capture(label) == PathCapture::Whole && len != captured.len() as u64
}

fn read_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(MAX_COMPARED_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn too_large(key: &str) -> String {
    format!("{key} is larger than the {} MiB compared", MAX_COMPARED_BYTES / (1024 * 1024))
}

/// Whether a `GrepMatch` capture reads as `grep -n` hits. A file name holding
/// a `:` splits early, leaving `b.txt:1:text` and a key naming some other path.
fn hits_are_well_formed(captured: &str) -> bool {
    captured.lines().all(|hit| {
        hit.split_once(':').is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn short(state: &str) -> &str {
    state.get(..16).unwrap_or(state)
}

/// Whether `root` is still the top of a git worktree. Only the root, never
/// [`compute`]: a dirty non-UTF-8 diff makes the state unresolvable on a tree
/// that is plainly here, and `git diff` runs what a repository's config names,
/// at a path `check` was handed by a memo it may have just received.
fn root_is_here(root: &str) -> bool {
    run_git(Path::new(root), &["rev-parse", "--show-toplevel"]).is_some_and(|top| top.trim() == root)
}

/// Which facts' captured extents differ from the same path in their
/// `world_root` today.
///
/// [`tree_report`] compares facts to each other; this compares each one to
/// the tree. It reads the file named by each `Path` and `GrepMatch` entry and
/// nothing else: a `Proc` entry names a command, not a subject, and a
/// `Search`/`NoMatch` entry would need its search run again, which the
/// report's standing non-coverage says.
///
/// It compares content rather than tree states, because a state hash moves
/// on any edit anywhere in the tree and would call every fact stale.
pub fn extent_drift(facts: &[crate::facts::Fact]) -> ExtentDrift {
    use crate::pending::ObservationKind::{GrepMatch, Path as PathKind};
    let mut drift = ExtentDrift::default();
    let mut roots: HashMap<String, bool> = HashMap::new();
    let mut unknown = |reason: String, id: &str| match drift.unknown.iter_mut().find(|(r, _)| *r == reason) {
        Some((_, ids)) => {
            if !ids.iter().any(|i| i == id) {
                ids.push(id.to_string());
            }
        }
        None => drift.unknown.push((reason, vec![id.to_string()])),
    };
    let mut stale = Vec::new();

    for fact in facts {
        let outputs = fact.observation_outputs();
        let mut next_output = 0usize;
        let mut subjects = Vec::new();
        for entry in &fact.extent {
            // Keeps `next_output` aligned: `observation_outputs` skips the
            // empty captures, so only a non-empty one consumes a slot.
            let captured = match (&outputs, entry.out_len) {
                (Some(_), Some(0)) => Some(""),
                (Some(all), Some(_)) => {
                    next_output += 1;
                    all.get(next_output - 1).copied()
                }
                _ => None,
            };
            let kind = match entry.kind {
                Some(kind @ (PathKind | GrepMatch)) => kind,
                // A command or a whole search names no file to compare.
                Some(_) => continue,
                None if entry.label.starts_with("proc: ") => continue,
                None => {
                    unknown("minted before an observation recorded its kind".to_string(), &fact.id);
                    continue;
                }
            };
            if entry.world_root.is_empty() {
                unknown("minted before an observation's tree marker named its tree".to_string(), &fact.id);
                continue;
            }
            if entry.world_root == NO_GIT {
                unknown("observed outside any git worktree".to_string(), &fact.id);
                continue;
            }
            if !*roots.entry(entry.world_root.clone()).or_insert_with(|| root_is_here(&entry.world_root)) {
                unknown(format!("{} could not be confirmed as a git worktree here", entry.world_root), &fact.id);
                continue;
            }
            let Some(captured) = captured else {
                unknown("minted before an extent recorded each observation's length".to_string(), &fact.id);
                continue;
            };
            let path = Path::new(&entry.key);
            if !path.is_absolute() {
                unknown(format!("{} does not name the subject: it is not an absolute path", entry.key), &fact.id);
                continue;
            }
            if kind == GrepMatch && !hits_are_well_formed(captured) {
                unknown(
                    format!("{} does not name the subject: a hit has no line number, as a file name holding a `:` would leave it", entry.key),
                    &fact.id,
                );
                continue;
            }
            // Asked before the read: a FIFO put where a file was would block
            // the read forever (TET-79).
            // `Err` is why it could not tell; `Ok(None)` is "still stands".
            let verdict: Result<Option<&str>, String> = match std::fs::metadata(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Some("no longer exists")),
                Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => Ok(Some("no longer exists")),
                Err(_) => Err(format!("{} could not be read here", entry.key)),
                Ok(meta) if !meta.is_file() => Ok(Some("is no longer a regular file")),
                Ok(meta) if meta.len() > MAX_COMPARED_BYTES => {
                    if whole_capture_cannot_match(kind, &entry.label, captured, meta.len()) {
                        Ok(Some("differs"))
                    } else {
                        Err(too_large(&entry.key))
                    }
                }
                Ok(_) => match read_bounded(path) {
                    Err(_) => Err(format!("{} could not be read here", entry.key)),
                    Ok(bytes) if bytes.len() as u64 > MAX_COMPARED_BYTES => Err(too_large(&entry.key)),
                    Ok(bytes) => {
                        // `look` refuses a non-UTF-8 file, but a grep hit is
                        // captured lossily, so it compares the same way.
                        let current = match (kind, String::from_utf8_lossy(&bytes)) {
                            (PathKind, std::borrow::Cow::Owned(_)) => None,
                            (_, text) => Some(text),
                        };
                        let stands = current.is_some_and(|c| still_matches(kind, &entry.label, captured, &c));
                        Ok((!stands).then_some("differs"))
                    }
                },
            };
            match verdict {
                Ok(Some(what)) => subjects.push(format!(
                    "{} {what} (captured at {})",
                    entry.label,
                    short(&entry.world_state)
                )),
                Ok(None) => {}
                Err(reason) => unknown(reason, &fact.id),
            }
        }
        if !subjects.is_empty() {
            stale.push(StaleExtent { fact_id: fact.id.clone(), subjects });
        }
    }
    drift.stale = stale;
    drift
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_capture_reads_every_label_look_path_writes() {
        assert_eq!(path_capture("a.rs"), PathCapture::Whole);
        assert_eq!(path_capture("a.rs lines 2-3"), PathCapture::Range);
        assert_eq!(path_capture("a.rs lines 2-2, cut to its first 9 of 90 bytes"), PathCapture::Cut);
        assert_eq!(
            path_capture("a.rs lines 5-9 (file has 3 line(s); nothing in range)"),
            PathCapture::NothingInRange(5)
        );
    }

    #[test]
    fn only_a_whole_capture_of_another_length_is_settled_without_a_read() {
        use crate::pending::ObservationKind::{GrepMatch, Path};
        assert!(whole_capture_cannot_match(Path, "a.rs", "ab", 3));
        assert!(!whole_capture_cannot_match(Path, "a.rs", "ab", 2));
        assert!(!whole_capture_cannot_match(Path, "a.rs lines 1-1", "ab", 3));
        assert!(!whole_capture_cannot_match(GrepMatch, "a.rs", "ab", 3));
    }

    fn fact(id: &str, extent: &[(&str, &str)]) -> crate::facts::Fact {
        crate::facts::Fact {
            id: id.to_string(),
            note: String::new(),
            extent: extent
                .iter()
                .map(|(root, state)| crate::facts::ExtentEntry {
                    key: String::new(),
                    label: String::new(),
                    world_root: root.to_string(),
                    world_state: state.to_string(),
                    kind: None,
                    pattern: String::new(),
                    out_len: None,
                    matcher: None,
                    root_relative: false,
                    ignored: Vec::new(),
                })
                .collect(),
            output: String::new(),
            pin: String::new(),
            revisions: 0,
        }
    }

    /// A fact carrying one extent entry with an explicit `kind`, `label`
    /// and `root_relative` marker — what [`tree_report`]'s TET-42 rows
    /// actually read, as opposed to [`fact`]'s bare `(root, state)` pairs.
    fn fact_with_label(
        id: &str,
        kind: crate::pending::ObservationKind,
        label: &str,
        root_relative: bool,
        world_root: &str,
    ) -> crate::facts::Fact {
        fact_with_entries(id, &[(kind, label, root_relative, world_root)])
    }

    /// A fact carrying several extent entries at once — what a single
    /// `look --grep` folds into one fact when it matches more than one
    /// file, and what the aggregation test below needs to construct the
    /// shape it is pinning.
    fn fact_with_entries(
        id: &str,
        entries: &[(crate::pending::ObservationKind, &str, bool, &str)],
    ) -> crate::facts::Fact {
        crate::facts::Fact {
            id: id.to_string(),
            note: String::new(),
            extent: entries
                .iter()
                .map(|(kind, label, root_relative, world_root)| crate::facts::ExtentEntry {
                    key: String::new(),
                    label: label.to_string(),
                    world_root: world_root.to_string(),
                    world_state: String::new(),
                    kind: Some(*kind),
                    pattern: String::new(),
                    out_len: None,
                    matcher: None,
                    root_relative: *root_relative,
                    ignored: Vec::new(),
                })
                .collect(),
            output: String::new(),
            pin: String::new(),
            revisions: 0,
        }
    }

    #[test]
    fn relative_label_roots_lists_every_distinct_root_behind_a_relativized_label() {
        use crate::pending::ObservationKind::Path;
        let r = tree_report(&[
            fact_with_label("F1", Path, "sub/f.txt", true, "/repoA"),
            fact_with_label("F2", Path, "x.txt", true, "/repoB"),
            // A second entry anchored to the same root as F1 must not
            // duplicate it.
            fact_with_label("F3", Path, "sub/g.txt", true, "/repoA"),
        ]);
        assert_eq!(r.relative_label_roots, vec!["/repoA".to_string(), "/repoB".to_string()]);
    }

    #[test]
    fn relative_label_roots_is_empty_when_nothing_was_relativized() {
        use crate::pending::ObservationKind::Path;
        let r = tree_report(&[fact_with_label("F1", Path, "/abs/f.txt", false, "/repoA")]);
        assert!(r.relative_label_roots.is_empty());
    }

    #[test]
    fn unmarked_relative_labels_flags_a_bare_relative_path_with_no_marker() {
        use crate::pending::ObservationKind::Path;
        // TET-42's own declined promise: a caller-spelled relative label
        // this design's rule never touched reads, once rendered, exactly
        // like a root-relative one — the marker is the only thing that
        // tells them apart, and this is what surfaces its absence.
        let r = tree_report(&[fact_with_label("F1", Path, "../otherrepo/x.txt", false, "/otherrepo")]);
        assert_eq!(r.unmarked_relative_labels, vec![("F1".to_string(), vec!["../otherrepo/x.txt".to_string()])]);
    }

    #[test]
    fn unmarked_relative_labels_groups_by_fact_not_by_entry() {
        use crate::pending::ObservationKind::{GrepMatch, Path};
        // The shape a single big `look --grep` produces: many matched
        // files, each its own extent entry, all folded into one fact by
        // `tetel fact`. An unaggregated list would print one line per
        // entry here; the aggregated report owes exactly one row for F1.
        let r = tree_report(&[
            fact_with_entries(
                "F1",
                &[
                    (Path, "a.txt", false, "/repo"),
                    (GrepMatch, "b.txt", false, "/repo"),
                    (GrepMatch, "c.txt", false, "/repo"),
                ],
            ),
            fact_with_label("F2", Path, "d.txt", false, "/repo"),
        ]);
        assert_eq!(r.unmarked_relative_labels.len(), 2, "one row per fact: {:?}", r.unmarked_relative_labels);
        let f1 = r.unmarked_relative_labels.iter().find(|(id, _)| id == "F1").unwrap();
        assert_eq!(f1.1, vec!["a.txt".to_string(), "b.txt".to_string(), "c.txt".to_string()]);
        let f2 = r.unmarked_relative_labels.iter().find(|(id, _)| id == "F2").unwrap();
        assert_eq!(f2.1, vec!["d.txt".to_string()]);
    }

    #[test]
    fn unmarked_relative_labels_ignores_an_absolute_label() {
        use crate::pending::ObservationKind::Path;
        let r = tree_report(&[fact_with_label("F1", Path, "/abs/f.txt", false, "no-git-worktree")]);
        assert!(r.unmarked_relative_labels.is_empty());
    }

    #[test]
    fn unmarked_relative_labels_ignores_search_no_match_and_proc_labels() {
        // A `Search`/`NoMatch` label is a sentence embedding a root, not a
        // bare path — it would misreport as "relative" merely for not
        // opening with `/`. A `Proc` label is a command line, to which
        // relative/absolute does not apply at all. Neither is a candidate
        // for this check, regardless of what its `root_relative` marker
        // says or what its label text looks like.
        use crate::pending::ObservationKind::{NoMatch, Proc, Search};
        let r = tree_report(&[
            fact_with_label("F1", Search, "search: . (grep (ERE): x) — 0 files matched — no exclusions", false, "/repo"),
            fact_with_label("F2", NoMatch, "no-match (ERE): x in . — no exclusions", false, "/repo"),
            fact_with_label("F3", Proc, "proc: echo hi (exit 0)", false, "no-git-worktree"),
        ]);
        assert!(r.unmarked_relative_labels.is_empty(), "{:?}", r.unmarked_relative_labels);
    }

    #[test]
    fn one_tree_seen_in_two_states_is_reported_with_who_saw_what() {
        let r = tree_report(&[
            fact("F1", &[("/repo", "git:aaa")]),
            fact("F2", &[("/repo", "git:bbb")]),
            fact("F3", &[("/repo", "git:aaa")]),
        ]);
        assert_eq!(r.divergent.len(), 1);
        assert_eq!(r.divergent[0].root, "/repo");
        assert_eq!(
            r.divergent[0].states,
            vec![
                ("git:aaa".to_string(), vec!["F1".to_string(), "F3".to_string()]),
                ("git:bbb".to_string(), vec!["F2".to_string()]),
            ]
        );
    }

    #[test]
    fn two_repositories_in_their_own_states_are_not_a_divergence() {
        // The measured shape: 9 of 23 workspaces read more than one
        // repository. Without grouping by root this fires on all of them
        // and means nothing.
        let r = tree_report(&[
            fact("F1", &[("/a", "git:aaa"), ("/b", "git:bbb")]),
            fact("F2", &[("/a", "git:aaa"), ("/b", "git:bbb")]),
        ]);
        assert!(r.divergent.is_empty(), "two repositories are supposed to differ from each other");
    }

    #[test]
    fn one_fact_spanning_two_states_of_one_tree_is_reported_against_itself() {
        // The tree moved between two observations inside a single mint
        // window — the fact's own extent disagrees about the world.
        let r = tree_report(&[fact("F1", &[("/repo", "git:aaa"), ("/repo", "git:bbb")])]);
        assert_eq!(r.divergent.len(), 1);
        assert_eq!(r.divergent[0].states.len(), 2);
        assert_eq!(r.divergent[0].states[0].1, vec!["F1".to_string()]);
        assert_eq!(r.divergent[0].states[1].1, vec!["F1".to_string()]);
    }

    #[test]
    fn markers_from_before_the_root_existed_are_ungradable_not_clean() {
        let r = tree_report(&[fact("F1", &[("", "git:aaa")]), fact("F2", &[("", "git:bbb")])]);
        assert!(r.divergent.is_empty(), "a state with no root is comparable with nothing");
        assert_eq!(r.ungradable_facts, vec!["F1".to_string(), "F2".to_string()]);
    }

    #[test]
    fn a_fact_listed_once_no_matter_how_many_entries_lack_a_root() {
        let r = tree_report(&[fact("F1", &[("", "x"), ("", "y"), ("/repo", "git:aaa")])]);
        assert_eq!(r.ungradable_facts, vec!["F1".to_string()]);
    }

    #[test]
    fn observations_outside_any_repository_never_diverge() {
        let r = tree_report(&[fact("F1", &[(NO_GIT, NO_GIT)]), fact("F2", &[(NO_GIT, NO_GIT)])]);
        assert!(r.divergent.is_empty());
        assert!(r.ungradable_facts.is_empty(), "no-git is a known answer, not a missing one");
    }

    #[test]
    fn resolution_never_panics_and_always_answers() {
        let mut s = Session::new();
        let m = s.for_cwd();
        assert!(!m.root.is_empty());
        assert!(!m.state.is_empty());
    }

    #[test]
    fn a_path_outside_any_repository_degrades_in_both_halves() {
        let mut s = Session::new();
        let m = s.for_path(Path::new("/"));
        // `/` is not inside a git worktree on any machine this runs on.
        assert_eq!(m.root, NO_GIT);
        assert_eq!(m.state, NO_GIT);
        assert!(!m.is_git_backed());
    }

    #[test]
    fn two_files_in_one_repository_share_a_marker_and_cost_one_resolution() {
        let mut s = Session::new();
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let a = s.for_path(&here.join("src").join("worldstate.rs"));
        let b = s.for_path(&here.join("src").join("facts.rs"));
        assert_eq!(a, b, "two files in one repository must not look like two trees");
        assert_eq!(s.by_dir.len(), 1, "one directory, one cached resolution");
    }

    #[test]
    fn a_subdirectory_resolves_to_the_repository_root_not_itself() {
        let mut s = Session::new();
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let deep = s.for_path(&here.join("src").join("facts.rs"));
        let shallow = s.for_path(&here.join("Cargo.toml"));
        if deep.is_git_backed() {
            assert_eq!(
                deep.root, shallow.root,
                "a file in src/ and one at the top must report the same worktree"
            );
            assert_eq!(deep.state, shallow.state);
        }
    }
}
