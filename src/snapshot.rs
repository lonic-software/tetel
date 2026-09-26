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
//!
//! # Which input moved (TET-43)
//!
//! The comparison has three inputs, not two: the document, the snapshot,
//! and the renderer doing the re-render. A change to what `render` emits
//! fails every committed memo exactly as a hand edit does, and a bare
//! byte comparison cannot say which of the three moved.
//!
//! So `render --out` also writes a render record, [`RECORD_FILE`], into
//! the snapshot: a digest of the document it wrote, one digest per
//! shipped snapshot file, and the label of the build that rendered them.
//! When a re-render differs, the record names the input that moved —
//! [`Provenance::SnapshotEdited`], [`Provenance::DocumentEdited`], or,
//! when neither did, [`Provenance::RendererChanged`]. A memo with no
//! record stays [`Provenance::Unattributed`].
//!
//! Every one of those is still a machine failure, `RendererChanged`
//! included: `check` parses the ledger, the target rows and the
//! transplant ids out of the document in the layout the current build
//! renders, so it cannot vouch for its own verdicts on a document in an
//! older one. What changes is that the failure is diagnosed exactly, and
//! that `tetel rerender` migrates a memo from its snapshot alone, without
//! the authoring workspace — see [`rerender`].
//!
//! The record guards against accidents, not adversaries: anyone who can
//! edit the snapshot and re-render could already forge provenance.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The render record's file name inside `<memo>.tetel/`.
///
/// Deliberately not in [`SNAPSHOT_FILES`]: those are copied from the
/// workspace, and this is written by the publication itself, about the
/// pair it has just produced. A workspace never has one.
pub const RECORD_FILE: &str = "render.json";

/// The files a snapshot carries. Enumerated rather than copied
/// wholesale so that a future workspace file is a deliberate decision to
/// ship or withhold, not an accident of `cp -r`.
///
/// `pending.json` is included even though `render` never reads it: a
/// non-empty buffer at snapshot time is a fact about how the document was
/// finished, and silently dropping it would hide it.
const SNAPSHOT_FILES: [&str; 10] = [
    "facts.jsonl",
    "claims.jsonl",
    "prose.jsonl",
    // Shipped, not withheld: without it `check` can see the rendered
    // target rows but nothing to verify them against, and the census
    // refusal would hold only inside the workspace that authored the
    // memo — exactly the reviewer-of-a-committed-document case the
    // snapshot exists for.
    "targets.jsonl",
    // Same reason, one step further: a premise is a quotation, and
    // re-verifying a quotation needs both the selection and the captured
    // bytes it claims to come from. `facts.jsonl` above carries the
    // second half, this the first — without it a reader can see the
    // donor's words on the page and has no way to tell they are the
    // donor's.
    "transplants.jsonl",
    "counters.json",
    "pending.json",
    "refusals.log",
    // Without this, `check` cannot tell who authored the memo, and so
    // cannot tell an author grounding their own claims from an
    // independent pass grounding them. That is the distinction the whole
    // mechanism exists for — 78% scope-equal self-grounded against 33%
    // independent — and it was invisible until this file shipped.
    "identity.json",
    // TET-61: `tetel prose --ack` discharges a `prose-revised-since-proof`
    // listing, and the discharge is a record rather than a rewrite —
    // see `acks.rs`'s module doc comment. Enumerated here, like every
    // other entry, so a build predating this line never opens the file
    // and reproduces the un-suppressed listing rather than silently
    // reading nothing.
    "acks.jsonl",
];

/// A memo's snapshot directory: `<memo>.tetel`, sitting next to it —
/// the same convention as [`crate::evidence::evidence_path`].
pub fn snapshot_path(memo: &Path) -> PathBuf {
    let mut s = memo.as_os_str().to_os_string();
    s.push(".tetel");
    PathBuf::from(s)
}

