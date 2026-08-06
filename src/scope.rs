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
//!
//! # Language-agnostic by construction
//!
//! Tetel documents systems in any language, so nothing here may be tuned
//! to one. There is no list of languages and no list of extensions.
//!
//! What counts as a filename is decided in two stages. Shape rules reject
//! what cannot be a file — prose abbreviations, version numbers, the
//! tails of method chains. Then the token's extension must be one this
//! workspace has **actually opened**, read out of the captured extents.
//! A `look` at a Swift file is what teaches the scanner that `.swift`
//! names a file here; nobody enumerates that in advance and no language
//! is excluded in advance.
//!
//! The second stage is load-bearing, not belt-and-braces. Attribute
//! access — `self.value`, `response.data` — is filename-shaped under any
//! shape rule worth having: a good stem, a good extension. Only "was
//! this extension ever opened" separates it from a real file, and it does
//! so without knowing which language is being discussed.
//!
//! Its cost is that the vocabulary is per-extension, not per-language: a
//! workspace that has opened `mix.exs` has not thereby learned `.ex`. The
//! alternative is a hard-coded map from extension to language, which is
//! exactly the coupling this avoids. Pinned by a test rather than left to
//! be rediscovered.

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

/// The extension of a path-shaped token, lowercased, or `None` if the
/// token does not look like a filename at all.
///
/// Deliberately not a fixed list of languages. Tetel documents systems in
/// any language, and a hard-coded set would silently stop checking the
/// moment someone wrote about Swift, Kotlin, Elixir or Zig — the worst
/// kind of gap, because it looks like a clean result.
///
/// The shape rules instead:
///   - a non-empty stem, rejecting the leading-dot fragments left by
///     method chains (`?.parents`, `).ok`);
///   - an extension of one to six ASCII letters, rejecting version
///     numbers (`v1.2`, digits) and the long or underscored tails of
///     field access (`p.tree_hash`, `parcel.parents`);
///   - not both parts a single character, which is what separates the
///     prose abbreviations `e.g` and `i.e` from real short filenames.
///
/// That last rule is why it is not simply "stem of two or more": `a.rs`
/// is a legitimate filename and `main.c` a legitimate one-letter
/// extension, so neither half can be excluded alone. Six characters
/// covers `.coffee` and `.python`; `parcel.parents` is seven and falls
/// out. Both boundaries are doing real work and are stated rather than
/// rounded off.
fn filename_extension(tok: &str) -> Option<String> {
    let (stem, ext) = tok.rsplit_once('.')?;
    let stem = stem.rsplit('/').next().unwrap_or(stem);
    if stem.is_empty() || ext.is_empty() || ext.len() > 6 {
        return None;
    }
    if stem.len() == 1 && ext.len() == 1 {
        return None;
    }
    if !ext.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// Every filename-shaped token in a note, in order of appearance.
///
/// Returns the token as written. A note saying `graph_utils.rs ~560-566`
/// yields `graph_utils.rs`; the line range is not parsed, because a note
/// naming the right file at the wrong lines is a question for a reader,
/// not a token this check can adjudicate.
///
/// `known_extensions` narrows the shape rules above to extensions this
/// workspace has actually opened — see [`outside_extent`], which derives
/// them from the captured extents. Pass an empty slice to accept every
/// filename-shaped token.
pub fn mentioned_paths(note: &str, known_extensions: &[String]) -> Vec<String> {
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
        let Some(ext) = filename_extension(tok) else {
            continue;
        };
        if !known_extensions.is_empty() && !known_extensions.contains(&ext) {
            continue;
        }
        if !out.iter().any(|s| s == tok) {
            out.push(tok.to_string());
        }
    }
    out
}

