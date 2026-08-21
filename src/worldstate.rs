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

    for fact in facts {
        let mut ungraded_here = false;
        for entry in &fact.extent {
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

    TreeReport { divergent, ungradable_facts: ungradable }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                })
                .collect(),
            output: String::new(),
            pin: String::new(),
            revisions: 0,
        }
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
