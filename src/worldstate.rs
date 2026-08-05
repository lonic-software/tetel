//! A marker of the actual working-tree state at the moment an
//! observation (`tetel look`/`tetel run`) was captured.
//!
//! This is distinct from a fact's `pin` (see `facts.rs`), which is a
//! content fingerprint over what was captured. Two observations can
//! carry different pins yet still have been taken with the *tracked*
//! tree in two different, unrecorded states around them (mid-edit,
//! across a `git checkout`, a rebase in flight) — a prototype run
//! produced two facts requiring opposite tree states under one
//! author-typed pin, with nothing in the record to tell them apart
//! short of trusting the note. This marker exists so that comparison
//! never has to rest on note honesty alone.
//!
//! # Implementation choice
//!
//! A hash over `git rev-parse HEAD` and `git diff HEAD --binary`, which
//! together determine every tracked file's content: `HEAD` names the
//! base, the diff names the delta from it. This was picked over "the
//! worktree id" (e.g. `git rev-parse --show-toplevel`, or the id
//! `git worktree list` assigns) because a worktree id only distinguishes
//! *which* checkout you're in, not whether it's dirty — and the defect
//! this fixes is exactly two facts taken against the same nominal
//! checkout with opposite uncommitted content. Untracked files are not
//! included, matching the spec's own "tracked files' contents" framing.
//!
//! # Known limitation
//!
//! Requires a `git` binary on `PATH` and a git repository with at least
//! one commit. Outside of one — or if `git` itself can't be run — the
//! marker degrades to the fixed string [`NO_GIT_MARKER`]. That degraded
//! marker still lets a git-backed observation be told apart from a
//! non-git one, but it cannot distinguish two non-git observations from
//! each other. This is surfaced, not silently masked: every fact's
//! extent carries its marker verbatim (see `facts.rs`), so a `NO_GIT_MARKER`
//! is visible for what it is rather than read as a positive guarantee.

use crate::evidence::sha256_hex;
use std::process::Command;

pub const NO_GIT_MARKER: &str = "no-git-worktree";

fn run_git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// Compute the marker for the current process's working directory.
pub fn compute() -> String {
    match (run_git(&["rev-parse", "HEAD"]), run_git(&["diff", "HEAD", "--binary"])) {
        (Some(head), Some(diff)) => {
            format!("git:{}", sha256_hex(&format!("{}\u{0}{}", head.trim(), diff)))
        }
        _ => NO_GIT_MARKER.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_never_panics_and_returns_something() {
        // Whatever directory `cargo test` runs from, this must not
        // panic — it either finds a git-backed answer or degrades to
        // the documented fallback.
        let marker = compute();
        assert!(!marker.is_empty());
    }
}