/// The file extensions this workspace has actually opened.
///
/// This is what keeps the check language-agnostic without drowning in
/// noise. A `look` at a Swift file teaches the scanner that `.swift`
/// names a file here; nobody has to enumerate languages in advance, and
/// no language is silently excluded. The vocabulary comes from the same
/// captured extents the check compares against — the machine-captured
/// side, never the authored one.
///
/// Its own limit, stated: a note naming a file type the workspace has
/// never opened is not checked. If every `look` was at Markdown and the
/// note concludes about `other.swift`, that passes. Rarer than the case
/// it buys, and preferable to a fixed list that fails the same way
/// permanently.
fn observed_extensions(facts: &[Fact]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in facts {
        for e in &f.extent {
            if let Some(ext) = filename_extension(&e.key) {
                if !out.contains(&ext) {
                    out.push(ext);
                }
            }
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

/// What to tell the author, at the moment of minting.
///
/// # Why the author gets this and not only the reviewer
///
/// The design assumes agents author and humans review. A finding that
/// reaches only `check` reaches only the reviewer — by which point the
/// note is written, the claim resting on it is written, and the memo is
/// rendered. The cheapest moment to fix an overreaching note is the
/// second after it is minted, while whoever wrote it still remembers
/// whether that location was context or a conclusion.
///
/// So this is deliberately phrased as a decision the author can act on
/// immediately, with both available corrections named. It is still not a
/// refusal: naming a location as context is legitimate, and refusing
/// would make the honest note unwritable in order to catch the dishonest
/// one.
pub fn advice(o: &OutsideExtent) -> String {
    format!(
        "note names {}, which this fact's extent does not cover (extent: {}). \
Fine if that's context. If it's a conclusion about {}, you have not read it here — \
`look` at it and mint a fact for it, or revise this note to drop the claim about it.",
        o.mentioned,
        o.extent_labels.join("; "),
        o.mentioned,
    )
}

/// The findings for one fact id in a workspace, for the authoring paths.
///
/// Loads from disk rather than taking a `Fact`, because `revise` returns
/// only an id — and a revised note can overreach exactly as a minted one
/// can. Checking mint but not revise would leave the obvious way to
/// introduce the defect unwatched.
///
/// Returns empty on a read error rather than failing the command: the
/// fact was already written, and a warning that cannot be computed must
/// not turn a successful mint into an error.
pub fn for_fact(workspace_dir: &std::path::Path, id: &str) -> Vec<OutsideExtent> {
    let Ok(facts) = crate::facts::load_all(workspace_dir) else {
        return Vec::new();
    };
    let Some(pos) = facts.iter().position(|f| f.id == id) else {
        return Vec::new();
    };
    // Graded alone, but against the whole workspace's vocabulary: the
    // first fact in a Swift codebase must not go unchecked merely
    // because it is the only one minted so far.
    outside_extent_in(&facts[pos..pos + 1], &facts)
}

/// Every location a fact's note names that its own extent does not
/// cover, checking `subjects` against a vocabulary drawn from `corpus`.
///
/// The split exists because the two authoring paths need different
/// halves: `check` grades every fact in a workspace, while `fact`
/// grades the one just written — but both must read the same extension
/// vocabulary, which only the whole workspace can supply.
fn outside_extent_in(subjects: &[Fact], corpus: &[Fact]) -> Vec<OutsideExtent> {
    let known = observed_extensions(corpus);
    let mut out = Vec::new();
    for f in subjects {
        // A fact minted purely from `run` has a command as its extent,
        // not a path. Its note routinely names the files the command
        // touched, and calling those out-of-extent would fire on every
        // measurement fact while telling a reader nothing they cannot
        // see. `check` already routes every `proc:` extent to human-owed
        // on its own.
        if f.extent.iter().all(|e| e.label.starts_with("proc:")) {
            continue;
        }
        for m in mentioned_paths(&f.note, &known) {
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

/// Every location a fact's note names that its own extent does not cover.
pub fn outside_extent(facts: &[Fact]) -> Vec<OutsideExtent> {
    outside_extent_in(facts, facts)
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
        let found = mentioned_paths("see audit_utils.rs and graph_utils.rs, e.g. v1.2 or i.e. this", &[]);
        assert_eq!(found, vec!["audit_utils.rs", "graph_utils.rs"]);
    }

    #[test]
    fn a_path_is_reported_once_however_often_it_appears() {
        assert_eq!(mentioned_paths("a.rs then a.rs again a.rs", &[]), vec!["a.rs"]);
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

    /// The check must work in any language, not the one tetel happens to
    /// be written in. Each of these is a fact whose extent is one file
    /// and whose note concludes about a second, unopened one — the F6
    /// shape, transposed. None of these extensions is enumerated
    /// anywhere in this module.
    #[test]
    fn the_check_is_not_specific_to_any_one_language() {
        let cases = [
            ("Model.swift", "ViewController.swift"),
            ("main.kt", "Repository.kt"),
            ("core.clj", "handlers.clj"),
            ("Service.cs", "Repo.cs"),
            ("router.ex", "plug.ex"),
            ("build.zig", "parser.zig"),
            ("app.dart", "store.dart"),
            ("Main.hs", "Parser.hs"),
            ("query.sql", "schema.sql"),
        ];
        for (opened, concluded_about) in cases {
            let f = fact(
                "F1",
                &format!("{opened} defines the type; {concluded_about} never uses it"),
                &[(&format!("/repo/{opened}"), opened)],
            );
            let found = outside_extent(&[f]);
            assert_eq!(
                found.len(),
                1,
                "should fire for {opened} -> {concluded_about}, got {:?}",
                found.iter().map(|o| &o.mentioned).collect::<Vec<_>>()
            );
            assert_eq!(found[0].mentioned, concluded_about);
        }
    }

    /// Prose abbreviations and code punctuation must not be mistaken for
    /// filenames, in any language.
    #[test]
    fn prose_and_code_punctuation_are_not_filenames() {
        // A realistic vocabulary: this workspace has opened Rust files.
        let known = vec!["rs".to_string()];
        for note in [
            "e.g. this and i.e. that",
            "version v1.2 and 3.14",
            "reads p.tree_hash from the parcel",
            "calls load_parcel(hash)?.parents directly",
            // Attribute access is filename-shaped by every rule above —
            // `self` is a fine stem and `value` a fine extension. Only
            // the observed-extension vocabulary separates them, which is
            // why that filter exists rather than shape rules alone.
            "self.value and obj.field",
            "config.settings and response.data",
        ] {
            assert!(
                mentioned_paths(note, &known).is_empty(),
                "should find no filename in: {note}"
            );
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

    /// The vocabulary is per-extension, not per-language, so a language
    /// using two extensions only checks the ones actually opened. Found
    /// while writing the cross-language test above, and kept: it is a
    /// consequence of learning the vocabulary from captured extents
    /// rather than enumerating languages, and the alternative — a
    /// hard-coded map of which extensions belong to which language — is
    /// the language-specific coupling this design exists to avoid.
    #[test]
    fn a_second_extension_of_the_same_language_is_not_inferred() {
        let f = fact(
            "F1",
            "mix.exs configures the project; router.ex never imports it",
            &[("/repo/mix.exs", "mix.exs")],
        );
        assert!(
            outside_extent(&[f]).is_empty(),
            "if this now fires, the vocabulary learned to generalise — update the module doc"
        );
    }

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
