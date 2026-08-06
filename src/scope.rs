//! Does a fact's note talk about somewhere it never looked?
//!
//! # The check this recovers
//!
//! The project's flagship check was once `Domain ⊆ Extent` — compare what
//! a claim says it ranges over against what its evidence examined. It was
//! abandoned as vacuous: when one author supplies both sides, they come
//! out byte-identical ~80% of the time, no better than hand-authored
//! rows. Independence, not capture, was what moved that number.
//!
//! This is the same comparison in a form where independence is
//! structural rather than procedural. A fact's **extent is captured by
//! the tool** — `look` and `run` record what they actually opened, and
//! there is deliberately no flag to type one in. A fact's **note is
//! authored**. Those are two different acts by construction, so comparing
//! the locations a note *names* against the extent that was *captured*
//! cannot be made vacuous by one author writing both sides.
//!
//! # The failure it was written for
//!
//! From the first real memo authored through this tool, verbatim:
//!
//! ```text
//! F6  extent: audit_utils.rs 785-817
//!     note:  "...graph_utils::parents (graph_utils.rs ~560-566) reads a
//!             stored commit-graph record's parents when present, else
//!             falls back to object_utils::load_parcel(hash)?.parents
//!             directly -- the same parents field ... (no separate,
//!             potentially-diverging source)."
//! ```
//!
//! The note names two branches and concludes there is one source, about a
//! file the fact never opened. A second fact overreached the same way,
//! the two contradicted each other, and a claim leaned on both by name.
//! Nothing in the tool noticed, because nothing compared a note against
//! its own extent.
//!
//! # What this is not
//!
//! It is a **scope** check, never a truth check. Whether the note is
//! *correct* about the location it names is exactly the limit this
//! project registered as permanently out of reach ("insincere
//! examination" — capture makes the extent honest, not the
//! interpretation). Nothing here touches that.
//!
//! It is also **human-owed, never a refusal**. A note may legitimately
//! name a location as context, contrast, or a pointer to what to read
//! next, and only a person can tell that from an unsupported inference.
//! Refusing would make the honest case unwritable to catch the dishonest
//! one.
//!
//! # What it catches, measured, and what it does not
//!
//! Of the two overreaching facts in the memo that motivated it, this
//! catches **one**. F6 named a file and a line range (`graph_utils.rs
//! ~560-566`) it never opened, which is the strong tell and fires here.
//! F7's note asserted the client "mirrors the server's" walk while its
//! extent covered only the client — an overreach that names no location
//! at all, and so is invisible to any check of this shape.
//!
//! One refinement was tried and rejected on the evidence: flagging a
//! `module::symbol` reference whose module never appears in the fact's
//! own captured output. Measured against the same three facts, it would
//! have flagged a legitimate quotation (F3 citing `object_utils::
//! load_parcel`, a call site inside the code it did read) while missing
//! both real overreaches, because `graph_utils` and `merge_utils` do
//! appear in the read source as call sites. Naming a symbol the code you
//! read calls is normal; concluding about what that symbol *does* is the
//! failure — and those two are not distinguishable by string matching.
//!
//! So: a file-and-line reference to unopened code is caught, and a
//! conclusion drawn without naming anything is not. That is a real
//! ceiling, not a to-do.

use crate::facts::Fact;

/// A location a note names that its own extent does not cover.
pub struct OutsideExtent {
    pub fact_id: String,
    /// The location token as it appears in the note, verbatim — so a
    /// reader can find it by searching rather than by guessing what was
    /// normalised.
    pub mentioned: String,
    /// What the fact actually examined, for the contrast.
    pub extent_labels: Vec<String>,
}

/// True for a byte that can appear inside a source-file path segment.
fn is_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.' || b == b'/'
}

/// File extensions worth scanning for. Deliberately a fixed list rather
/// than "any dotted token": prose is full of `e.g.` and version numbers,
/// and a scanner that fired on those would be turned off within a day.
///
/// This is a coverage/noise trade made explicit rather than silently — a
/// note naming a file in a language not listed here will not be checked,
/// and that is a known gap, not an oversight.
const SOURCE_EXTENSIONS: [&str; 14] = [
    ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".java", ".rb", ".c", ".h", ".cpp", ".sh",
    ".toml",
];

/// Every file-path-shaped token in a note, in order of appearance.
///
/// Returns the token as written. A note saying `graph_utils.rs ~560-566`
/// yields `graph_utils.rs`; the line range is not parsed, because a note
/// naming the right file at the wrong lines is a question for a reader,
/// not a token this check can adjudicate.
pub fn mentioned_paths(note: &str) -> Vec<String> {
    let bytes = note.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if !is_path_byte(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_path_byte(bytes[i]) {
            i += 1;
        }
        // Trim trailing sentence punctuation: `audit_utils.rs.` and
        // `audit_utils.rs` are the same file.
        let tok = note[start..i].trim_end_matches('.');
        if SOURCE_EXTENSIONS.iter().any(|e| tok.ends_with(e)) && !out.iter().any(|s| s == tok) {
            out.push(tok.to_string());
        }
    }
    out
}

/// Whether `mentioned` is covered by an extent entry.
///
/// Compared by path suffix in both directions: an extent key is an
/// absolute path (`/Users/.../util/audit_utils.rs`) while a note names
/// whatever the author typed (`audit_utils.rs`, or
/// `crates/forklift-core/src/util/audit_utils.rs`). Matching on either
/// containing the other keeps both spellings resolving to the same file
/// without inventing a path-resolution step that could differ from what
/// `look` recorded.
fn extent_covers(fact: &Fact, mentioned: &str) -> bool {
    fact.extent.iter().any(|e| {
        let hay = format!("{} {}", e.key, e.label);
        hay.contains(mentioned) || mentioned.contains(e.key.as_str())
    })
}

