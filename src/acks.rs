//! `tetel prose --ack` — a residue-scoped acknowledgement that a human
//! re-read a `prose-revised-since-proof` block's current text against
//! the claims it cites and found nothing to change.
//!
//! # Why a sibling log, not a `ProseEvent` variant
//!
//! `ProseEvent` is an internally-tagged serde enum, and
//! `workspace::read_jsonl` turns any single unparseable line into an
//! error over the *whole* log — there is no per-line skip. Adding an
//! `Ack` tag to that enum would mean every build that does not know the
//! new tag loses the ability to read `prose.jsonl` at all, including
//! every `Create`/`Revise` line it understood perfectly well. So the
//! event lives in its own append-only log, `acks.jsonl`, shipped as a
//! tenth [`crate::snapshot::SNAPSHOT_FILES`] entry. A build predating
//! that entry never opens the file and reproduces the un-suppressed
//! listing.
//!
//! # No withdrawal verb
//!
//! There is exactly one verb, mint. An author who acknowledged in error
//! and now believes the paragraph is faulty has an honest act already:
//! repair the paragraph, which voids the acknowledgement by changing its
//! key (see [`crate::checks::prose_after_proof`]). Declining a
//! withdrawal verb also keeps this log monotone within one workspace —
//! nothing here can remove or truncate it — which is what lets a stale
//! copy of it, carried forward by `snapshot::write`'s copy-only loop
//! across a render from a workspace that never held one, be a *prefix*
//! of the truth rather than an arbitrary earlier state.
//!
//! # The digest source, and why it is not `prose_after_proof`'s
//!
//! Every digest here is taken over a claim's proposition **as
//! `claims.jsonl` holds it** — never the ledger-derived proposition
//! `prose_after_proof`'s own in-proof test uses. Rendering a claim into
//! the evidence ledger (`compose::ledger_cell`) replaces embedded
//! newlines with spaces, and importing (`ledger::split_row_cells`) trims
//! every cell; the two strings are not the same one. Keying the ack on
//! the ledger-derived digest would make any claim with an embedded
//! newline or edge whitespace permanently unackable, with nothing red to
//! say so. Both the mint side (here) and the check side
//! (`checks::prose_after_proof`) read the same source — `claims.jsonl`,
//! whichever workspace or snapshot copy of it is in hand — so the two
//! digests are always computed the same way.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::claims;
use crate::evidence;
use crate::prose;
use crate::workspace::{self, AuthoringError};

/// One acknowledgement. Carries a timestamp because every workspace
/// event does (`workspace::now_unix()`), but no comparison in this
/// crate ever reads it against anything else — see
/// `checks::prose_after_proof`'s doc comment on why the sketch's
/// timestamp clause was dropped. Every other field is part of the
/// suppression key: a listing is discharged exactly when some ack's
/// `block`, `text`, `cite` and `digests` all equal what the check
/// recomputes, byte for byte, and `identity` equals the snapshot's own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckEvent {
    /// The id of the prose block this acknowledges.
    pub block: String,
    /// The block's text at the moment of acknowledgement.
    pub text: String,
    /// The block's citation list at the moment of acknowledgement.
    pub cite: Vec<String>,
    /// One sha256 digest per entry of `cite`, in the same order, each
    /// over that claim's proposition **as `claims.jsonl` holds it** —
    /// see the module doc comment on why this is deliberately not the
    /// ledger-derived digest.
    pub digests: Vec<String>,
    /// The identity of the workspace that minted this event
    /// (`workspace::identity`). Suppression additionally requires this
    /// to equal the identity the snapshot itself ships, because nothing
    /// binds a rendered memo to the workspace that produced it — without
    /// this, a snapshot copied from elsewhere could carry an
    /// acknowledgement no workspace on this machine ever made.
    pub identity: String,
    /// The author's own stated reason, carried verbatim — never a
    /// paraphrase, and never parsed by this tool.
    pub why: String,
    pub timestamp: u64,
}

fn log_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("acks.jsonl")
}

/// Every acknowledgement ever minted in this workspace, in append
/// order. A missing file is an empty log, never an error — see
/// [`workspace::read_jsonl`]. An unreadable-but-present file propagates
/// its error, which `check_file` turns into a machine failure (see
/// `Findings::acks_unreadable`): unlike `prose.jsonl`, `compose::render`
/// never reads this file, so nothing else has already reddened
/// provenance on its behalf.
pub fn load_all(workspace_dir: &Path) -> io::Result<Vec<AckEvent>> {
    workspace::read_jsonl(&log_path(workspace_dir))
}

/// Mint an acknowledgement for `block_id`, refusing on any of:
///
/// - the block does not exist (the one refusal this verb shares with
///   every other prose mode);
/// - `why` is empty (`--ack requires --why`, the identical test
///   `--revise` already backs);
/// - the block is a heading — no listing exists for one to discharge;
/// - the block cites nothing — same reason;
/// - a cited id does not resolve to a claim in `claims.jsonl` — the
///   ack's key needs a digest per citation, and an unresolvable one has
///   none.
///
/// Deliberately absent: any check that the block is *currently listed*.
/// An authoring workspace holds prose, claims, facts, targets and
/// transplants and no evidence records at all — grading records live
/// beside the rendered memo — so there is no anchor to compute here, and
/// none is needed: an ack on an unlisted block simply suppresses
/// nothing.
pub fn create(workspace_dir: &Path, block_id: &str, why: &str) -> Result<AckEvent, AuthoringError> {
    let blocks = prose::load_all(workspace_dir)?;
    let Some(block) = blocks.iter().find(|b| b.id == block_id) else {
        return Err(workspace::refuse(workspace_dir, "prose", format!("no such prose block: {block_id}")));
    };
    if why.trim().is_empty() {
        return Err(workspace::refuse(
            workspace_dir,
            "prose",
            "--ack requires --why (acknowledgements must explain themselves)",
        ));
    }
    if block.heading {
        return Err(workspace::refuse(
            workspace_dir,
            "prose",
            format!("cannot --ack {block_id}: it is a heading, and a heading is never listed by prose-revised-since-proof"),
        ));
    }
    if block.cite.is_empty() {
        return Err(workspace::refuse(
            workspace_dir,
            "prose",
            format!("cannot --ack {block_id}: it cites nothing, and an uncited block is never listed by prose-revised-since-proof"),
        ));
    }
    let claim_list = claims::load_all(workspace_dir)?;
    let mut digests = Vec::with_capacity(block.cite.len());
    for cid in &block.cite {
        let Some(claim) = claim_list.iter().find(|c| &c.id == cid) else {
            return Err(workspace::refuse(
                workspace_dir,
                "prose",
                format!("cannot --ack {block_id}: it cites {cid}, which is not a claim in this workspace; try `tetel query claims`"),
            ));
        };
        digests.push(evidence::sha256_hex(&claim.prop));
    }
    let identity = workspace::identity(workspace_dir)?.id;
    let event = AckEvent {
        block: block_id.to_string(),
        text: block.text.clone(),
        cite: block.cite.clone(),
        digests,
        identity,
        why: why.to_string(),
        timestamp: workspace::now_unix(),
    };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(event)
}
