//! Shipped provenance: the workspace snapshot that travels with a
//! rendered document, and the drift check that proves the two still
//! agree.
//!
//! # Why a rendered document is not self-contained
//!
//! `render` writes prose, `*cites: …*` markers, and a claim ledger — and
//! nothing else. A fact's captured extent, its command output, and its
//! world-state pin never appear in the document at all. The ids in those
//! markers are workspace-relative by contract (see [`crate::workspace`]),
//! meaning only what the minting workspace says they mean.
//!
//! So a memo committed without its workspace is a document whose every
//! citation is a dangling pointer: the repository holds text asserting
//! that evidence exists, and cannot produce it. The snapshot is what
//! makes the citation resolvable by someone who was not there.
//!
//! # Why this is not the `.tetel/` directory the workspace module rejects
//!
//! [`crate::workspace`] refuses to put state "inside the repository being
//! authored", and that still holds — for two reasons that are worth
//! keeping distinct, because only the second is about tidiness.
//!
//! The first is an observer effect. `look` and `run` capture what they
//! find in the tree under design, including `git status --porcelain` and
//! a world-state pin derived from that tree. Writing tetel's own working
//! state into that tree changes what tetel then observes about it — the
//! instrument would be recording its own presence. That argument is
//! specific to the tree under design and does not transfer to the
//! repository the *memo* lives in.
//!
//! The second is temporal, and it is the one that generalises: live
//! authoring state churns on every command, exists before any memo does,
//! and carries half-consumed intermediates (a pending buffer of
//! observations not yet minted into facts). None of that belongs in any
//! repository's history.
//!
//! A snapshot is neither. It is written once, by the same act that writes
//! the memo, from a workspace whose pending buffer is empty; it is
//! immutable thereafter; and it sits beside the memo under the same
//! `<memo>.<suffix>` convention [`crate::evidence`] already established
//! for `<memo>.evidence.jsonl`. The distinction that matters is not
//! planning-repo versus code-repo — it is working state versus shipped
//! record.
//!
//! # What drift means
//!
//! Because `render` is deterministic in the workspace alone, a snapshot
//! either re-renders its memo byte-for-byte or it does not. A mismatch
//! means the committed text and the record it claims to rest on have
//! diverged — the memo was hand-edited after rendering, or the workspace
//! moved on without the memo being re-rendered. Either way a reader
//! following a citation would land somewhere the text was never produced
//! from, which is precisely the failure this tool exists to prevent.
//!
//! This commits the project to one policy, stated here so it is not
//! rediscovered: **a rendered memo is never hand-edited.** Corrections go
//! through `prose --revise` and a re-render.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The files a snapshot carries. Enumerated rather than copied
/// wholesale so that a future workspace file is a deliberate decision to
/// ship or withhold, not an accident of `cp -r`.
///
/// `pending.json` is included even though `render` never reads it: a
/// non-empty buffer at snapshot time is a fact about how the document was
/// finished, and silently dropping it would hide it.
const SNAPSHOT_FILES: [&str; 8] = [
    "facts.jsonl",
    "claims.jsonl",
    "prose.jsonl",
    // Shipped, not withheld: without it `check` can see the rendered
    // target rows but nothing to verify them against, and the census
    // refusal would hold only inside the workspace that authored the
    // memo — exactly the reviewer-of-a-committed-document case the
    // snapshot exists for.
    "targets.jsonl",
    "counters.json",
    "pending.json",
    "refusals.log",
    // Without this, `check` cannot tell who authored the memo, and so
    // cannot tell an author grounding their own claims from an
    // independent pass grounding them. That is the distinction the whole
    // mechanism exists for — 78% scope-equal self-grounded against 33%
    // independent — and it was invisible until this file shipped.
    "identity.json",
];

/// A memo's snapshot directory: `<memo>.tetel`, sitting next to it —
/// the same convention as [`crate::evidence::evidence_path`].
pub fn snapshot_path(memo: &Path) -> PathBuf {
    let mut s = memo.as_os_str().to_os_string();
    s.push(".tetel");
    PathBuf::from(s)
}