/// Publish a rendered memo: the document, its snapshot, and the render
/// record certifying the pair. `rendered` is what the caller's own
/// `compose::render` of `workspace_dir` produced.
///
/// # Why one function does all of it, in this order
///
/// Both `render --out` callers used to write the document and then call a
/// copy-only version of this. Folding the whole publication in here means
/// neither caller can forget a step or misorder one:
///
/// 1. **Remove any existing record**, before the identity mint or
///    anything else that can fail. An interruption at any later step then
///    leaves no record, which reads as `Unattributed` (or an unrecorded
///    match, if nothing had changed yet) and never as an edit against a
///    record of an older pair.
/// 2. **Write the document.**
/// 3. **Mirror the workspace**: copy each [`SNAPSHOT_FILES`] name the
///    workspace has and remove each one it lacks. The copy-only loop this
///    replaced carried a file forward from an earlier workspace, which
///    would make a correct render fail its own certification below.
/// 4. **Write the record, only if the snapshot re-renders `rendered`
///    exactly.** Rendering and copying are two separate reads of the
///    workspace; without this check the record could certify a pair that
///    never matched. If it fails, the error names the first differing
///    line and no record is left.
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
pub fn write(memo: &Path, workspace_dir: &Path, rendered: &str) -> io::Result<()> {
    let dir = snapshot_path(memo);
    remove_record(&dir)?;
    crate::workspace::identity(workspace_dir)?;
    fs::write(memo, rendered)?;
    fs::create_dir_all(&dir)?;
    for name in SNAPSHOT_FILES {
        let src = workspace_dir.join(name);
        let dst = dir.join(name);
        if src.exists() {
            fs::copy(&src, &dst)?;
        } else {
            match fs::remove_file(&dst) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
    }
    let reproduced = crate::compose::render(&dir)?;
    if reproduced != rendered {
        let diff = Diff::between(&reproduced, rendered);
        return Err(io::Error::other(format!(
            "the snapshot does not re-render the document it was published with ({}), so no \
render record was written — the workspace probably changed during the render; render again",
            diff.describe()
        )));
    }
    write_record(&dir, rendered)
}

/// The render record: what `render --out` (or `tetel rerender`) wrote,
/// and which build wrote it.
///
/// Digests are kept per file, not as one digest over the list, because
/// [`SNAPSHOT_FILES`] has grown over time: a single digest over the
/// current list would change the first time a name was added, and every
/// existing snapshot would then read as edited.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RenderRecord {
    /// sha256 of the document's bytes.
    pub document: String,
    /// Each snapshot file present at publication, by name, to its sha256.
    pub files: BTreeMap<String, String>,
    /// [`crate::buildid::label`] of the build that rendered the pair.
    pub build: String,
}