/// Every location a fact's note names that its own extent does not cover.
pub fn outside_extent(facts: &[Fact]) -> Vec<OutsideExtent> {
    let mut out = Vec::new();
    for f in facts {
        // A fact minted purely from `run` has a command as its extent,
        // not a path. Its note routinely names the files the command
        // touched, and calling those out-of-extent would fire on every
        // measurement fact while telling a reader nothing they cannot
        // see. `check` already routes every `proc:` extent to human-owed
        // on its own.
        if f.extent.iter().all(|e| e.label.starts_with("proc:")) {
            continue;
        }
        for m in mentioned_paths(&f.note) {
            if !extent_covers(f, &m) {
                out.push(OutsideExtent {
                    fact_id: f.id.clone(),
                    mentioned: m,
                    extent_labels: f.extent.iter().map(|e| e.label.clone()).collect(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests_support {
    use crate::facts::{ExtentEntry, Fact};

    pub(crate) fn fact(id: &str, note: &str, extent: &[(&str, &str)]) -> Fact {
        Fact {
            id: id.to_string(),
            note: note.to_string(),
            extent: extent
                .iter()
                .map(|(k, l)| ExtentEntry {
                    key: k.to_string(),
                    label: l.to_string(),
                    world_state: String::new(),
                })
                .collect(),
            output: String::new(),
            pin: String::new(),
            revisions: 0,
        }
    }

}

#[cfg(test)]
mod tests {
    use super::tests_support::fact;
    use super::*;

    #[test]
    fn finds_source_paths_and_ignores_prose_dots() {
        let found = mentioned_paths("see audit_utils.rs and graph_utils.rs, e.g. v1.2 or i.e. this");
        assert_eq!(found, vec!["audit_utils.rs", "graph_utils.rs"]);
    }

    #[test]
    fn a_path_is_reported_once_however_often_it_appears() {
        assert_eq!(mentioned_paths("a.rs then a.rs again a.rs"), vec!["a.rs"]);
    }

    /// The regression this module was written for, reduced to its shape:
    /// a note concluding about a file its extent never opened.
    #[test]
    fn a_note_concluding_about_an_unopened_file_is_reported() {
        let f = fact(
            "F6",
            "new_parcels walks the graph. graph_utils::parents (graph_utils.rs ~560-566) reads \
the same parents field, so there is no separate source.",
            &[("/repo/crates/util/audit_utils.rs", "crates/util/audit_utils.rs lines 785-817")],
        );
        let found = outside_extent(&[f]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fact_id, "F6");
        assert_eq!(found[0].mentioned, "graph_utils.rs");
    }

    #[test]
    fn a_note_naming_only_its_own_extent_is_silent() {
        let f = fact(
            "F3",
            "verify_parcel_closure_with (audit_utils.rs ~1399-1442) computes base_root once.",
            &[("/repo/crates/util/audit_utils.rs", "crates/util/audit_utils.rs lines 1399-1442")],
        );
        assert!(outside_extent(&[f]).is_empty());
    }

    /// The author's spelling should not decide the outcome: a note naming
    /// the full repo-relative path and one naming the bare filename both
    /// resolve to the extent that was captured as an absolute path.
    #[test]
    fn either_spelling_of_a_path_resolves_to_the_same_extent() {
        for note in [
            "read crates/util/audit_utils.rs closely",
            "read audit_utils.rs closely",
        ] {
            let f = fact(
                "F1",
                note,
                &[("/repo/crates/util/audit_utils.rs", "crates/util/audit_utils.rs lines 1-10")],
            );
            assert!(outside_extent(&[f]).is_empty(), "failed for note: {note}");
        }
    }

    /// A `run`-only fact's note names whatever the command touched; that
    /// is not overreach, and firing on it would bury the real signal.
    #[test]
    fn a_command_only_fact_is_skipped() {
        let f = fact(
            "F10",
            "instrumented object_utils.rs and remote_utils.rs with a counter",
            &[("bash measure/repro.sh", "proc: bash measure/repro.sh (exit 0)")],
        );
        assert!(outside_extent(&[f]).is_empty());
    }
}

#[cfg(test)]
mod known_limits {
    use super::tests_support::fact;
    use super::*;

    /// Documents the ceiling rather than hiding it: F7's real note, whose
    /// overreach ("mirrors the server's") names no location, so nothing
    /// of this shape can see it. If a later design catches this, delete
    /// the test — do not let it quietly start passing unremarked.
    #[test]
    fn an_overreach_that_names_no_location_is_not_caught() {
        let f = fact(
            "F7",
            "The client's own new-parcel selection in lift_pallet_inner (remote_utils.rs \
~4355-4377) mirrors the server's: it walks back from local_head over ALL of parcel.parents. So \
the client, independently, arrives at the same partition the server's new_parcels computes.",
            &[(
                "/repo/crates/forklift-core/src/util/remote_utils.rs",
                "crates/forklift-core/src/util/remote_utils.rs lines 4355-4380",
            )],
        );
        assert!(
            outside_extent(&[f]).is_empty(),
            "if this now fires, the check improved — update the module doc's measured claim"
        );
    }
}
