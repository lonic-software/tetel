//! `tetel review` — every prose block beside the claims it cites, for
//! reading.
//!
//! # Where this came from
//!
//! S12 arm F's only substantive revision came from doing this by hand.
//! The author's own account: the muddle was caught *"rereading the
//! rendered draft against the claims it was supposed to be restating
//! (C3, C4), not by rereading the prose in isolation… This is the one
//! clear case in this session where the tool changed **what** I said, not
//! just how I typed it: free-form, I'd likely have written the same
//! muddled sentence and not noticed, because there'd have been nothing
//! external to check it against."*
//!
//! A review channel nobody designed, found by accident. What made it work
//! was having something external to read the prose against — and what
//! made it expensive was assembling that pairing by hand, flipping
//! between a paragraph and a ledger row several sections away.
//!
//! # Why this presents rather than scores
//!
//! The obvious automation is to measure prose against its claims —
//! overlapping terms, a similarity number, a threshold. That was tried
//! against the fork94 workspace before this module was written, and the
//! measurement killed it: content words appearing in a prose block but in
//! none of its cited claims ran at **43%, 61%, 75% and 90%** across the
//! four blocks measured. There is no threshold in that range that
//! separates a block restating its claims from one wandering off, because
//! prose legitimately carries framing, transitions and detail no
//! proposition contains. A score there would be noise wearing a number.
//!
//! It would also be the exact thing this project refuses elsewhere: no
//! coverage percentage, no aggregate, nothing to write prose to inflate.
//!
//! So this assembles and presents. The judgment stays with the reader,
//! which is where the one recorded success of this technique put it.
//!
//! # The failure it is aimed at
//!
//! From the FORK-94 review: a paragraph opened *"an aligned
//! per-immediate-parent prune is never worse … it is a strict
//! improvement"* and cited `C6`, whose proposition is entirely about
//! tolerant-fallback discipline and function scoping. The memo's single
//! load-bearing reason to proceed was prose riding under a citation to a
//! different sentence. Nothing mechanical here detects that — but a
//! reader seeing the two a line apart cannot miss it, and today they are
//! forty lines apart in a rendered document.

use std::io;
use std::path::Path;

use crate::claims;
use crate::prose;

/// One prose block with the propositions it cites, resolved.
pub struct Pairing {
    pub block_id: String,
    pub text: String,
    /// `(claim id, proposition)` for each cited claim, in cited order.
    pub cited: Vec<(String, String)>,
    /// Ids cited by the block that no claim defines. Not a failure here —
    /// `check` owns that verdict — but shown so a pairing is never
    /// silently short a row the reader was told to expect.
    pub unresolved: Vec<String>,
}

/// Every non-heading prose block, paired with the claims it cites.
///
/// Headings are skipped: a heading citing nothing is ordinary structure,
/// and listing them would pad the output with items no reader needs to
/// weigh.
///
/// Blocks citing nothing are **kept**, because that is the shape worth
/// looking at hardest — a paragraph asserting something with no claim
/// behind it at all. They are separated in [`render`] rather than
/// dropped.
pub fn pairings(workspace_dir: &Path) -> io::Result<Vec<Pairing>> {
    let blocks = prose::load_all(workspace_dir)?;
    let claim_list = claims::load_all(workspace_dir)?;

    Ok(blocks
        .into_iter()
        .filter(|b| !b.heading)
        .map(|b| {
            let mut cited = Vec::new();
            let mut unresolved = Vec::new();
            for id in &b.cite {
                match claim_list.iter().find(|c| &c.id == id) {
                    Some(c) => cited.push((c.id.clone(), c.prop.clone())),
                    // A fact id cited from prose lands here: legitimate,
                    // and not a claim this pairing can show. Named rather
                    // than dropped so the reader knows why the list is
                    // shorter than the block's own `*cites:*` line.
                    None => unresolved.push(id.clone()),
                }
            }
            Pairing { block_id: b.id, text: b.text, cited, unresolved }
        })
        .collect())
}

/// The review, as text: each block, then the propositions it cites.
///
/// Uncited blocks are collected into their own closing section rather
/// than interleaved, so the reader meets them as a group and can weigh
/// "is there a claim missing here" once, with all of them in view.
pub fn render(workspace_dir: &Path) -> io::Result<String> {
    let pairings = pairings(workspace_dir)?;
    if pairings.is_empty() {
        return Ok("no prose to review — write some with `tetel prose` first.\n".to_string());
    }

    let mut out = String::new();
    out.push_str(
        "Each paragraph, then the propositions it cites. Read them against each other: does the \
paragraph say what its claims say, no more? A paragraph asserting something none of its claims \
carries is the failure this is for — nothing below detects it, and a reader seeing the two \
together will.\n",
    );

    let mut uncited: Vec<&Pairing> = Vec::new();
    for p in &pairings {
        if p.cited.is_empty() && p.unresolved.is_empty() {
            uncited.push(p);
            continue;
        }
        out.push_str(&format!("\n--- {} ---\n{}\n", p.block_id, p.text));
        for (id, prop) in &p.cited {
            out.push_str(&format!("\n  cites {id}: {prop}\n"));
        }
        for id in &p.unresolved {
            out.push_str(&format!(
                "\n  cites {id}: not a claim in this workspace — a fact id cites evidence \
directly; `tetel query facts` shows what it names\n"
            ));
        }
    }

    if !uncited.is_empty() {
        out.push_str(
            "\n=== paragraphs citing nothing ===\nNothing in the workspace stands behind these. \
Some are legitimately framing or transition; any that assert something are the ones to look at.\n",
        );
        for p in &uncited {
            out.push_str(&format!("\n--- {} ---\n{}\n", p.block_id, p.text));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_workspace_says_so_rather_than_printing_nothing() {
        let dir = std::env::temp_dir().join(format!(
            "tetel-review-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let out = render(&dir).unwrap();
        assert!(out.contains("no prose to review"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