fn sha256_bytes(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Each [`SNAPSHOT_FILES`] name present in `dir`, to its sha256.
fn file_digests(dir: &Path) -> io::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for name in SNAPSHOT_FILES {
        match fs::read(dir.join(name)) {
            Ok(bytes) => {
                out.insert(name.to_string(), sha256_bytes(&bytes));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

fn remove_record(dir: &Path) -> io::Result<()> {
    match fs::remove_file(dir.join(RECORD_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Write a record for `document` and the snapshot as it stands now.
/// Written to a temporary name and renamed, so an interruption leaves no
/// record rather than a truncated one.
fn write_record(dir: &Path, document: &str) -> io::Result<()> {
    let record = RenderRecord {
        document: sha256_bytes(document.as_bytes()),
        files: file_digests(dir)?,
        build: crate::buildid::label(),
    };
    let mut json = serde_json::to_string_pretty(&record).map_err(io::Error::other)?;
    json.push('\n');
    let tmp = dir.join(format!("{RECORD_FILE}.tmp"));
    fs::write(&tmp, json)?;
    fs::rename(&tmp, dir.join(RECORD_FILE))
}

/// The record beside a snapshot: `Ok(None)` when there is none, an error
/// when one exists but cannot be read — never silently treated as absent.
pub fn read_record(dir: &Path) -> Result<Option<RenderRecord>, String> {
    match fs::read_to_string(dir.join(RECORD_FILE)) {
        Ok(raw) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(|e| format!("{RECORD_FILE} is malformed ({e})")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{RECORD_FILE} could not be read ({e})")),
    }
}

/// The snapshot files that differ from `record`, sorted by name: a
/// recorded file whose digest differs, a recorded file that is missing,
/// and a [`SNAPSHOT_FILES`] name present but absent from the record.
///
/// A missing file has to count. `workspace::read_jsonl` reads a missing
/// file as empty, so a snapshot that lost `claims.jsonl` still renders —
/// and without this it would pass for a renderer change.
fn snapshot_edits(dir: &Path, record: &RenderRecord) -> Vec<String> {
    let mut edited = Vec::new();
    for (name, digest) in &record.files {
        // A record names files by bare name; anything else was not written
        // by this module, and is reported rather than followed.
        if name.contains(['/', '\\']) || name == ".." {
            edited.push(name.clone());
            continue;
        }
        match fs::read(dir.join(name)) {
            Ok(bytes) if &sha256_bytes(&bytes) == digest => {}
            _ => edited.push(name.clone()),
        }
    }
    for name in SNAPSHOT_FILES {
        if !record.files.contains_key(name) && dir.join(name).exists() {
            edited.push(name.to_string());
        }
    }
    edited.sort();
    edited
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

/// Where a re-render and the document first part, for a reader to look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    /// 1-based line of the first difference. `None` when the two agree
    /// line-for-line but differ in length.
    pub first_diff_line: Option<usize>,
    pub snapshot_lines: usize,
    pub memo_lines: usize,
}

impl Diff {
    fn between(rendered: &str, memo_source: &str) -> Diff {
        Diff {
            first_diff_line: rendered
                .lines()
                .zip(memo_source.lines())
                .position(|(a, b)| a != b)
                .map(|i| i + 1),
            snapshot_lines: rendered.lines().count(),
            memo_lines: memo_source.lines().count(),
        }
    }

    pub fn describe(&self) -> String {
        let where_ = match self.first_diff_line {
            Some(n) => format!("first difference at line {n}"),
            None => "identical line-for-line but different lengths".to_string(),
        };
        format!(
            "{where_}; snapshot {} lines, document {}",
            self.snapshot_lines, self.memo_lines
        )
    }
}

/// What a memo's provenance looks like on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// No `<memo>.tetel/` beside the memo. Only reportable when the memo
    /// carries citations — a document with no citations owes no record.
    Missing,
    /// The snapshot re-renders the memo exactly, and any record beside it
    /// is a record of this document.
    Matches,
    /// The snapshot re-renders the memo exactly, but the record beside it
    /// describes a different document. The pair was rewritten by a build
    /// that writes no record (every build before TET-43): it neither
    /// writes one nor removes an old one. Reported now, while the renderer
    /// has not moved, rather than surfacing at the next renderer change as
    /// an edit `rerender` refuses.
    StaleRecord { recorded_build: String },
    /// The re-render differs and there is no record to say why: a hand
    /// edit, a workspace that moved on without a re-render, or a renderer
    /// change are all possible.
    Unattributed(Diff),
    /// The re-render differs, and some snapshot file differs from the
    /// record (named, sorted).
    SnapshotEdited { files: Vec<String>, diff: Diff },
    /// The re-render differs, and the document differs from the record.
    DocumentEdited(Diff),
    /// The re-render differs, but neither the document nor any snapshot
    /// file differs from the record: the renderer moved.
    RendererChanged { recorded_build: String, diff: Diff },
    /// A snapshot exists but could not be rendered from, or its record
    /// could not be read. Never silently treated as "matches".
    Unreadable(String),
}

impl Provenance {
    /// Whether this outcome fails `check`. Every outcome but `Missing` and
    /// `Matches` does — `RendererChanged` included; see the module doc
    /// comment.
    pub fn failed(&self) -> bool {
        !matches!(self, Provenance::Missing | Provenance::Matches)
    }
}

/// Compare a memo's committed bytes against what its snapshot renders,
/// and when they differ, attribute the difference using the render
/// record. See the module doc comment.
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
    let record = match read_record(&dir) {
        Ok(r) => r,
        Err(e) => return Provenance::Unreadable(e),
    };
    classify(&dir, memo_source, &rendered, record.as_ref())
}

fn classify(
    dir: &Path,
    memo_source: &str,
    rendered: &str,
    record: Option<&RenderRecord>,
) -> Provenance {
    let document_recorded =
        |r: &RenderRecord| r.document == sha256_bytes(memo_source.as_bytes());
    if rendered == memo_source {
        return match record {
            Some(r) if !document_recorded(r) => {
                Provenance::StaleRecord { recorded_build: r.build.clone() }
            }
            _ => Provenance::Matches,
        };
    }
    let diff = Diff::between(rendered, memo_source);
    let Some(record) = record else {
        return Provenance::Unattributed(diff);
    };
    let files = snapshot_edits(dir, record);
    if !files.is_empty() {
        return Provenance::SnapshotEdited { files, diff };
    }
    if !document_recorded(record) {
        return Provenance::DocumentEdited(diff);
    }
    Provenance::RendererChanged { recorded_build: record.build.clone(), diff }
}

/// What [`rerender`] did to one memo.
#[derive(Debug, PartialEq, Eq)]
pub enum Rerendered {
    /// Clean and sealed already: not a byte written.
    Unchanged,
    /// The document already matched and had no record: a record was
    /// written, the document untouched.
    Sealed,
    /// A stale record (see [`Provenance::StaleRecord`]) was replaced.
    StaleRecordReplaced { recorded_build: String },
    /// The document was rewritten as its snapshot's current render, and
    /// sealed. `vouched` is true when the operator vouched for an
    /// unattributed memo.
    Migrated { recorded_build: Option<String>, vouched: bool },
}

/// Re-render a memo from its own `<memo>.tetel` directory — never from a
/// workspace, which on a fresh clone does not exist — and seal it.
///
/// Every record this writes is for a document it has just made equal to
/// the render of the snapshot. It refuses rather than overwrite anything
/// that differs from its record, and refuses any rewrite that would change
/// which claims the ledger holds or what any of them says: grading records
/// are keyed to digests of the ledger's propositions as `check` reads them
/// back, so such a rewrite would silently put graded claims out of proof.
///
/// `unattributed` is the operator vouching for a memo with no record. It
/// still writes the render of the snapshot, never the bytes as they stand.
pub fn rerender(memo: &Path, unattributed: bool) -> Result<Rerendered, String> {
    let dir = snapshot_path(memo);
    if !dir.is_dir() {
        return Err(format!(
            "no snapshot beside it ({} is not a directory) — there is nothing to re-render from",
            dir.display()
        ));
    }
    let source =
        fs::read_to_string(memo).map_err(|e| format!("could not read it ({e})"))?;
    let rendered = crate::compose::render(&dir)
        .map_err(|e| format!("its snapshot could not be rendered from ({e})"))?;
    let record = read_record(&dir)?;
    let io_err = |e: io::Error| format!("could not write ({e})");

    match classify(&dir, &source, &rendered, record.as_ref()) {
        Provenance::Matches => match record {
            None => {
                write_record(&dir, &source).map_err(io_err)?;
                Ok(Rerendered::Sealed)
            }
            Some(r) => {
                // `check` compares only the document digest under a match.
                // Here every file is compared: an edit render does not
                // reflect (identity.json, acks.jsonl, a fact field render
                // never prints) still matches, and resealing would erase
                // the only trace of it.
                let files = snapshot_edits(&dir, &r);
                if files.is_empty() {
                    Ok(Rerendered::Unchanged)
                } else {
                    Err(format!(
                        "it re-renders exactly, but these snapshot files differ from its render \
record: {}. An edit that render does not reflect is still an edit, and resealing would erase \
the only trace of it — restore the files, or re-render from the workspace that wrote them",
                        files.join(", ")
                    ))
                }
            }
        },
        Provenance::StaleRecord { recorded_build } => {
            write_record(&dir, &source).map_err(io_err)?;
            Ok(Rerendered::StaleRecordReplaced { recorded_build })
        }
        Provenance::RendererChanged { recorded_build, .. } => {
            migrate(memo, &dir, &source, &rendered)?;
            Ok(Rerendered::Migrated { recorded_build: Some(recorded_build), vouched: false })
        }
        Provenance::Unattributed(diff) => {
            if !unattributed {
                return Err(format!(
                    "it has no render record, so nothing says whether the document, the snapshot \
or the renderer moved ({}). If you know the renderer is the only thing that changed, re-run with \
--unattributed to vouch for it",
                    diff.describe()
                ));
            }
            migrate(memo, &dir, &source, &rendered)?;
            Ok(Rerendered::Migrated { recorded_build: None, vouched: true })
        }
        Provenance::DocumentEdited(diff) => Err(format!(
            "the document differs from its render record ({}) — refusing to overwrite an edit",
            diff.describe()
        )),
        Provenance::SnapshotEdited { files, diff } => Err(format!(
            "snapshot files differ from its render record: {} ({}) — refusing to seal an edit",
            files.join(", "),
            diff.describe()
        )),
        Provenance::Unreadable(e) => Err(e),
        Provenance::Missing => unreachable!("the snapshot directory was checked above"),
    }
}

/// Rewrite `memo` as `rendered` and seal it — unless the rewrite would
/// change the ledger's claim set or any claim's proposition.
fn migrate(memo: &Path, dir: &Path, source: &str, rendered: &str) -> Result<(), String> {
    let changed = ledger_changes(source, rendered);
    if !changed.is_empty() {
        return Err(format!(
            "re-rendering would change the ledger for {} — their grading records are keyed to \
the propositions as the document states them now, so the rewrite would put them out of proof",
            changed.join(", ")
        ));
    }
    let io_err = |e: io::Error| format!("could not write ({e})");
    remove_record(dir).map_err(io_err)?;
    fs::write(memo, rendered).map_err(io_err)?;
    write_record(dir, rendered).map_err(io_err)
}

/// Claim ids whose presence or proposition differs between two documents'
/// ledgers, as `ledger::import` reads them — the reading every
/// `proposition_digest` on record was computed over.
fn ledger_changes(before: &str, after: &str) -> Vec<String> {
    let read = |s: &str| -> BTreeMap<String, String> {
        crate::ledger::import(&crate::parse::parse_document(s).body)
            .claims
            .into_iter()
            .map(|c| (c.id, c.proposition))
            .collect()
    };
    let (a, b) = (read(before), read(after));
    let ids: std::collections::BTreeSet<&String> =
        a.keys().chain(b.keys()).filter(|id| a.get(*id) != b.get(*id)).collect();
    ids.into_iter().cloned().collect()
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
    fn no_verifier_output_can_reach_a_snapshot() {
        // A snapshot is the thing a reader is entitled to recompute, and
        // the verifier's findings are the one class of text in a workspace
        // that does not recompute: the same input can produce a different
        // answer. They stay out by construction rather than by manners —
        // `write` walks exactly this array and copies exactly these names,
        // so a file whose name is absent cannot be shipped at all.
        // Shipping one later would take a deliberate line here.
        for name in SNAPSHOT_FILES {
            assert!(
                !name.starts_with("verify"),
                "`{name}` would ship non-reproducible model output into a snapshot"
            );
        }
    }

    #[test]
    fn a_missing_snapshot_directory_is_missing_not_a_match() {
        assert!(matches!(
            check(Path::new("/nonexistent/memo.md"), "whatever"),
            Provenance::Missing
        ));
    }

    /// A fresh pair of directories — a workspace and the directory a memo
    /// is published into — private to one test.
    fn scratch(tag: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "tetel-snapshot-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let ws = root.join("ws");
        fs::create_dir_all(&ws).unwrap();
        (ws, root.join("memo.md"))
    }

    /// C15 (v): bytes the snapshot does not re-render leave no record —
    /// not a new one, and not the one that was there before. Mutation A
    /// (write the record unconditionally) and mutation B (drop the
    /// up-front removal) each leave a `render.json` behind.
    #[test]
    fn a_publication_that_does_not_reproduce_leaves_no_record() {
        let (ws, memo) = scratch("no-reproduce");
        let dir = snapshot_path(&memo);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(RECORD_FILE), "{\"from\": \"an earlier render\"}\n").unwrap();

        let err = write(&memo, &ws, "not what this workspace renders\n").unwrap_err();
        assert!(err.to_string().contains("no render record was written"), "{err}");
        assert!(!dir.join(RECORD_FILE).exists(), "no record may survive a failed certification");
        // The document itself was still written: the failure is reported,
        // and `check` reads the unrecorded pair as unattributed drift.
        assert_eq!(fs::read_to_string(&memo).unwrap(), "not what this workspace renders\n");
    }

    /// C15 (xvi): a file an earlier workspace put in the snapshot is
    /// removed when the workspace publishing now lacks it, and the
    /// publication still certifies. Mutation: the copy-only loop leaves
    /// the file in place.
    ///
    /// The left-over file is `refusals.log`, which render never reads, so
    /// under the mutation the publication still succeeds and the mirror
    /// assertion alone goes red. A left-over `targets.jsonl` fails the
    /// same mutant earlier, at certification, which would not show which
    /// assertion caught it.
    #[test]
    fn publication_mirrors_the_workspace() {
        let (ws, memo) = scratch("mirror");
        let dir = snapshot_path(&memo);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("refusals.log"), "left over from an earlier workspace\n").unwrap();

        let rendered = crate::compose::render(&ws).unwrap();
        write(&memo, &ws, &rendered).unwrap();
        assert!(!dir.join("refusals.log").exists(), "a file the workspace lacks must not be carried forward");
        let record = read_record(&dir).unwrap().expect("a reproducing publication is recorded");
        assert_eq!(record.document, sha256_bytes(rendered.as_bytes()));
        assert!(record.files.contains_key("identity.json"), "{record:?}");
        assert!(!record.files.contains_key("refusals.log"), "{record:?}");
    }

    /// C15 (xvii): a publication that fails at the document write, with a
    /// record from an earlier render in place, leaves no record. Mutation:
    /// removing the record only after the document is written.
    #[test]
    fn a_publication_failing_at_the_document_leaves_no_record() {
        let (ws, memo) = scratch("doc-write-fails");
        let dir = snapshot_path(&memo);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(RECORD_FILE), "{}\n").unwrap();
        // A directory where the document should go: the write fails.
        fs::create_dir_all(&memo).unwrap();

        let rendered = crate::compose::render(&ws).unwrap();
        assert!(write(&memo, &ws, &rendered).is_err());
        assert!(!dir.join(RECORD_FILE).exists(), "the old record must be gone before the document write");
    }
}