/// Copy `workspace_dir`'s shipped files into `<memo>.tetel/`.
///
/// Existing files are overwritten: re-rendering a memo replaces its
/// snapshot, since the snapshot's whole purpose is to match the memo
/// currently on disk. A file absent from the workspace is skipped rather
/// than erroring — a workspace with no refusals has no refusals log.
///
/// # Why this mints the workspace identity itself
///
/// `identity.json` is the one entry in [`SNAPSHOT_FILES`] that a
/// workspace may not have yet, and skipping it is not harmless: without
/// it `check` cannot tell an author grounding their own claims from an
/// independent pass, which is the distinction the whole mechanism exists
/// for.
///
/// It used to be minted by the caller, and only one of the two callers
/// did it. Every memo authored over MCP therefore shipped a snapshot with
/// no identity, and the self-versus-independent report was silently
/// unavailable for exactly the surface the authoring agents use — while
/// grounding workspaces got one anyway, via `record`, which hid the
/// asymmetry from the side most likely to be inspected.
///
/// So the precondition lives here, with the code that depends on it,
/// rather than in each caller. That removes the pair instead of
/// synchronising it: a third caller cannot reintroduce the bug by
/// forgetting, because there is nothing left to forget. Minting is
/// idempotent — a workspace that already has an identity keeps it, so
/// this can never re-date a pass.
pub fn write(memo: &Path, workspace_dir: &Path) -> io::Result<()> {
    crate::workspace::identity(workspace_dir)?;
    let dir = snapshot_path(memo);
    fs::create_dir_all(&dir)?;
    for name in SNAPSHOT_FILES {
        let src = workspace_dir.join(name);
        if src.exists() {
            fs::copy(&src, dir.join(name))?;
        }
    }
    Ok(())
}

/// Whether the workspace still holds observations that were never minted
/// into facts. Reported at snapshot time rather than refused: an author
/// may have deliberately looked at something they chose not to cite, and
/// only they can tell that from having forgotten to mint it.
pub fn pending_count(workspace_dir: &Path) -> usize {
    let Ok(raw) = fs::read_to_string(workspace_dir.join("pending.json")) else {
        return 0;
    };
    serde_json::from_str::<Vec<serde_json::Value>>(&raw)
        .map(|v| v.len())
        .unwrap_or(0)
}

/// What a memo's provenance looks like on disk.
pub enum Provenance {
    /// No `<memo>.tetel/` beside the memo. Only reportable when the memo
    /// carries citations — a document with no citations owes no record.
    Missing,
    /// The snapshot re-renders the memo exactly.
    Matches,
    /// The snapshot renders something other than the memo's own bytes.
    Drifted {
        /// 1-based line of the first difference, for a reader to look at.
        /// `None` when the two agree line-for-line but differ in length.
        first_diff_line: Option<usize>,
        snapshot_lines: usize,
        memo_lines: usize,
    },
    /// A snapshot exists but could not be rendered from — malformed or
    /// unreadable. Never silently treated as "matches".
    Unreadable(String),
}

/// Compare a memo's committed bytes against what its snapshot renders.
///
/// `memo_source` is passed in rather than re-read so this grades exactly
/// the bytes the rest of `check` graded.
pub fn check(memo: &Path, memo_source: &str) -> Provenance {
    let dir = snapshot_path(memo);
    if !dir.is_dir() {
        return Provenance::Missing;
    }
    let rendered = match crate::compose::render(&dir) {
        Ok(r) => r,
        Err(e) => return Provenance::Unreadable(e.to_string()),
    };
    if rendered == memo_source {
        return Provenance::Matches;
    }
    let first_diff_line = rendered
        .lines()
        .zip(memo_source.lines())
        .position(|(a, b)| a != b)
        .map(|i| i + 1);
    Provenance::Drifted {
        first_diff_line,
        snapshot_lines: rendered.lines().count(),
        memo_lines: memo_source.lines().count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_path_appends_to_the_whole_filename() {
        // Not `with_extension`, which would turn `memo.md` into
        // `memo.tetel` and collide with a different memo's name.
        assert_eq!(
            snapshot_path(Path::new("/x/memo.md")),
            PathBuf::from("/x/memo.md.tetel")
        );
    }

    #[test]
    fn a_missing_snapshot_directory_is_missing_not_a_match() {
        assert!(matches!(
            check(Path::new("/nonexistent/memo.md"), "whatever"),
            Provenance::Missing
        ));
    }
}
