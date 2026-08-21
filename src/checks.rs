//! Runs the five reddening checks plus the report-only observations
//! (cited-but-undefined, defined-but-uncited) against a parsed [`Document`].
//!
//! Check 5 (the dependency-propagation cascade) shares its citation
//! syntax with check 4 but not its scope: check 4 owns direct, hop-1
//! prose citations of an unsettled row. Check 5 owns everything check 4
//! cannot see — row→row citation edges (found inside a row's own
//! `claim`/`note` fields, at any hop, including hop 1) and any citation,
//! prose or row, reached transitively at hop ≥ 2. Between them every
//! citation of an unsettled row is caught exactly once.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use crate::acks::AckEvent;
use crate::citations::{
    abutting_context, citation_ids_in, normalize_literal, scan_citations, AbuttingContext, Citation,
};
use crate::claims::Claim as AckClaim;
use crate::evidence::{EvidenceRecord, Source, Verdict};
use crate::ledger::Claim;
use crate::model::{Designator, Kind, Row, Status};
use crate::parse::Document;
use crate::prose::ProseEvent;

/// Everything the report needs, already computed. Kept as plain owned
/// data (not references into `Document`) so it's simple to assert on in
/// tests and simple to render.
pub struct Findings {
    /// Formatted `line N: message`, sorted by line — check 1.
    pub grammar_errors: Vec<String>,
    /// Formatted failures — check 2, enumerated rows only.
    pub subset_failures: Vec<String>,
    /// Rows check 2 could not run on at all: `(id, claim)`.
    pub coverage_skipped: Vec<(String, String)>,
    /// Formatted failures — check 3.
    pub abutting_failures: Vec<String>,
    /// Formatted informational near-misses for check 3 (never fail).
    pub abutting_candidates: Vec<String>,
    /// Formatted failures — check 4.
    pub unsettled_failures: Vec<String>,
    /// Formatted failures — check 5. One entry per unsettled root that
    /// has at least one qualifying dependent, each entry a multi-line
    /// block naming every dependent under that root (grouped, not one
    /// finding per dependent — see the module doc comment).
    pub cascade_failures: Vec<String>,
    /// IDs cited in prose with no matching row.
    pub cited_undefined: Vec<String>,
    /// Rows never cited anywhere: `(id, claim)`.
    pub defined_uncited: Vec<(String, String)>,
    /// READING/OBSERVED/ATTESTED rows: `(id, "KIND/STATUS", claim)`.
    pub human_owed_rows: Vec<(String, String, String)>,
    /// IDs of every RUN row, for the class-level correspondence line.
    pub run_row_ids: Vec<String>,

    // --- the grounding brief/record/check slice ---------------------
    /// How many claims the memo's evidence ledger table(s) carried, valid
    /// or not — distinguishes "no ledger in this file" from "a ledger
    /// with zero rows" for the report's early-exit guard.
    pub ledger_claims_found: usize,
    /// Formatted `line N: message` — ledger rows that could not be
    /// parsed at all (wrong cell count, empty id, duplicate id). Never
    /// silently dropped.
    pub ledger_errors: Vec<String>,
    /// A ledger claim with no evidence record at all: `(id, proposition)`.
    /// Human-owed — absence of evidence is not itself a failure.
    pub ungrounded_claims: Vec<(String, String)>,
    /// A ledger claim grounded, but only by evidence that derives to
    /// [`Kind::Attested`] for standing — today, that's *every* grounded
    /// claim, since ingestion (`tetel record`) is the only write path and
    /// every ingested record derives to `Attested` regardless of what it
    /// claims (see `evidence::EvidenceRecord::derived_kind`). Distinct
    /// from `ungrounded_claims`: this is "someone looked, off-instrument",
    /// not "nobody looked". `(id, proposition)`, human-owed, never a
    /// failure — it stops being the whole list only once a witnessed
    /// grounding exists to sit beside ingested ones.
    pub attested_grounded_claims: Vec<(String, String)>,
    /// One line per evidence record whose `source` is a path that does not
    /// resolve on disk. Human-owed, never a failure — mirrors the
    /// non-failing disposition of `cited_undefined`, one line per record
    /// (not aggregated into a bracketed list) since each names a distinct
    /// claim and path.
    pub unresolved_evidence_sources: Vec<String>,
    /// A ledger claim whose Domain/Extent cells are exactly
    /// [`crate::ledger::NO_SCOPE_DECLARED`] — minted by `tetel claim` via
    /// `compose::render`, which has no scope field to draw from.
    /// Human-owed, never a failure: distinct from `coverage_skipped`
    /// (a *declared* domain/extent this crate's checks can't enumerate,
    /// e.g. a `proc:`/`external:` designator) — this is a domain/extent
    /// that was never declared at all, and it says so plainly rather than
    /// let a silently-passing check imply a coverage claim nobody made.
    pub no_scope_claims: Vec<(String, String)>,
    /// Formatted failures: two evidence records for one claim disagreeing
    /// on verdict, or a verdict contradicting the author's own Status
    /// cell. A machine failure — an unresolved contradiction, the same
    /// shape as the unsettled-citation check.
    pub verdict_disagreements: Vec<String>,
    /// Whether the memo still matches the workspace snapshot beside it.
    /// See [`crate::snapshot`] for why a rendered document is not
    /// self-contained and what a mismatch means.
    pub provenance: crate::snapshot::Provenance,
    /// Whether the memo cites anything at all. A document with no
    /// citations owes no snapshot, so a missing one is only worth
    /// reporting when something in the text points into a workspace.
    pub cites_something: bool,
    /// Facts whose note names a location their own captured extent does
    /// not cover. Human-owed: see [`crate::scope`] for why this is a
    /// scope check rather than a truth check, and why it never refuses.
    /// Only populated when a snapshot shipped beside the memo, since
    /// facts appear nowhere in the rendered document.
    pub notes_outside_extent: Vec<crate::scope::OutsideExtent>,
    /// One line per fact whose extent contains a pre-dialect no-match or
    /// whole-search record — see [`pre_dialect_no_matches`] and
    /// [`crate::facts::ExtentEntry::pre_dialect_unescaped_ere_metachar`].
    /// Human-owed, verbatim, never a failure: TET-68's dialect fix cannot
    /// repair an already-minted extent, only point at it. Like
    /// `notes_outside_extent`, only populated when a snapshot shipped
    /// beside the memo.
    pub pre_dialect_no_matches: Vec<String>,
    /// Refusals recorded between one fact's mint and the previous one —
    /// what the author tried and could not do in the window that produced
    /// each fact. Human-owed and verbatim; a mint following a refusal is
    /// frequently correct. Like `notes_outside_extent`, only populated
    /// when a snapshot shipped beside the memo, and only as complete as
    /// the log inside it.
    pub mint_windows: Vec<crate::facts::MintWindow>,
    /// Working trees this memo's facts observed in more than one state,
    /// and the facts that saw each. A record rather than a warning — two
    /// facts seeing one repository in two states is the normal condition
    /// of any design that outlives an edit. What it settles is whether
    /// two facts asserting opposite things about the same code disagree
    /// about the world or about the same world, which before this the
    /// ledger could not answer at all. Same snapshot dependency as
    /// `mint_windows`.
    pub tree_states: Vec<crate::worldstate::RootStates>,
    /// Facts whose markers predate roots being recorded, so their tree
    /// state describes the process's working directory rather than what
    /// they read. Reported rather than passed over: an ungradable record
    /// is not a matching one.
    pub tree_ungradable: Vec<String>,
    /// Declared modification targets whose cited fact does not census
    /// them, re-verified against the shipped snapshot — a **machine
    /// failure**.
    ///
    /// `tetel target` refuses these at declaration, so a workspace-
    /// authored memo cannot produce one. What this catches is the
    /// document that never passed through that verb: hand-authored,
    /// edited after rendering, or carrying a target row its own snapshot
    /// does not have. Both directions are checked, because tampering can
    /// go either way — a row invented in the document, or a target whose
    /// citation stopped censusing it.
    ///
    /// It is an objective contradiction between a document and the record
    /// it claims to rest on, which is what the machine partition is for.
    pub uncensused_targets: Vec<String>,
    /// Target rows in a document that shipped no snapshot, so nothing
    /// can verify them — **human-owed**, never a failure.
    ///
    /// Same standing choice as every other snapshot-dependent finding: a
    /// missing snapshot grades the tooling's history rather than the
    /// document, and every memo authored before `render --out` existed
    /// lacks one.
    pub unverifiable_targets: Vec<String>,
    /// Transplant rows whose premise inventory does not hold up against
    /// the shipped snapshot — **machine failure**.
    ///
    /// The verb refuses a premise that is not the donor's words, and
    /// `render --out` refuses a document with an unanswered one, so a
    /// finding here can only mean a document that never passed through
    /// either: hand-authored, edited after rendering, or carrying a
    /// snapshot that does not match it.
    pub unquoted_premises: Vec<String>,
    /// Transplant rows in a document that shipped no snapshot, so nothing
    /// can verify them — **human-owed**, never a failure. Same standing
    /// choice as [`Findings::unverifiable_targets`].
    pub unverifiable_transplants: Vec<String>,
    /// True when the memo's ledger has no scope columns for any claim to
    /// declare into — a tetel-authored ledger. Reported once at document
    /// level rather than once per claim; see
    /// [`claims_without_declared_scope`] for why that changed.
    pub ledger_has_no_scope_columns: bool,
    /// One line per grounded claim describing how its evidence stands:
    /// witnessed (extent captured by this tool, in a named workspace) or
    /// ingested (extent typed by a caller). Human-owed, printed item by
    /// item, never a failure — see [`grounding_provenance`].
    pub grounding_provenance: Vec<String>,
    /// `(claim id, pass, note)` per record whose verdict is
    /// [`Verdict::Qualifies`]. Human-owed, never a failure.
    ///
    /// Until this existed, a qualifying verdict was invisible. On the
    /// first real independent grounding pass, the two claims the grounder
    /// declined to confirm — because the numbers they rest on could not
    /// be verified from what it was given — printed as plainly "grounded",
    /// while the one claim where it qualified a single premise of a
    /// multi-premise argument was the only thing reddening the report.
    /// The check fired on the wrong claim and was silent on the right
    /// ones.
    pub qualified_claims: Vec<(String, String, String)>,
    /// Claims **out of proof**: every record on file grades a wording this
    /// claim no longer carries, and nothing has graded the current one.
    /// The proof-house sense exactly — the stamp no longer certifies this
    /// barrel. A machine failure; see [`analyze_ledger`].
    ///
    /// The remedy is to **reprove**: ground the claim again against what it
    /// now says. Nothing else clears it, and nothing clears it silently —
    /// the ledger is append-only, so the superseding record is added rather
    /// than the stale one edited.
    ///
    /// **One entry per claim, not per record.** A claim revised several
    /// times before its first reprove can carry several stale records; the
    /// original per-record loop pushed one row for each of them, so a busy
    /// revision history read as that many separate machine failures. It
    /// is one — the claim is out of proof — so it is one row.
    ///
    /// The aggregated row drops the per-record pass, verdict and note the
    /// old shape inlined, and carries a `grep` pointer at the ledger in
    /// their place (code review, F3) — this is the exit-1 partition, and
    /// a row that says nothing but a count and a claim id gives an
    /// author nothing to act on. See the comment in [`analyze_ledger`]
    /// where the pointer is built for why it greps `name`, the on-disk
    /// in-toto key, and not `claim_id`, the in-memory field.
    pub out_of_proof: Vec<String>,
    /// Records that graded an earlier wording of a claim which *has* since
    /// been reproved. Human-owed history, never a failure — the claim is
    /// back in proof, and these are the marks from before it was. See
    /// [`analyze_ledger`] on why the distinction matters.
    ///
    /// **One entry per claim, not per record**, for the same reason as
    /// [`Findings::out_of_proof`]. Measured on this crate's own real
    /// memos (TET-73): the largest, a 375-record ledger
    /// (fork94-update-ref-audit-prune-asymmetry, 27 claims total), had
    /// 216 stale records spread across 19 of those claims — one row
    /// each, under the old shape, would have put 216 history entries in
    /// front of a reader for the 19. `out_of_proof` was 0 on every real
    /// memo checked: a memo that has been reproved at all has nothing
    /// left out of proof, so in practice this list, not `out_of_proof`,
    /// is where a revised memo's history actually accumulates.
    pub superseded_evidence: Vec<String>,
    /// Prose blocks whose text (or citations) postdate the settling of
    /// what they cite — see [`prose_after_proof`] for the rule and why
    /// its anchor is first proof rather than last grading pass.
    /// Human-owed, never a failure: this says a paragraph's wording is
    /// younger than the evidence it rests on, not that the paragraph is
    /// wrong. Only populated when a snapshot shipped beside the memo,
    /// same as [`Findings::mint_windows`] — the prose history this reads
    /// lives only in the workspace's `prose.jsonl`.
    pub prose_revised_since_proof: Vec<ProseRevisedSinceProof>,
    /// Blocks [`prose_revised_since_proof`] would otherwise have listed,
    /// but for which some `tetel prose --ack` matches the check's own
    /// recomputation — see [`prose_after_proof`] for exactly what
    /// "matches" requires. Human-owed, never a failure: the residue's
    /// own act, discharged by a human's own words rather than settled by
    /// this tool. A filter over `prose_revised_since_proof`'s trigger,
    /// never an input to it, so this can only ever shrink the other
    /// list, never grow it.
    pub prose_acknowledged: Vec<AcknowledgedBlock>,
    /// `Some(message)` when the snapshot ships an `acks.jsonl` that
    /// could not be parsed. A **machine failure** — unlike a corrupt
    /// `prose.jsonl`, `compose::render` never reads this file, so
    /// nothing else has already reddened provenance on its behalf; see
    /// [`Findings::machine_check_failed`]. `None` covers both "no
    /// `acks.jsonl` shipped" (an empty log, never an error — see
    /// [`crate::workspace::read_jsonl`]) and "it parsed cleanly".
    pub acks_unreadable: Option<String>,
}

impl Findings {
    pub fn machine_check_failed(&self) -> bool {
        !self.grammar_errors.is_empty()
            || !self.subset_failures.is_empty()
            || !self.abutting_failures.is_empty()
            || !self.unsettled_failures.is_empty()
            || !self.cascade_failures.is_empty()
            || !self.ledger_errors.is_empty()
            || !self.verdict_disagreements.is_empty()
            || !self.out_of_proof.is_empty()
            || !self.uncensused_targets.is_empty()
            || !self.unquoted_premises.is_empty()
            || self.acks_unreadable.is_some()
            || self.provenance_failed()
    }

    /// Drift and an unreadable snapshot are machine failures: both are
    /// objective contradictions between a document and the record it
    /// claims to rest on, decidable without a human.
    ///
    /// A *missing* snapshot is deliberately not a failure. Every memo
    /// authored before `render --out` existed lacks one, and turning that
    /// into a hard failure would grade the tooling's own history rather
    /// than the document. It is reported as human-owed instead.
    pub fn provenance_failed(&self) -> bool {
        matches!(
            self.provenance,
            crate::snapshot::Provenance::Drifted { .. }
                | crate::snapshot::Provenance::Unreadable(_)
        )
    }
}

/// One prose block whose text — or citation list — postdates the
/// settling of every in-proof claim it cites. See [`prose_after_proof`]
/// for the full rule this is the output of.
pub struct ProseRevisedSinceProof {
    pub block_id: String,
    /// 1-based line in the rendered document where this block's text
    /// begins. `compose::render` never writes a block's id into the
    /// document, so this is filled in by the caller from
    /// [`crate::compose::block_offsets`] — `prose_after_proof` itself
    /// takes no dependency on `compose`, since its own two inputs are
    /// exactly the snapshot's prose log and the claims/evidence
    /// `check_file` already holds (see that function's doc comment).
    /// `None` if the block's id is absent from the offset map (should
    /// not happen when both are read from the same snapshot, but this is
    /// display metadata, never fabricated — a crate whose whole thesis
    /// is not overstating provenance must not print a line number that
    /// does not exist as if it did).
    pub line: Option<usize>,
    /// Every in-proof claim this block cites, each with that claim's own
    /// first-proof timestamp and the pass whose record achieved it —
    /// the inputs the anchor comparison takes, printed so a reader can
    /// disagree with the conclusion rather than trust it. A cited claim
    /// that is ungrounded or out of proof is omitted here (it already
    /// prints under `ungrounded`/`out-of-proof`) and contributed nothing
    /// to the anchor either — see the existential-gate note below.
    pub cited: Vec<(String, u64, String)>,
    /// The block's own text timestamp: the earliest event in this
    /// block's history whose text *and* citation list both equal the
    /// current ones.
    pub block_timestamp: u64,
    /// The `why` the author gave for the event that produced
    /// `block_timestamp`, when that event was a [`ProseEvent::Revise`] —
    /// the author's own stated reason for the edit, carried verbatim
    /// rather than paraphrased (the never-paraphrase rule permits this:
    /// it is the author's own words, not a rendering of their prose).
    /// `None` when that event was a [`ProseEvent::Create`], which has no
    /// `why` to carry — a block can enter the list on its first version,
    /// never revised at all, and printing an empty string in that case
    /// would fabricate a reason nobody gave.
    pub why: Option<String>,
}

/// One prose block whose [`ProseRevisedSinceProof`] listing was
/// discharged by a matching `tetel prose --ack`. See
/// [`prose_after_proof`] for exactly what "matching" requires: the
/// block's current text, citation list and every cited claim's current
/// digest all equal to what the check recomputes, and the ack's own
/// minting identity equal to the snapshot's own — no timestamp compared
/// anywhere.
pub struct AcknowledgedBlock {
    pub block_id: String,
    /// Same fill-in-by-caller convention as
    /// [`ProseRevisedSinceProof::line`] — `None` rather than a
    /// fabricated line number when the offset lookup misses.
    pub line: Option<usize>,
    /// The block's current citation list, printed beside the author's
    /// reason so a reader can pair the two without re-opening the
    /// document — see the non-coverage entry on why this is as far as
    /// per-citation attribution goes.
    pub cited: Vec<String>,
    /// The *earliest* matching ack's timestamp. More than one ack can
    /// match one block's current key at once (a repeat ack, or a second
    /// ack after the first was itself superseded by an edit and then the
    /// edit reverted); this is the moment the discharge was first made,
    /// not the latest restatement of it, mirroring
    /// `prose_after_proof`'s own use of the *earliest* event bearing a
    /// block's current wording.
    pub timestamp: u64,
    /// The earliest matching ack's own verbatim reason. Never a
    /// paraphrase, same rule as [`ProseRevisedSinceProof::why`].
    pub why: String,
}

/// A prose block is listed when it is a paragraph (not a heading), it
/// cites at least one id, at least one cited id resolves to a claim in
/// `claims` that is **in proof**, and the block's text timestamp is
/// strictly later than the latest **first proof** among those in-proof
/// citations.
///
/// # The anchor is first proof, not last pass
///
/// Every grounding pass is briefed on every claim, whether or not it has
/// changed, so a pass that finds nothing to change still appends a fresh
/// record against the unchanged digest. Anchoring at the *latest*
/// grading would walk forward every round — fix-round prose written in
/// round three would stop being "since the last pass" the moment round
/// four ran, even though no pass had read a word of it. Anchoring at the
/// *earliest* record grading a claim's *current* wording fixes that: a
/// later pass over unchanged text cannot move it, only a revision of the
/// claim followed by a grading of the new wording does. Per claim this
/// takes the earliest in-proof record (that is when the wording
/// settled); per block it takes the *latest* of those (a block is not
/// anchored until every claim it leans on has settled — taking the
/// earliest instead would list nearly every paragraph in a memo, since
/// almost all prose postdates *some* first-graded citation).
///
/// # The pair gate: text *and* citations, not the last event
///
/// A block's text timestamp is looked up by content, not by its last
/// event, and by *both* the text and the citation list together. Both
/// halves close a specific attack:
///
/// - Keyed on the last event instead of on content: `prose.jsonl` is
///   append-only, so reverting a paragraph to the wording its claims
///   settled under still appends a strictly later timestamp, and a
///   listing could never clear.
/// - Keyed on text alone instead of the pair: `prose::revise` never
///   compares text and its `cite` parameter exists precisely so an
///   author can add a citation to unchanged prose. An author could leave
///   the text untouched and repoint the citation list at a claim the fix
///   round had legitimately settled — the anchor moves forward, but a
///   text-only key would still find the *original* (pre-citation-change)
///   timestamp as the earliest matching version, silencing the listing
///   even though this exact paragraph now rests on a claim it never
///   named when it was last actually read against its evidence.
///
/// Keying on the pair means changing either moves the timestamp with it,
/// so only two acts ever clear a listing: restoring the exact text and
/// citations the claims settled under, or revising/adding a cited claim
/// and having the new wording graded.
///
/// # Existential, not universal
///
/// Only one cited id needs to resolve to an in-proof claim. Requiring
/// *every* citation to be in proof was tried first and rejected: it let
/// a single record-less citation suppress a block with nothing red to
/// show for it, since a claim with no records is human-owed
/// (`ungrounded`) rather than a machine failure. A record-less or
/// out-of-proof citation contributes nothing to the anchor either way —
/// it simply is not in `evidence`'s in-proof set for its claim — so the
/// existential gate costs nothing and closes that hole.
///
/// # Two inputs, and the one that fails silently if swapped
///
/// `prose_events` must be read directly from the snapshot's
/// `prose.jsonl` — [`crate::prose::load_all`] replays the log into
/// [`crate::prose::Block`], which keeps only a revision count and
/// discards every timestamp, so this function cannot be built on top of
/// it.
///
/// `claims` must be the ones `check_file` already imports from the
/// *rendered document* (`ledger::import`), **never** the snapshot's
/// `claims.jsonl`. Every `proposition_digest` on record was computed
/// over a ledger-derived proposition, and the two are not the same
/// string: rendering a claim into the ledger table
/// (`compose::ledger_cell`) replaces embedded newlines with spaces, and
/// importing (`ledger::split_row_cells`) trims every cell. Handed
/// `claims.jsonl`'s propositions instead, a claim whose text has an
/// embedded newline or edge whitespace would digest to something no
/// record ever matches, this function would call it never in proof, and
/// every block citing it would silently drop out of the list — with
/// nothing red anywhere to say so.
///
/// # First proof, not first grading — and not `analyze_ledger`'s digest test
///
/// This function's in-proof test **deliberately diverges** from
/// [`analyze_ledger`]'s in two ways, both load-bearing, both explained here
/// rather than left to be rediscovered as a mismatch.
///
/// **A refuting record is not proof.** `analyze_ledger` answers "does any
/// record on file grade the wording this claim carries today" — verdict-
/// blind, because that question is only about whether the claim was ever
/// re-examined against its current text, and *that* a claim was refuted is
/// reported through a wholly separate channel (`verdict_disagreements`), not
/// through `out_of_proof`. This function answers a different question: "when
/// did this wording enter *proof*" — a word this crate then prints to a
/// reader as `first entered proof at …`. A record whose verdict is
/// [`Verdict::Refutes`] examined the wording and rejected it; counting it
/// as the moment the wording "entered proof" would print a false statement
/// about a refuted claim, and would let a later refutation raise a block's
/// `max` anchor and silence a listing for the paragraph a refutation makes
/// most worth a human's attention. So a refuting record is skipped here —
/// a claim graded only by refutations has no first proof and contributes
/// nothing to any block's anchor, exactly as an ungrounded claim does not.
/// [`Verdict::Qualifies`] is left counting, matching `analyze_ledger`'s own
/// treatment of it as a valid (if noted) grounding, never a contradiction.
///
/// **An empty digest is not evidence of *when* the current wording
/// settled.** `analyze_ledger` grandfathers a record with no digest at all
/// (written before the field existed) into "grades the current text",
/// because failing to do so would silently retire its checks for every
/// pre-digest document — and grandfathering there fails loudly, in the safe
/// direction: at worst it reports a disagreement a revision has since
/// resolved. Here the empty digest would instead be *read as a timestamp*
/// for the event "this wording settled" — a fact the record does not
/// attest to, since it predates the field that would let it say which
/// wording it graded. Reusing the grandfather here fails in the *unsafe*
/// direction: it can anchor a block years before its current wording was
/// ever actually graded, silencing a listing that should have fired (see
/// the module-level bug this fixed, reproduced in
/// `empty_digest_grandfathering_does_not_anchor_before_current_wording_was_ever_graded`).
/// So only a record whose digest **exactly matches** the claim's current
/// proposition counts here; a claim graded only by pre-digest records has
/// no first proof, the same non-finding an ungrounded claim gets. This can
/// under-report a legacy-only claim that in fact still matches its current
/// wording — the safe direction for a check that is silent by default (see
/// `report::render`'s preamble on this check's own clock caveat) — rather
/// than over-report by trusting a timestamp the record never promised.
///
/// # Acknowledgement is a filter over this trigger, never an input to it
///
/// `acks` and `ack_claims` exist only to *discharge* an entry this
/// function would otherwise have produced — they can never manufacture
/// one. The trigger above (text/citation pair postdating the anchor) is
/// computed exactly as it always was; only afterwards, for a block that
/// trigger already listed, is a match against `acks` attempted, and a
/// match moves the entry from the first returned partition to the
/// second rather than dropping it.
///
/// A match requires the ack's `block`, `text` and `cite` to equal the
/// block's current ones, its `digests` to equal one sha256 digest per
/// entry of the current `cite` — taken over each claim's proposition
/// **as `ack_claims` holds it**, deliberately not `claims` above, see
/// [`crate::acks`]'s module doc comment — and its `identity` to equal
/// `ack_identity`, the snapshot's own. All four are pure equality over
/// recomputed values; no timestamp is compared. When more than one ack
/// matches, the *earliest* is used, mirroring this function's own use of
/// the earliest event bearing a block's current wording.
pub fn prose_after_proof(
    prose_events: &[ProseEvent],
    claims: &[Claim],
    evidence: &[EvidenceRecord],
    acks: &[AckEvent],
    ack_claims: &[AckClaim],
    ack_identity: Option<&str>,
) -> (Vec<ProseRevisedSinceProof>, Vec<AcknowledgedBlock>) {
    // Per claim, the earliest in-proof record: its timestamp and the
    // pass that wrote it. A claim absent from this map has no first
    // proof — either ungrounded, out of proof, graded only by a
    // refutation, or graded only by pre-digest records — and contributes
    // nothing to any block's anchor. See the doc comment above for why
    // this is *not* `analyze_ledger`'s own digest test.
    let mut first_proof: HashMap<&str, (u64, &str)> = HashMap::new();
    for claim in claims {
        let current = crate::evidence::sha256_hex(&claim.proposition);
        let mut best: Option<(u64, &str)> = None;
        for r in evidence.iter().filter(|r| r.claim_id == claim.id) {
            // Only a record that actually grades today's wording — an
            // empty (pre-digest) digest does not qualify, unlike
            // `analyze_ledger`'s grandfathering (see doc comment).
            if r.proposition_digest != current {
                continue;
            }
            // A refutation examined this wording and rejected it; it did
            // not put the wording in proof (see doc comment).
            if r.verdict == Verdict::Refutes {
                continue;
            }
            best = Some(match best {
                None => (r.timestamp, r.pass.as_str()),
                Some((t, _)) if r.timestamp < t => (r.timestamp, r.pass.as_str()),
                Some(existing) => existing,
            });
        }
        if let Some(b) = best {
            first_proof.insert(claim.id.as_str(), b);
        }
    }

    // Per block, its full history as (timestamp, text, cite-list, why)
    // after each event, in event order — cite carried forward across a
    // Revise that leaves it unstated, exactly as `prose::load_all` does.
    // `why` is `None` for a `Create` (which has none) and `Some` for a
    // `Revise` (which always carries one) — read back below to fill
    // `ProseRevisedSinceProof::why`.
    struct History {
        heading: bool,
        versions: Vec<(u64, String, Vec<String>, Option<String>)>,
    }
    let mut order: Vec<String> = Vec::new();
    let mut blocks: HashMap<String, History> = HashMap::new();
    for ev in prose_events {
        match ev {
            ProseEvent::Create { id, heading, text, cite, before, timestamp, .. } => {
                // Guard matches `prose::load_all`'s dedup of a repeated
                // `Create` for one id: reachable via a hand-edited log,
                // and via a race in `workspace::next_id`'s unlocked
                // read-modify-write of `counters.json` under concurrent
                // `tetel prose` invocations (verified: `load_counters`
                // reads, increments in memory, `save_counters` writes —
                // no lock, no compare-and-swap — so two processes can
                // both read the same counter and mint the same id).
                // `load_all` pushes the id into its order list only
                // once (a second push is a no-op there because
                // `filter_map`'s `by_id.remove` finds nothing left the
                // second time), so the id keeps its *first*-occurrence
                // position while `by_id.insert` — like `blocks.insert`
                // below — still lets the *last* Create's content win.
                // Without this guard, `order` carries the id twice and
                // the final loop below emits the same (already-merged)
                // block once per `Create`, not once per block.
                //
                // `before` is honoured the same way `load_all` honours
                // it — inserted at the anchor's current position if
                // still present, appended otherwise — so this function's
                // `order`, and therefore the entries it reports, follow
                // *document* order rather than authoring (event) order.
                // Without this, a memo authored with `prose --before`
                // (see prose.rs's module doc comment on why insertion
                // exists) prints its residue entries out of the order a
                // reader sees them in the rendered document.
                if !blocks.contains_key(id) {
                    match before.as_deref().and_then(|b| order.iter().position(|o| o == b)) {
                        Some(at) => order.insert(at, id.clone()),
                        None => order.push(id.clone()),
                    }
                }
                blocks.insert(
                    id.clone(),
                    History {
                        heading: *heading,
                        versions: vec![(*timestamp, text.clone(), cite.clone(), None)],
                    },
                );
            }
            ProseEvent::Revise { id, text, cite, why, timestamp, .. } => {
                if let Some(h) = blocks.get_mut(id) {
                    let carried = h.versions.last().map(|(_, _, c, _)| c.clone()).unwrap_or_default();
                    let cite = cite.clone().unwrap_or(carried);
                    h.versions.push((*timestamp, text.clone(), cite, Some(why.clone())));
                }
            }
        }
    }

    // Digest per claim id, taken over the proposition as `ack_claims`
    // (claims.jsonl) holds it — deliberately a *different* map from
    // `first_proof` above, which digests `claims`' (ledger-derived)
    // propositions. See this function's doc comment and `crate::acks`'s
    // module doc comment for why the two must not be conflated.
    let ack_digest: HashMap<&str, String> =
        ack_claims.iter().map(|c| (c.id.as_str(), crate::evidence::sha256_hex(&c.prop))).collect();

    let mut listed = Vec::new();
    let mut acknowledged = Vec::new();
    for id in &order {
        let Some(h) = blocks.get(id) else { continue };
        if h.heading {
            continue;
        }
        let Some((_, cur_text, cur_cite, _)) = h.versions.last() else { continue };
        if cur_cite.is_empty() {
            continue;
        }

        let mut cited: Vec<(String, u64, String)> = cur_cite
            .iter()
            .filter_map(|cid| first_proof.get(cid.as_str()).map(|(t, p)| (cid.clone(), *t, (*p).to_string())))
            .collect();
        if cited.is_empty() {
            continue; // existential gate: no cited claim is in proof
        }
        cited.sort_by(|a, b| a.0.cmp(&b.0));
        let anchor = cited.iter().map(|(_, t, _)| *t).max().unwrap();

        // Earliest event whose text AND citation list both equal the
        // current ones — not the last event. See the doc comment above
        // on why keying on content, and on the pair, is load-bearing.
        // Its own `why` (present only if that event was a `Revise`)
        // travels with it, since it is the reason for *this* wording,
        // not for whichever event happened to be last.
        let (block_timestamp, why) = h
            .versions
            .iter()
            .filter(|(_, t, c, _)| t == cur_text && c == cur_cite)
            .min_by_key(|(ts, _, _, _)| *ts)
            .map(|(ts, _, _, w)| (*ts, w.clone()))
            .expect("the current version is always a member of its own history");

        if block_timestamp > anchor {
            // Try to discharge via a matching ack before listing — see
            // this function's doc comment on what "matching" requires.
            // `recomputed_digests` is `None` the moment any current
            // citation fails to resolve against `ack_claims`, which
            // makes a match impossible (as it must be: `acks::create`
            // itself refuses to mint an ack for a block citing an id
            // `claims.jsonl` cannot resolve, so no real ack could ever
            // carry a digest for one anyway).
            let recomputed_digests: Option<Vec<String>> =
                cur_cite.iter().map(|cid| ack_digest.get(cid.as_str()).cloned()).collect();
            let matched = recomputed_digests.as_ref().and_then(|digests| {
                acks.iter()
                    .filter(|a| {
                        a.block == *id
                            && a.text == *cur_text
                            && a.cite == *cur_cite
                            && a.digests == *digests
                            && ack_identity.is_some_and(|snap_id| a.identity == snap_id)
                    })
                    .min_by_key(|a| a.timestamp)
            });
            match matched {
                Some(a) => acknowledged.push(AcknowledgedBlock {
                    block_id: id.clone(),
                    line: None,
                    cited: cur_cite.clone(),
                    timestamp: a.timestamp,
                    why: a.why.clone(),
                }),
                None => listed.push(ProseRevisedSinceProof {
                    block_id: id.clone(),
                    line: None,
                    cited,
                    block_timestamp,
                    why,
                }),
            }
        }
    }
    (listed, acknowledged)
}

fn covers(extent: &[Designator], d: &Designator) -> bool {
    extent.iter().any(|e| match (e, d) {
        (Designator::Path(ep), Designator::Path(dp)) => ep == dp,
        (Designator::Path(ep), Designator::Symbol { path: dp, .. }) => ep == dp,
        (
            Designator::Symbol { path: ep, symbol: es },
            Designator::Symbol { path: dp, symbol: ds },
        ) => ep == dp && es == ds,
        _ => false,
    })
}

fn designator_display(d: &Designator) -> String {
    match d {
        Designator::Symbol { path, symbol } => format!("{path}#{symbol}"),
        Designator::Path(p) => p.clone(),
        Designator::Proc(c) => format!("proc: {c}"),
        Designator::External(t) => format!("external: {t}"),
    }
}

/// `ledger_claims` is the memo's own evidence ledger (see
/// [`crate::ledger::import`]), passed in so that a citation of a ledger
/// claim id is recognised as defined — a ledger claim is never a fenced
/// row, so `rows_by_id` alone used to be blind to it entirely (every
/// citation of one reported `cited but undefined`, on every memo
/// `render` produces, since its own evidence ledger is the only thing
/// its prose ever cites). Fixed here, not by weakening the check: a
/// citation resolving to neither a row nor a ledger claim is still
/// reported. See the `ledger_by_id` map built below for which checks a
/// ledger-claim citation does and does not participate in.
pub fn analyze(doc: &Document, ledger_claims: &[Claim]) -> Findings {
    let mut grammar_errors: Vec<(usize, String)> = doc
        .grammar_errors
        .iter()
        .map(|e| (e.line, e.message.clone()))
        .collect();

    let rows_by_id: HashMap<&str, &Row> = doc.rows.iter().map(|r| (r.id.as_str(), r)).collect();
    // A ledger claim is a citable id too — see the doc comment above on
    // why `rows_by_id` alone can't answer "is this id defined": a ledger
    // claim has no `Row` to sit in that map. This index exists only to
    // settle "is the id defined at all" (fed into the `None` arm below,
    // and into the row→row edge scan for check 5); it is deliberately
    // never consulted by check 3 (abutting literal, needs a row's
    // `value`) or check 4 (unsettled citation, needs a row's typed
    // `Status`) — a ledger claim has no equivalent of either field, and
    // inventing one (e.g. parsing `Status` prose for VERIFIED/REFUTED,
    // the way `author_status_verdict` does for a different purpose) would
    // fabricate a value this crate has no basis for. See the report for
    // which checks were affected and why each was left to not apply.
    let ledger_by_id: HashMap<&str, &Claim> = ledger_claims.iter().map(|c| (c.id.as_str(), c)).collect();
    // A third place an id can be defined: the rendered facts table.
    // Prose can cite a fact directly, and before the document carried
    // one, every such citation reported as undefined — the renderer
    // emitting an id the checker had no table to resolve it in.
    let fact_ids = crate::ledger::facts_table_ids(&doc.body);

    // Check 2 — domain ⊆ extent, enumerated rows only.
    let mut subset_failures = Vec::new();
    let mut coverage_skipped = Vec::new();
    for row in &doc.rows {
        if !row.fully_enumerated() {
            coverage_skipped.push((row.id.clone(), row.claim.clone()));
            continue;
        }
        let uncovered: Vec<String> = row
            .domain
            .iter()
            .filter(|d| !covers(&row.extent, d))
            .map(designator_display)
            .collect();
        if !uncovered.is_empty() {
            subset_failures.push(format!(
                "line {}: {} — domain not covered by extent: {}",
                row.line,
                row.id,
                uncovered.join(", ")
            ));
        }
    }

    // Citations drive checks 3 and 4, plus the two report-only lists.
    let citations = scan_citations(&doc.body);
    let mut cited_ids: HashSet<String> = HashSet::new();
    let mut cited_undefined = Vec::new();
    let mut unsettled_failures = Vec::new();
    let mut abutting_failures = Vec::new();
    let mut abutting_candidates = Vec::new();

    for cit in &citations {
        cited_ids.insert(cit.id.clone());
        match rows_by_id.get(cit.id.as_str()) {
            None => {
                // Defined by the ledger instead of a fenced row: the
                // citation resolves, so it is not cited-but-undefined —
                // but checks 3 and 4 have nothing to run against it (see
                // `ledger_by_id`'s doc comment above), so nothing further
                // happens for this arm either way.
                if !ledger_by_id.contains_key(cit.id.as_str())
                    && !fact_ids.contains(&cit.id)
                    && !cited_undefined.contains(&cit.id)
                {
                    cited_undefined.push(cit.id.clone());
                }
            }
            Some(row) => {
                // Check 4 — unmarked citation of an unsettled row; a stance
                // marker on an already-VERIFIED row is a grammar error.
                if cit.stance_refuted {
                    if row.status == Status::Verified {
                        grammar_errors.push((
                            cit.line,
                            format!(
                                "[!{}] marks a VERIFIED row as refuted — the stance marker on a VERIFIED row is itself a grammar error",
                                cit.id
                            ),
                        ));
                    }
                } else if row.status.is_unsettled() {
                    unsettled_failures.push(format!(
                        "line {}: bare citation [{}] of {} row {} — needs the [!{}] stance marker, or the row must be settled",
                        cit.line, cit.id, row.status, cit.id, cit.id
                    ));
                }

                // Check 3 — abutting-literal mismatch.
                match abutting_context(&doc.body[cit.line - 1], cit.col) {
                    AbuttingContext::Abutting(tok) => {
                        if let Some(value) = &row.value {
                            let norm = normalize_literal(&tok);
                            let value_trimmed = value.trim();
                            let value_norm = normalize_literal(value_trimmed);
                            // A prose literal matches either the row's
                            // whole value or just its trailing word —
                            // multi-token values ("code 137", "exit 0")
                            // are cited by their own last word, and this
                            // is a no-op for a value that's already a
                            // single token.
                            let value_tail = value_trimmed
                                .rsplit(char::is_whitespace)
                                .next()
                                .unwrap_or(value_trimmed);
                            let value_tail_norm = normalize_literal(value_tail);
                            if norm != value_norm && norm != value_tail_norm {
                                let marker = if cit.stance_refuted { "!" } else { "" };
                                abutting_failures.push(format!(
                                    "line {}: literal '{}' abuts [{}{}] but row {}'s value is '{}'",
                                    cit.line, norm, marker, cit.id, cit.id, value
                                ));
                            }
                        }
                    }
                    AbuttingContext::Candidate(tok) => {
                        let marker = if cit.stance_refuted { "!" } else { "" };
                        abutting_candidates.push(format!(
                            "line {}: '{}' is near citation [{}{}] but not at abutting distance — printed candidate, never a failure",
                            cit.line, normalize_literal(&tok), marker, cit.id
                        ));
                    }
                    AbuttingContext::None => {}
                }
            }
        }
    }

    // Check 5 — dependency-propagation cascade. Two edge sources feed a
    // shared graph: prose→row (`citations`, already scanned above) and
    // row→row (a row's own `claim`/`note` fields, scanned here — the
    // load-bearing new source). A self-citation is never a dependency
    // edge. A citation of an id with no matching row is folded into
    // `cited_undefined`, exactly like an undefined prose citation — same
    // non-failing disposition, same reason: nothing here validates a
    // free-text field's content beyond field-name well-formedness, and
    // treating this case differently from the identical prose case would
    // need its own justification this slice doesn't have. See the report
    // for the fuller argument.
    let mut row_citers: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    for row in &doc.rows {
        let mut seen: HashSet<String> = HashSet::new();
        let fields = std::iter::once(row.claim.as_str()).chain(row.note.as_deref());
        for field_text in fields {
            for (id, _stance) in citation_ids_in(field_text) {
                if id == row.id {
                    continue; // self-citation is not a dependency
                }
                if rows_by_id.contains_key(id.as_str()) {
                    cited_ids.insert(id.clone());
                    if seen.insert(id.clone()) {
                        row_citers
                            .entry(id.clone())
                            .or_default()
                            .push((row.id.clone(), row.line));
                    }
                } else if ledger_by_id.contains_key(id.as_str()) {
                    // Defined by the ledger, not a row: it cannot be a
                    // cascade root (no typed `Status`) or a row→row
                    // dependent (it isn't a `Row`), so it never enters
                    // `row_citers` — but it is not cited-but-undefined.
                    cited_ids.insert(id.clone());
                } else {
                    cited_ids.insert(id.clone());
                    if !cited_undefined.contains(&id) {
                        cited_undefined.push(id.clone());
                    }
                }
            }
        }
    }

    // Reverse index: which prose citations name a given row id — reused
    // from the citations already scanned for checks 3 and 4.
    let mut prose_citers: HashMap<&str, Vec<&Citation>> = HashMap::new();
    for cit in &citations {
        prose_citers.entry(cit.id.as_str()).or_default().push(cit);
    }

    // BFS, one root at a time, over the reverse graph (who cites this
    // id) starting at each unsettled row. `visited` seeds with the root
    // itself so a citation cycle back to the root terminates instead of
    // looping, and so the root is never reported as its own dependent.
    // Row hits are kept at every hop (row→row edges are never covered by
    // check 4, at any distance); prose hits only ever arrive at hop ≥ 2
    // here, because a root's own direct prose citers — hop 1 — are
    // deliberately never looked up: that's exactly what check 4 already
    // reports, so this loop must not rediscover it.
    let mut cascade_failures = Vec::new();
    for root in doc.rows.iter().filter(|r| r.status.is_unsettled()) {
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(root.id.clone());
        let mut queue: VecDeque<(String, usize, String)> = VecDeque::new();
        for (citer_id, _line) in row_citers.get(&root.id).cloned().unwrap_or_default() {
            if visited.insert(citer_id.clone()) {
                queue.push_back((citer_id, 1, root.id.clone()));
            }
        }

        let mut lines: Vec<(usize, String)> = Vec::new();
        while let Some((id, hop, cites)) = queue.pop_front() {
            let line = rows_by_id.get(id.as_str()).map(|r| r.line).unwrap_or(0);
            lines.push((hop, format!("hop {hop} [row] {id} (line {line}): cites {cites}")));

            if let Some(citers) = prose_citers.get(id.as_str()) {
                for cit in citers {
                    lines.push((
                        hop + 1,
                        format!("hop {} [prose] line {}: cites {}", hop + 1, cit.line, id),
                    ));
                }
            }
            if let Some(further) = row_citers.get(&id) {
                for (next_id, _next_line) in further {
                    if visited.insert(next_id.clone()) {
                        queue.push_back((next_id.clone(), hop + 1, id.clone()));
                    }
                }
            }
        }

        if lines.is_empty() {
            continue;
        }
        lines.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let dependents = lines.len();
        let body = lines
            .iter()
            .map(|(_, s)| format!("      {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        cascade_failures.push(format!(
            "root {} ({}) — {} dependent{}:\n{}",
            root.id,
            root.status,
            dependents,
            if dependents == 1 { "" } else { "s" },
            body
        ));
    }

    let mut defined_uncited = Vec::new();
    let mut human_owed_rows = Vec::new();
    let mut run_row_ids = Vec::new();
    for row in &doc.rows {
        if !cited_ids.contains(&row.id) {
            defined_uncited.push((row.id.clone(), row.claim.clone()));
        }
        match row.kind {
            Kind::Run => run_row_ids.push(row.id.clone()),
            Kind::Reading | Kind::Observed | Kind::Attested => {
                human_owed_rows.push((
                    row.id.clone(),
                    format!("{}/{}", row.kind, row.status),
                    row.claim.clone(),
                ));
            }
        }
    }
    // The same inverse for ledger claims: "an uncited claim prints,
    // default disposition delete" is a deliberate rule (see report.rs),
    // and it must hold for a ledger claim exactly as it does for a
    // fenced row — fixing citation resolution to see ledger claims
    // cannot also make an uncited one invisible in the other direction.
    for claim in ledger_claims {
        if !cited_ids.contains(&claim.id) {
            defined_uncited.push((claim.id.clone(), claim.proposition.clone()));
        }
    }

    grammar_errors.sort_by_key(|(line, _)| *line);
    let grammar_errors = grammar_errors
        .into_iter()
        .map(|(line, msg)| format!("line {line}: {msg}"))
        .collect();

    Findings {
        grammar_errors,
        subset_failures,
        coverage_skipped,
        abutting_failures,
        abutting_candidates,
        unsettled_failures,
        cascade_failures,
        cited_undefined,
        defined_uncited,
        human_owed_rows,
        run_row_ids,
        ledger_claims_found: 0,
        ledger_errors: Vec::new(),
        ungrounded_claims: Vec::new(),
        attested_grounded_claims: Vec::new(),
        unresolved_evidence_sources: Vec::new(),
        no_scope_claims: Vec::new(),
        verdict_disagreements: Vec::new(),
        // `analyze` grades a parsed document in isolation and never
        // touches the filesystem; provenance needs the memo's path, so
        // `check_file` fills both in.
        provenance: crate::snapshot::Provenance::Missing,
        cites_something: false,
        notes_outside_extent: Vec::new(),
        pre_dialect_no_matches: Vec::new(),
        tree_states: Vec::new(),
        tree_ungradable: Vec::new(),
        uncensused_targets: Vec::new(),
        unverifiable_targets: Vec::new(),
        unquoted_premises: Vec::new(),
        unverifiable_transplants: Vec::new(),
        mint_windows: Vec::new(),
        ledger_has_no_scope_columns: false,
        grounding_provenance: Vec::new(),
        qualified_claims: Vec::new(),
        out_of_proof: Vec::new(),
        superseded_evidence: Vec::new(),
        prose_revised_since_proof: Vec::new(),
        prose_acknowledged: Vec::new(),
        acks_unreadable: None,
    }
}

/// The author's own verdict, read off a ledger `Status` cell's leading
/// bolded keyword — high-confidence only. A `Status` cell is free prose
/// ("**DISCHARGED BY A RUN, and narrowed.**", "**CITED, NOT
/// RE-OBSERVED.**"...), and guessing a verdict out of every shape it can
/// take would mean fabricating disagreements the author never stated.
/// This only ever fires on the two unambiguous keywords the corpus's own
/// `tetel` row grammar already treats as opposites (`VERIFIED`/
/// `REFUTED`); anything else — including `OWED`-shaped or qualified
/// prose — is left unclassified rather than guessed at, mirroring
/// citations.rs's own Abutting-vs-Candidate confidence split.
fn author_status_verdict(status_cell: &str) -> Option<Verdict> {
    let verified = status_cell.find("VERIFIED");
    let refuted = status_cell.find("REFUTED");
    match (verified, refuted) {
        (Some(v), Some(r)) => Some(if v < r { Verdict::Supports } else { Verdict::Refutes }),
        (Some(_), None) => Some(Verdict::Supports),
        (None, Some(_)) => Some(Verdict::Refutes),
        (None, None) => None,
    }
}

/// `(ungrounded, attested_grounded, verdict_disagreements, qualified)`.
///
/// `qualified` is `(claim id, pass, note)` per qualifying record —
/// human-owed, never a failure. A grounder that reports "true only under
/// a condition this proposition does not state", or "I could not
/// establish this from what I was given", has produced the most
/// load-bearing sentence in its whole pass; it belongs in front of a
/// reader, not buried in a file.
type LedgerFindings = (
    Vec<(String, String)>,
    Vec<(String, String)>,
    Vec<String>,
    Vec<(String, String, String)>,
    Vec<String>,
    Vec<String>,
);

/// Quote `s` as one POSIX shell word: wrap it in single quotes, and
/// escape any single quote it contains as `'\''` (close the quoted
/// string, an escaped literal quote, reopen). A claim id comes from a
/// markdown table cell (see [`crate::ledger::import`]) and a memo path
/// comes from argv — neither is restricted to shell-safe characters, and
/// the commands built with this are printed for a human to copy and run,
/// not executed by this process, so they must be correct standing alone.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The checks this slice and the grounding-provenance slice on top of it
/// add, run independently of the five `tetel`-row checks above: a claim
/// with no evidence record at all (human-owed — absence isn't a failure);
/// a claim grounded only by evidence that derives to `Attested` standing
/// (human-owed, and distinct from the first — someone looked,
/// off-instrument); and two verdicts that contradict each other, whether
/// that's two grounding passes disagreeing or a pass contradicting the
/// author's own `Status` cell (a machine failure — an unresolved
/// contradiction).
pub fn analyze_ledger(claims: &[Claim], evidence: &[EvidenceRecord], memo: &Path) -> LedgerFindings {
    let mut ungrounded = Vec::new();
    let mut attested_grounded = Vec::new();
    let mut disagreements = Vec::new();
    let mut qualified = Vec::new();
    let mut out_of_proof = Vec::new();
    let mut superseded = Vec::new();
    // The ledger a reader can `grep` for the records this pass aggregates
    // away — computed once, from the same path `record`/`evidence::load`
    // already derive theirs from, not a placeholder.
    let ledger_path = crate::evidence::evidence_path(memo).display().to_string();
    let quoted_ledger_path = shell_single_quote(&ledger_path);

    for claim in claims {
        let records: Vec<&EvidenceRecord> = evidence.iter().filter(|e| e.claim_id == claim.id).collect();

        // A record grades a *proposition*, not an id. The digest of the
        // text it graded is written into every record; comparing it to
        // the claim's current text is what stops evidence transferring to
        // a rewritten claim.
        //
        // Without this, revising a proposition through the ordinary
        // `claim --revise` + `render --out` path left its old evidence
        // attached — including to the exact negation of what was graded,
        // with the memo still matching its snapshot, no drift, and the
        // machine partition clean.
        //
        // **Whether a stale record fails depends on what sits beside it.**
        // The first version of this check failed on any stale record, and
        // that was undischargeable: the evidence log is append-only with
        // no supersession, so re-grounding — the remedy this very check
        // printed — added a current record and left the failure standing.
        // A red nobody can clear is the defect this crate rejected in the
        // verdict-disagreement check hours earlier, rebuilt here.
        //
        // So: a claim with no record matching its current text is a
        // machine failure, because nothing grades what it now says. A
        // claim that has one is grounded, and its superseded records are
        // history — worth printing, never a failure. Re-grounding is now
        // a remedy that works.
        let current = crate::evidence::sha256_hex(&claim.proposition);
        let (fresh, stale): (Vec<&&EvidenceRecord>, Vec<&&EvidenceRecord>) = records
            .iter()
            .filter(|r| !r.proposition_digest.is_empty())
            .partition(|r| r.proposition_digest == current);

        // One row per *claim*, not per stale record. The first version of
        // this loop pushed a full row — including the attacker's whole
        // note — for every stale record on file, so a claim revised
        // several times before its first reprove read as that many
        // separate findings. Measured against this crate's own real
        // memos (TET-73): the largest ledger on file (375 records,
        // fork94-update-ref-audit-prune-asymmetry) put 216 stale records
        // in one row each, 253,692 of the report's 303,373 characters
        // (84%). It is one claim, one state (out of proof or
        // superseded), and now one row.
        if !stale.is_empty() {
            // The on-disk key is `name` — the in-toto `subject[].name`
            // field, per the Statement shape `evidence::parse_line` reads
            // (`claim_id: subject.name.clone()`, src/evidence.rs). That
            // Rust-side field is named `claim_id`; the JSON on disk never
            // is. Printing `"claim_id":"…"` here would build a command
            // that matches zero lines against every ledger this tool
            // writes — verified against this crate's own fixture,
            // `stale_evidence_aggregation.md.evidence.jsonl`.
            //
            // Both the id and the memo path are user-supplied text (a
            // markdown table cell and argv, respectively) with no
            // shell-safety guarantee, so both are quoted as one POSIX
            // shell word via `shell_single_quote` rather than
            // interpolated bare.
            let quoted_pattern = shell_single_quote(&format!("\"name\":\"{}\"", claim.id));
            let retrieval_cmd = format!("grep {quoted_pattern} {quoted_ledger_path}");
            if fresh.is_empty() {
                out_of_proof.push(format!(
                    "{} — {} record(s) grade wording this claim no longer carries, and nothing \
grades what it says today. The ledger keeps only a digest of the wording each record graded, not \
the text itself, so recovering what it said means the memo's own history, not this file. Reprove \
against the current wording, or recover the earlier wording from that history and restore it.\n      \
Retrieve the stale records with: {retrieval_cmd}",
                    claim.id,
                    stale.len(),
                ));
            } else {
                // Digests are dropped here on purpose: several stale
                // records can carry several different recorded digests,
                // and once they're aggregated "the recorded digest" no
                // longer names a single thing. A reader who needs one is
                // going to the ledger anyway — point there instead.
                superseded.push(format!(
                    "{} — {} record(s) grade wording this claim no longer carries; {} record(s) \
grade the current wording. Kept because it is what an earlier pass actually found, and the log \
is append-only.\n      Retrieve them with: {retrieval_cmd}",
                    claim.id,
                    stale.len(),
                    fresh.len(),
                ));
            }
        }

        if records.is_empty() {
            ungrounded.push((claim.id.clone(), claim.proposition.clone()));
            continue;
        }

        // Every check below asks what the evidence says about *what this
        // claim says now*, so every one of them reads this set rather
        // than `records`.
        //
        // Making only the stale/superseded partition digest-aware and
        // leaving the verdict checks comparing all records was the same
        // defect one layer down, and it punished exactly the loop this
        // crate exists to produce: a pass refutes a claim, the author
        // fixes the wording, a later pass grounds the new wording
        // unanimously — and `verdict-disagreement` still fired, forever,
        // on a supports/refutes pair against text that no longer exists.
        // The only escape was withdrawing the claim and re-issuing it
        // under a fresh id, which erases the refutation from the rendered
        // ledger — the one trail the loop exists to preserve. An
        // undischargeable red that can only be cleared by destroying
        // evidence is worse than the one it replaced.
        //
        // A record with no digest at all predates the field. It is
        // treated as grading the current text rather than dropped:
        // dropping it would silently retire these checks for every
        // document written before digests existed, turning a live
        // contradiction into a clean report. Grandfathering can at worst
        // report a disagreement that a revision has since resolved —
        // which is the direction that fails loudly.
        let grading_current: Vec<&EvidenceRecord> = records
            .iter()
            .copied()
            .filter(|r| r.proposition_digest.is_empty() || r.proposition_digest == current)
            .collect();

        // Distinct from "ungrounded": at least one record grades the
        // current text, and every one that does derives to `Attested` for
        // standing purposes. Today that's unconditionally true — ingestion
        // is the only write path, and `derived_kind` never returns
        // anything else for an ingested record — but the check is written
        // against `derived_kind`, not against "has any evidence", so it
        // keeps working once a witnessed grounding can also land here.
        //
        // The emptiness guard is not redundant: a claim whose records all
        // grade superseded text has already been reported as a machine
        // failure by the partition above, and `all` over nothing is true,
        // so without it that claim would also be announced as grounded.
        if !grading_current.is_empty() && grading_current.iter().all(|r| r.derived_kind() == Kind::Attested) {
            attested_grounded.push((claim.id.clone(), claim.proposition.clone()));
        }

        // A contradiction is `supports` *and* `refutes` on one
        // proposition — P and not-P, decidable without a human, which is
        // what belongs in the machine partition.
        //
        // Two things this deliberately no longer does.
        //
        // It no longer treats `qualifies` as contradicting anything.
        // `qualifies` means "true under a condition the proposition does
        // not state" or "I could not establish this from what I was
        // given"; neither is inconsistent with another pass finding the
        // proposition supported. The author-vs-record comparison below
        // has always taken that view — only Supports/Refutes contradict —
        // and the record-vs-record loop disagreeing with it was the
        // inconsistency, not the doctrine. It also made an honest
        // multi-fact grounding unwritable in effect: `record --from-fact`
        // takes one fact per record, so a claim resting on several facts
        // accrues several same-pass records, and a grounder qualifying
        // one premise of a multi-premise argument reddened the memo for
        // being precise. Worse, the red was undischargeable — the
        // evidence log is append-only with no supersession, so nothing
        // the author could do afterwards would clear it.
        //
        // And it compares the whole set rather than adjacent pairs. The
        // old `windows(2)` loop compared (1,2) and (2,3) but never (1,3),
        // so records ordered `supports, qualifies, refutes` reported two
        // findings about the qualifies pairs and never mentioned the one
        // real contradiction. Order-dependent, and wrong in the direction
        // that hides the failure.
        //
        // Pass identity is deliberately not a condition. For an ingested
        // record `pass` is caller-typed free text, so resting a machine
        // verdict on passes differing would put a contradiction behind an
        // unverifiable field — and same-pass Supports+Refutes on one
        // proposition is a formal contradiction regardless of who wrote
        // both.
        let supporting: Vec<&&EvidenceRecord> =
            grading_current.iter().filter(|r| r.verdict == Verdict::Supports).collect();
        let refuting: Vec<&&EvidenceRecord> =
            grading_current.iter().filter(|r| r.verdict == Verdict::Refutes).collect();
        if !supporting.is_empty() && !refuting.is_empty() {
            let side = |rs: &[&&EvidenceRecord]| -> String {
                rs.iter()
                    .map(|r| {
                        format!(
                            "\n      pass {} => {} — {}",
                            r.pass,
                            r.verdict,
                            r.note.as_deref().unwrap_or("(no note)")
                        )
                    })
                    .collect()
            };
            disagreements.push(format!(
                "{} — {}{}{}",
                claim.id,
                claim.proposition,
                side(&supporting),
                side(&refuting),
            ));
        }

        // A qualified claim is human-owed, and until now it was silent.
        // On the first real grounding pass, the two claims the grounder
        // explicitly declined to confirm read as plainly "grounded", with
        // nothing anywhere in the report saying so — while the one claim
        // where it qualified a single premise of a multi-premise argument
        // was the one that reddened. The check fired on the wrong claim
        // and said nothing about the right ones.
        //
        // Digest-scoped for the same reason the machine half is: folding
        // a qualification into the claim's wording and re-grounding is
        // how an author *discharges* one, and a line that survived it
        // would leave the human partition accumulating exactly the
        // undischargeable residue the machine partition just shed. The
        // record itself still prints, under superseded evidence.
        for r in grading_current.iter().filter(|r| r.verdict == Verdict::Qualifies) {
            qualified.push((
                claim.id.clone(),
                r.pass.clone(),
                r.note.clone().unwrap_or_else(|| "(no note)".to_string()),
            ));
        }

        if let Some(author_verdict) = author_status_verdict(&claim.status) {
            for record in &grading_current {
                let contradicts = matches!(
                    (author_verdict, record.verdict),
                    (Verdict::Supports, Verdict::Refutes) | (Verdict::Refutes, Verdict::Supports)
                );
                if contradicts {
                    disagreements.push(format!(
                        "{} — {}\n      pass {} => {} — {}\n      pass {} => {} — {}",
                        claim.id,
                        claim.proposition,
                        "author (Status cell)",
                        author_verdict,
                        claim.status,
                        record.pass,
                        record.verdict,
                        record.note.as_deref().unwrap_or("(no note)"),
                    ));
                }
            }
        }
    }

    (ungrounded, attested_grounded, disagreements, qualified, out_of_proof, superseded)
}

/// One line per evidence record whose `source` designator is a path that
/// does not resolve on disk. A `proc:` source is never checked here — it
/// names a transcript, not a file, so there is nothing on disk to resolve
/// and its absence is not this check's business.
///
/// Non-failing, human-owed, mirroring `cited_undefined`'s disposition:
/// fabricating an attested fact should require fabricating a preserved
/// artifact, but a *missing* artifact is residue for a human to chase, not
/// grounds to redden the run — the same reasoning that keeps an undefined
/// citation out of the machine-checked partition.
///
/// Resolved against the current working directory — the same resolution a
/// path given on the command line gets. There is no existing convention in
/// this crate for resolving a designator against the memo's own directory
/// instead; `domain`/`extent` designators are never resolved against the
/// filesystem at all.
pub fn unresolved_evidence_sources(claims: &[Claim], evidence: &[EvidenceRecord]) -> Vec<String> {
    let propositions: HashMap<&str, &str> = claims.iter().map(|c| (c.id.as_str(), c.proposition.as_str())).collect();
    let mut out = Vec::new();
    for record in evidence {
        if let Some(Source::Path(path)) = Source::parse(&record.source) {
            if !Path::new(&path).exists() {
                let proposition = propositions.get(record.claim_id.as_str()).copied().unwrap_or("");
                out.push(format!(
                    "{}: evidence source `{}` does not resolve — {}",
                    record.claim_id, path, proposition
                ));
            }
        }
    }
    out
}

/// One line per fact whose extent contains a pre-dialect `NoMatch` or
/// `Search` record — see
/// [`crate::facts::ExtentEntry::pre_dialect_unescaped_ere_metachar`].
///
/// A `NoMatch` and a `Search` are different states, and the wording says
/// so rather than flattening both into "no matches": a `NoMatch` found
/// nothing (the original bounded-negative defect), a `Search` matched the
/// literal string and never asked what the pattern's own metacharacter(s)
/// would have meant under ERE (a real find that still under-asked the
/// question its own pattern looks like it asks). Saying "no-match" about
/// an extent that in fact matched something would be exactly the kind of
/// record-versus-reality gap this whole ticket exists to close.
///
/// Round-2 review, F3: this used to say "an unescaped `|`" specifically.
/// Generalised to the full flipped-metacharacter set (see
/// [`crate::facts::ERE_FLIPPED_METACHARS`]) rather than narrowed the
/// category wording instead — the second-model verifier reads these
/// labels regardless of which of the seven characters flipped, and the
/// wording below no longer names `|` specifically for the same reason.
///
/// Human-owed, not a refusal: the predicate is a byte property of the
/// pattern (an unescaped ERE metacharacter, decided the same way
/// `scripts/grep-dialect-census.py` decides it), so it never breaches the
/// format-level rule against refusing on a truth question — but the
/// record it points at is immutable and there is no singular remedy to
/// apply *to it*; only "re-run the search under this build" fixes what it
/// found (or didn't).
pub fn pre_dialect_no_matches(facts: &[crate::facts::Fact]) -> Vec<String> {
    use crate::pending::ObservationKind::{NoMatch, Search};
    let mut out = Vec::new();
    for f in facts {
        let hits: Vec<String> = f
            .extent
            .iter()
            .filter(|e| e.pre_dialect_unescaped_ere_metachar())
            .map(|e| match e.kind {
                Some(NoMatch) => format!(
                    "found nothing for `{}` — a bounded negative that may never have asked what \
its own regex metacharacter(s) suggest under ERE",
                    e.pattern
                ),
                Some(Search) => format!(
                    "matched `{}` on the literal string alone — never asked what its regex \
metacharacter(s) would have meant under ERE",
                    e.pattern
                ),
                _ => unreachable!(
                    "pre_dialect_unescaped_ere_metachar only admits Some(NoMatch) or Some(Search)"
                ),
            })
            .collect();
        if hits.is_empty() {
            continue;
        }
        out.push(format!(
            "{}: this fact's extent contains a pre-dialect record whose then-active grep read a \
regex metacharacter in its pattern literally, not as the ERE syntax it now is — {}; the remedy is \
to re-run it under this build",
            f.id,
            hits.join("; ")
        ));
    }
    out
}

/// Ledger claims minted with no declared scope at all (see
/// [`crate::ledger::NO_SCOPE_DECLARED`] and `compose::render`). Both cells
/// must match the sentinel exactly — an ordinary hand-written ledger row
/// whose Domain/Extent text happens to coincide is not this crate's
/// business to guess at, and `compose::render` always writes both cells
/// identically, so a real match never falls short of both.
/// Claims in a ledger that *has* scope columns but left them at the
/// no-scope sentinel.
///
/// Deliberately narrowed. This used to fire on every claim of every
/// tetel-authored memo, because `render` wrote the sentinel into two
/// columns unconditionally — eight identical lines on the first real
/// memo, saying the same structural fact about tetel rather than anything
/// about the document. A finding no author can ever discharge is not a
/// finding; it dilutes the human-owed list this design says should
/// *concentrate* reading.
///
/// A ledger with no scope columns at all now says so once, at document
/// level (see [`Findings::ledger_has_no_scope_columns`]). This function is
/// left for the case it was actually about: a table that offers the
/// columns and carries the sentinel anyway.
pub fn claims_without_declared_scope(claims: &[Claim]) -> Vec<(String, String)> {
    claims
        .iter()
        .filter(|c| {
            c.has_scope_columns
                && c.domain == crate::ledger::NO_SCOPE_DECLARED
                && c.extent == crate::ledger::NO_SCOPE_DECLARED
        })
        .map(|c| (c.id.clone(), c.proposition.clone()))
        .collect()
}

/// How each grounded claim's evidence stands, one line per claim.
///
/// # Why this is printed rather than checked
///
/// A grounding pass declares its own independence today: `pass` is a free
/// string in `record`'s JSON, validated only for being non-empty. So
/// "this claim was independently grounded" is an assertion nothing can
/// contradict, on a mechanism whose entire measured value (78% → 33%)
/// comes from independence being real.
///
/// A witnessed record fixes the checkable half. Its extent was copied
/// from a fact this tool captured, and it carries the identity of the
/// workspace that captured it — so whether a pass grounded claims in its
/// *own* observations is recomputable rather than claimed.
///
/// # What it still cannot establish, said out loud
///
/// That the grounding agent saw only the brief. Nothing in a record can
/// show that: it is a property of the sandbox the agent was handed, and
/// stays owed to a run protocol. This prints what it knows and names what
/// it does not, rather than letting a witnessed record read as a stronger
/// guarantee than it is.
/// What the snapshot beside a memo can say about who authored it.
///
/// The two negative cases are deliberately distinct. They were one
/// `None` and the report called both "no snapshot beside this memo",
/// which sent a reader looking for a missing directory that was in fact
/// present with six of its seven files — the message named a cause that
/// was not the cause. They also differ in what a reader should do: a
/// missing snapshot means the memo was never rendered by `render --out`,
/// while a snapshot without an identity means it was rendered by a build
/// that did not ship one, and cannot be repaired after the fact because
/// minting an identity now would date the pass wrongly.
pub enum AuthoringIdentity {
    /// No snapshot directory beside the memo at all.
    NoSnapshot,
    /// A snapshot exists but carries no readable `identity.json`.
    SnapshotWithoutIdentity,
    /// The identity of the workspace that authored the memo.
    Known(String),
}

/// The distinct non-author workspaces holding a **witnessed** grading of
/// the wording `claim` carries now.
///
/// Three restrictions, each load-bearing. **Witnessed only**, because an
/// ingested record's `pass` is a string its reporter typed, so a floor
/// counting those could be discharged by naming alone. **Current wording
/// only**, using the same in-proof predicate `analyze_ledger` applies —
/// empty digest grandfathered — so a claim graded before a revision does
/// not count as confirmation of what it says now. And **non-author**,
/// because a workspace grading its own memo is the author's own reading.
///
/// Returns `None` when authorship cannot be determined: with no snapshot,
/// or a snapshot carrying no identity, nothing can say whether a grading
/// workspace was the author's own, and a count taken anyway would score
/// self-grading as independent confirmation.
fn confirming_passes<'a>(
    claim: &Claim,
    evidence: &'a [EvidenceRecord],
    authoring_identity: &AuthoringIdentity,
) -> Option<Vec<&'a str>> {
    let AuthoringIdentity::Known(author) = authoring_identity else {
        return None;
    };
    // The same digest `analyze_ledger` takes, spelled the same way.
    let current = crate::evidence::sha256_hex(&claim.proposition);
    let mut passes: Vec<&str> = evidence
        .iter()
        .filter(|r| r.claim_id == claim.id && r.witnessed)
        .filter(|r| r.proposition_digest.is_empty() || r.proposition_digest == current)
        .map(|r| r.pass.as_str())
        .filter(|p| p != author)
        .collect();
    passes.sort_unstable();
    passes.dedup();
    Some(passes)
}

/// Which claims a grounding pass is being asked to grade.
///
/// **The whole predicate**: a claim is owed when fewer than `floor`
/// distinct non-author workspaces hold a witnessed grading of the wording
/// it carries now.
///
/// The two conditions that look like separate rules are theorems of this
/// one, not disjuncts beside it. A claim with no records at all, and a
/// claim whose every digested record graded superseded text, both have a
/// confirmation count of zero — below any floor of at least one. That is
/// why this ships without touching [`analyze_ledger`]: there is no set to
/// expose and no id list to add.
///
/// A floor of zero would leave nothing ever owed on any memo, which is the
/// empty section a switched-off flag would produce — so both front ends
/// reject it, and bounding the floor below is part of the decision not to
/// have a flag rather than input validation.
///
/// When authorship is undeterminable every claim stays owed. That errs
/// toward scheduling rather than silent discharge, and it is keyed on
/// whether a file is there and parses rather than on a guess about it.
pub fn owed_claims(
    claims: &[Claim],
    evidence: &[EvidenceRecord],
    authoring_identity: &AuthoringIdentity,
    floor: u32,
) -> Vec<String> {
    claims
        .iter()
        .filter(|claim| match confirming_passes(claim, evidence, authoring_identity) {
            None => true,
            Some(passes) => (passes.len() as u32) < floor,
        })
        .map(|c| c.id.clone())
        .collect()
}

pub fn grounding_provenance(
    claims: &[Claim],
    evidence: &[EvidenceRecord],
    authoring_identity: &AuthoringIdentity,
) -> Vec<String> {
    let mut out = Vec::new();
    for claim in claims {
        let records: Vec<&EvidenceRecord> =
            evidence.iter().filter(|e| e.claim_id == claim.id).collect();
        if records.is_empty() {
            continue;
        }
        let witnessed: Vec<&&EvidenceRecord> = records.iter().filter(|r| r.witnessed).collect();
        if witnessed.is_empty() {
            out.push(format!(
                "{}: grounded, all {} record(s) ingested — extent typed by the reporter, not captured by this tool; `pass` is whatever the reporter wrote",
                claim.id,
                records.len()
            ));
        } else {
            let passes: Vec<&str> = {
                let mut p: Vec<&str> = witnessed.iter().map(|r| r.pass.as_str()).collect();
                p.sort_unstable();
                p.dedup();
                p
            };
            // The distinction the whole mechanism exists for. A claim
            // grounded in the same workspace that authored it was graded
            // by the author against their own reading — the arrangement
            // measured at 78% scope-equal, no better than hand-authored
            // rows. Independence is what moved that to 33%. A check that
            // could not tell the two apart would be reporting the wrong
            // thing confidently, which is worse than reporting nothing.
            let standing = match authoring_identity {
                AuthoringIdentity::NoSnapshot => "no snapshot beside this memo, so whether the \
grounding workspace is also the authoring one cannot be determined from here"
                    .to_string(),
                AuthoringIdentity::SnapshotWithoutIdentity => "the snapshot beside this memo \
carries no identity, so whether the grounding workspace is also the authoring one cannot be \
determined from here — the memo was rendered by a build that did not ship one, and it cannot be \
added now without re-dating the pass"
                    .to_string(),
                AuthoringIdentity::Known(author) => {
                    // Counted separately rather than collapsed to a
                    // single verdict: a claim can carry both a
                    // self-grounded record and an independent one, and
                    // saying only "self-grounded" there would hide a real
                    // independent pass, while saying only "independently
                    // grounded" would hide that the author also graded
                    // their own work.
                    // Restricted to the wording the claim carries now, as
                    // the floor requires — a pass that graded superseded
                    // text confirms nothing about this text.
                    let current = crate::evidence::sha256_hex(&claim.proposition);
                    let now: Vec<&str> = {
                        let mut p: Vec<&str> = witnessed
                            .iter()
                            .filter(|r| r.proposition_digest.is_empty() || r.proposition_digest == current)
                            .map(|r| r.pass.as_str())
                            .collect();
                        p.sort_unstable();
                        p.dedup();
                        p
                    };
                    let by_author = now.iter().filter(|p| *p == author).count();
                    let by_others = now.len() - by_author;
                    match (by_author, by_others) {
                        // Restricting the input is only half the fix. Without
                        // this arm a claim nobody has graded on its current
                        // wording falls into `(0, _)` and reads
                        // "independently grounded" beside an empty list.
                        (0, 0) => "no workspace has graded the wording this claim carries now — \
earlier records, if any, graded text it no longer says"
                            .to_string(),
                        (0, _) => format!(
                            "independently grounded: no grounding workspace here is the one \
that authored this memo. {} distinct non-author workspace(s) have graded its current wording",
                            by_others
                        ),
                        (_, 0) => "SELF-GROUNDED: the only workspace that graded this claim is the \
one that authored the memo, so no independent pass has run on it. The author's own reading is the \
arrangement measured at 78% scope-equal; independence is what moved that to 33%"
                            .to_string(),
                        (a, o) => format!(
                            "MIXED: {a} grounding workspace(s) authored this memo and {o} did not \
— read the self-grounded record(s) as the author's own reading, not as independent confirmation. \
{o} distinct non-author workspace(s) have graded its current wording"
                        ),
                    }
                }
            };
            out.push(format!(
                "{}: grounded, {} of {} record(s) witnessed in workspace(s) {} — {}. That the \
grounding pass saw only the brief is not shown by any record and remains owed to the run protocol",
                claim.id,
                witnessed.len(),
                records.len(),
                passes.join(", "),
                standing
            ));
        }
    }
    out
}

/// Whether the memo's ledger is a tetel-authored one, whose format has no
/// scope columns for any claim to declare into.
pub fn ledger_has_no_scope_columns(claims: &[Claim]) -> bool {
    !claims.is_empty() && claims.iter().all(|c| !c.has_scope_columns)
}

#[cfg(test)]
mod tests {
    use crate::parse::parse_document;
    use crate::report::{render, EXIT_CHECK_FAILED, EXIT_CLEAN, EXIT_NO_ROWS};

    use super::*;

    fn check(source: &str) -> (i32, String, Findings) {
        let doc = parse_document(source);
        let ledger = crate::ledger::import(&doc.body);
        let findings = analyze(&doc, &ledger.claims);
        let (code, text) = render("test.md", &doc, &findings, "tetel test build");
        (code, text, findings)
    }

    fn a_claim(id: &str, proposition: &str) -> Claim {
        let body: Vec<String> = format!(
            "| ID | Proposition | Domain | Extent | Kind | Status |\n\
             |---|---|---|---|---|---|\n\
             | {id} | {proposition} | d | e | READING | **VERIFIED** |\n"
        )
        .lines()
        .map(str::to_string)
        .collect();
        crate::ledger::import(&body).claims.remove(0)
    }

    fn a_record(claim_id: &str, pass: &str, graded: &str, witnessed: bool) -> EvidenceRecord {
        EvidenceRecord {
            claim_id: claim_id.to_string(),
            verdict: Verdict::Supports,
            pass: pass.to_string(),
            reported_kind: "reading".to_string(),
            source: "proc:x".to_string(),
            extent: vec![],
            note: None,
            pin: None,
            timestamp: 0,
            witnessed,
            proposition_digest: crate::evidence::sha256_hex(graded),
        }
    }

    /// The two conditions the ticket proposed as separate rules — never
    /// graded, and graded against other words — are **theorems** of the
    /// floor, not disjuncts beside it. Both produce a confirmation count
    /// of zero, and zero is below any floor of at least one. If this test
    /// ever needs a second predicate to pass, the collapse has been undone.
    #[test]
    fn never_graded_and_graded_against_other_words_are_both_below_any_floor() {
        let author = AuthoringIdentity::Known("author-ws".to_string());
        let claim = a_claim("C1", "the current wording");

        // No records at all.
        assert_eq!(owed_claims(&[claim.clone()], &[], &author, 1), vec!["C1"]);

        // Graded, but against text the claim no longer carries.
        let stale = vec![a_record("C1", "k1", "some superseded wording", true)];
        assert_eq!(owed_claims(&[claim.clone()], &stale, &author, 1), vec!["C1"]);

        // Graded on the current wording by one non-author workspace.
        let fresh = vec![a_record("C1", "k1", "the current wording", true)];
        assert!(owed_claims(&[claim.clone()], &fresh, &author, 1).is_empty());

        // …and still owed at a floor of two, which is the whole point of
        // the parameter.
        assert_eq!(owed_claims(&[claim], &fresh, &author, 2), vec!["C1"]);
    }

    /// Two restrictions that would each let a claim discharge itself.
    #[test]
    fn self_grading_and_ingested_records_do_not_confirm() {
        let author = AuthoringIdentity::Known("author-ws".to_string());
        let claim = a_claim("C1", "the current wording");

        // The authoring workspace grading its own memo is the author's own
        // reading, not confirmation.
        let by_author = vec![a_record("C1", "author-ws", "the current wording", true)];
        assert_eq!(owed_claims(&[claim.clone()], &by_author, &author, 1), vec!["C1"]);

        // An ingested record's `pass` is a string its reporter typed, so a
        // floor counting those could be discharged by naming alone.
        let ingested = vec![a_record("C1", "k1", "the current wording", false)];
        assert_eq!(owed_claims(&[claim.clone()], &ingested, &author, 1), vec!["C1"]);

        // Distinct workspaces, not distinct records: two records from one
        // pass confirm once.
        let same_pass = vec![
            a_record("C1", "k1", "the current wording", true),
            a_record("C1", "k1", "the current wording", true),
        ];
        assert_eq!(owed_claims(&[claim], &same_pass, &author, 2), vec!["C1"]);
    }

    /// With no snapshot, or a snapshot carrying no identity, nothing can
    /// say whether a grading workspace was the author's own — so the rule
    /// schedules rather than discharging silently.
    #[test]
    fn undeterminable_authorship_leaves_every_claim_owed() {
        let claim = a_claim("C1", "the current wording");
        let graded = vec![a_record("C1", "k1", "the current wording", true)];
        for identity in [AuthoringIdentity::NoSnapshot, AuthoringIdentity::SnapshotWithoutIdentity] {
            assert_eq!(
                owed_claims(&[claim.clone()], &graded, &identity, 1),
                vec!["C1"],
                "a count taken here would score self-grading as independent confirmation"
            );
        }
    }

    #[test]
    fn no_tetel_blocks_is_a_distinct_exit_code() {
        let (code, out, _) = check("# just prose\n\nno rows here.\n");
        assert_eq!(code, EXIT_NO_ROWS);
        assert_ne!(code, EXIT_CLEAN);
        assert!(out.contains("no tetel rows found"));
    }

    #[test]
    fn duplicate_id_is_a_grammar_error() {
        let source = "\
```tetel
id: X-1
claim: First claim.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: READING
status: VERIFIED
```

```tetel
id: X-1
claim: Second claim reusing the same id.
domain: b.rs#g
extent: b.rs#g
pin: p1
kind: READING
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(code, EXIT_CHECK_FAILED);
        assert!(findings.grammar_errors.iter().any(|e| e.contains("duplicate id")));
    }

    #[test]
    fn value_forbidden_on_reading_is_a_grammar_error() {
        let source = "\
```tetel
id: X-2
claim: A reading row that wrongly carries a value.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: READING
value: 3
status: VERIFIED
```
";
        let (_, _, findings) = check(source);
        assert!(findings
            .grammar_errors
            .iter()
            .any(|e| e.contains("forbidden") && e.contains("value")));
    }

    #[test]
    fn stance_marker_on_verified_row_is_a_grammar_error() {
        let source = "\
Cited as refuted even though it is verified [!X-3].

```tetel
id: X-3
claim: A verified claim wrongly cited as refuted.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: READING
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(code, EXIT_CHECK_FAILED);
        assert!(findings
            .grammar_errors
            .iter()
            .any(|e| e.contains("VERIFIED") && e.contains("refuted")));
    }

    #[test]
    fn proc_designator_skips_subset_check_and_lands_in_coverage_skipped() {
        let source = "\
```tetel
id: X-4
claim: A proc-shaped row whose coverage is not machine-checked.
domain: proc: grep -rn Foo src
extent: proc: grep -rn Foo src
pin: p1
kind: READING
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(code, EXIT_CLEAN, "proc: rows must not fail the subset check");
        assert!(findings.subset_failures.is_empty());
        assert_eq!(findings.coverage_skipped.len(), 1);
        assert_eq!(findings.coverage_skipped[0].0, "X-4");
    }

    #[test]
    fn cited_but_undefined_id_is_reported_without_failing() {
        let source = "\
Refers to a row that was never defined [GHOST-1].

```tetel
id: X-5
claim: An unrelated defined row.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: READING
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(code, EXIT_CLEAN, "an undefined citation must not fail the run");
        assert_eq!(findings.cited_undefined, vec!["GHOST-1".to_string()]);
    }

    #[test]
    fn defined_but_uncited_id_is_reported_without_failing() {
        let source = "\
```tetel
id: X-6
claim: A row nobody ever cites.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: READING
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(code, EXIT_CLEAN);
        assert_eq!(findings.defined_uncited.len(), 1);
        assert_eq!(findings.defined_uncited[0].0, "X-6");
    }

    #[test]
    fn non_abutting_literal_is_a_candidate_never_a_failure() {
        let source = "\
The gateway answers at 29s, or thereabouts, according to [X-7].

```tetel
id: X-7
claim: The gateway's health endpoint responds within its SLA.
domain: a.rs#f
extent: a.rs#f
pin: p1
kind: OBSERVED
run: curl
value: 31s
status: VERIFIED
```
";
        let (code, _, findings) = check(source);
        assert_eq!(
            code, EXIT_CLEAN,
            "a non-abutting literal must never fail, even when it disagrees with the value"
        );
        assert!(findings.abutting_failures.is_empty());
        assert_eq!(findings.abutting_candidates.len(), 1);
    }

    // --- prose_after_proof ------------------------------------------

    fn a_record_at(claim_id: &str, pass: &str, graded: &str, ts: u64) -> EvidenceRecord {
        EvidenceRecord {
            claim_id: claim_id.to_string(),
            verdict: Verdict::Supports,
            pass: pass.to_string(),
            reported_kind: "reading".to_string(),
            source: "proc:x".to_string(),
            extent: vec![],
            note: None,
            pin: None,
            timestamp: ts,
            witnessed: false,
            proposition_digest: crate::evidence::sha256_hex(graded),
        }
    }

    /// Same as `a_record_at`, but with a caller-chosen verdict — for the
    /// tests pinning that a refuting record cannot serve as first proof.
    fn a_record_at_verdict(claim_id: &str, pass: &str, graded: &str, ts: u64, verdict: Verdict) -> EvidenceRecord {
        EvidenceRecord { verdict, ..a_record_at(claim_id, pass, graded, ts) }
    }

    fn create_ev(id: &str, text: &str, cite: &[&str], ts: u64) -> ProseEvent {
        ProseEvent::Create {
            id: id.to_string(),
            heading: false,
            level: None,
            text: text.to_string(),
            cite: cite.iter().map(|s| s.to_string()).collect(),
            before: None,
            timestamp: ts,
        }
    }

    fn revise_ev(id: &str, text: &str, cite: Option<&[&str]>, ts: u64) -> ProseEvent {
        revise_ev_why(id, text, "test revision", cite, ts)
    }

    /// Same as `revise_ev`, but with a caller-chosen `why` — for the test
    /// pinning that `prose_after_proof` carries it through.
    fn revise_ev_why(id: &str, text: &str, why: &str, cite: Option<&[&str]>, ts: u64) -> ProseEvent {
        ProseEvent::Revise {
            id: id.to_string(),
            text: text.to_string(),
            why: why.to_string(),
            cite: cite.map(|c| c.iter().map(|s| s.to_string()).collect()),
            timestamp: ts,
        }
    }

    /// The case creation-counting exists to catch (C5 in the design
    /// memo): a paragraph written under nothing but an already-settled
    /// claim. No revision anywhere — the block's one and only event
    /// postdates the claim's first proof.
    #[test]
    fn lists_a_paragraph_created_after_its_only_claim_settled() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 100)];
        let prose = vec![create_ev("P1", "A paragraph about C1.", &["C1"], 200)];

        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        assert_eq!(
            out.len(),
            1,
            "expected exactly one listed block, got {:?}",
            out.iter().map(|o| o.block_id.clone()).collect::<Vec<_>>()
        );
        assert_eq!(out[0].block_id, "P1");
        assert_eq!(out[0].block_timestamp, 200);
        assert_eq!(out[0].cited, vec![("C1".to_string(), 100, "k1".to_string())]);
    }

    /// The load-bearing refutation this design records under "The pair
    /// gate": keying a block's timestamp on text alone lets an author
    /// leave the text untouched and repoint the citation list at a claim
    /// the fix round had just settled — silencing a listing with an act
    /// indistinguishable from good authoring. Keying on text *and*
    /// citations together closes it, because changing either moves the
    /// timestamp with it.
    ///
    /// This is the test named in the task's revert-check: green with the
    /// pair-gate `t == cur_text && c == cur_cite` filter in place, red
    /// if that filter is narrowed back to `t == cur_text` alone.
    #[test]
    fn citation_only_revision_is_listed_not_silenced() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 100)];
        let prose = vec![
            // Created uncited, well before C1 ever settled.
            create_ev("P1", "Body text that never changes.", &[], 10),
            // Later: citations added, text left byte-identical. An
            // ordinary, unrefused act — see prose.rs's `revise` doc
            // comment on why `cite` exists.
            revise_ev("P1", "Body text that never changes.", Some(&["C1"]), 150),
        ];

        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        assert_eq!(
            out.len(),
            1,
            "a citation-only revision that points at an already-settled claim must be listed, \
not silenced"
        );
        assert_eq!(out[0].block_timestamp, 150, "keyed on the pair, not on text alone");
    }

    /// Requiring every cited claim to be in proof let a single
    /// record-less citation suppress a listing with nothing red to show
    /// for it. The gate is existential: one in-proof citation anchors
    /// the block, and the record-less one contributes nothing (and is
    /// omitted from `cited`).
    #[test]
    fn existential_gate_ignores_a_record_less_citation() {
        let claims = vec![a_claim("C1", "the wording"), a_claim("C2", "an ungrounded claim")];
        let evidence = vec![a_record_at("C1", "k1", "the wording", 50)];
        let prose = vec![create_ev("P1", "Rests on both C1 and C2.", &["C1", "C2"], 100)];

        let out = prose_after_proof(&prose, &claims, &evidence, &[], &[], None).0;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cited, vec![("C1".to_string(), 50, "k1".to_string())], "C2 has no first proof and must not appear");
    }

    // --- TET-61: acknowledgement -------------------------------------

    fn an_ack_claim(id: &str, prop: &str) -> AckClaim {
        AckClaim { id: id.to_string(), prop: prop.to_string(), from: vec![], withdrawn: false, revisions: 0 }
    }

    fn an_ack(block: &str, text: &str, cite: &[&str], digests: &[&str], identity: &str, why: &str, ts: u64) -> AckEvent {
        AckEvent {
            block: block.to_string(),
            text: text.to_string(),
            cite: cite.iter().map(|s| s.to_string()).collect(),
            digests: digests.iter().map(|s| s.to_string()).collect(),
            identity: identity.to_string(),
            why: why.to_string(),
            timestamp: ts,
        }
    }

    /// The baseline positive case: an ack whose block, text, citations,
    /// digests and identity all equal what the check recomputes moves the
    /// entry from the listed partition to the acknowledged one, and the
    /// acknowledged entry carries the block's cited ids and the ack's own
    /// verbatim reason.
    #[test]
    fn a_matching_ack_discharges_the_listing() {
        let claim = a_claim("C1", "the wording");
        let ack_claim = an_ack_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 100)];
        let prose = vec![create_ev("P1", "A paragraph about C1.", &["C1"], 200)];
        let digest = crate::evidence::sha256_hex("the wording");
        let acks = vec![an_ack("P1", "A paragraph about C1.", &["C1"], &[&digest], "ws-1", "re-read, still accurate", 250)];

        let (listed, acknowledged) =
            prose_after_proof(&prose, &[claim], &evidence, &acks, &[ack_claim], Some("ws-1"));
        assert!(listed.is_empty(), "a matching ack must clear the listed partition: {:?}", listed.iter().map(|l| l.block_id.clone()).collect::<Vec<_>>());
        assert_eq!(acknowledged.len(), 1);
        assert_eq!(acknowledged[0].block_id, "P1");
        assert_eq!(acknowledged[0].cited, vec!["C1".to_string()]);
        assert_eq!(acknowledged[0].why, "re-read, still accurate");
        assert_eq!(acknowledged[0].timestamp, 250);
    }

    /// An ack whose minting identity does not match the snapshot's own is
    /// void rather than honoured — see C4/C8 in the design memo: nothing
    /// binds a rendered memo to the workspace that produced it, so a
    /// stale ack log copied in from elsewhere must not suppress.
    #[test]
    fn an_ack_from_a_different_identity_does_not_suppress() {
        let claim = a_claim("C1", "the wording");
        let ack_claim = an_ack_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 100)];
        let prose = vec![create_ev("P1", "A paragraph about C1.", &["C1"], 200)];
        let digest = crate::evidence::sha256_hex("the wording");
        let acks = vec![an_ack("P1", "A paragraph about C1.", &["C1"], &[&digest], "someone-elses-workspace", "re-read, fine", 250)];

        let (listed, acknowledged) =
            prose_after_proof(&prose, &[claim], &evidence, &acks, &[ack_claim], Some("this-snapshots-identity"));
        assert_eq!(listed.len(), 1, "identity mismatch must not suppress");
        assert!(acknowledged.is_empty());
    }

    /// Invariant (i) from the design memo's C10: the ack's key includes
    /// the block's citation list, not just its text, so a later
    /// citation-only revision voids an ack minted against the earlier
    /// citation set — even when the two claims happen to share the exact
    /// same proposition text (and therefore the same digest), which is
    /// what makes this test able to catch a predicate weakened to ignore
    /// `cite` while still checking `digests`: C1 and C2 are given
    /// identical propositions on purpose, so a digest-only comparison
    /// cannot tell the citation swap apart from no change at all.
    ///
    /// Mutation to redden this test: drop `a.cite == *cur_cite` from the
    /// match predicate in `prose_after_proof`.
    #[test]
    fn ack_keyed_on_text_alone_would_survive_a_citation_only_revision() {
        let claims = vec![a_claim("C1", "shared text"), a_claim("C2", "shared text")];
        let ack_claims = vec![an_ack_claim("C1", "shared text"), an_ack_claim("C2", "shared text")];
        let evidence = vec![a_record_at("C1", "k1", "shared text", 5), a_record_at("C2", "k2", "shared text", 15)];
        let digest = crate::evidence::sha256_hex("shared text");
        let prose = vec![
            create_ev("P1", "Body.", &["C1"], 10),
            // The ack matches this original (text, cite=[C1]) pair.
            revise_ev_why("P1", "Body.", "repoint citation", Some(&["C2"]), 20),
        ];
        let acks = vec![an_ack("P1", "Body.", &["C1"], &[&digest], "ws-1", "read against C1, fine", 11)];

        let (listed, acknowledged) =
            prose_after_proof(&prose, &claims, &evidence, &acks, &ack_claims, Some("ws-1"));
        assert_eq!(
            listed.len(),
            1,
            "a citation-only revision must void the old ack and stay listed, not be silently \
suppressed by an ack keyed on text alone: acknowledged = {:?}",
            acknowledged.iter().map(|a| a.block_id.clone()).collect::<Vec<_>>()
        );
        assert!(acknowledged.is_empty());
    }

    /// Invariant (ii) from the design memo's C10: the ack's key includes
    /// a digest per cited claim, so a later rewrite of a cited claim's
    /// proposition voids an ack minted against the earlier wording — even
    /// though the block's own text and citation *list* (the id `C1`) are
    /// unchanged. `claims` (the ledger list the trigger's own anchor
    /// calculation reads) is held constant across both sides so only the
    /// digest recomputation moves — isolating exactly the property this
    /// invariant is about.
    ///
    /// Mutation to redden this test: drop `a.digests == *digests` from
    /// the match predicate in `prose_after_proof`.
    #[test]
    fn ack_keyed_without_digests_would_survive_a_rewrite_of_the_cited_claim() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 100)];
        let prose = vec![create_ev("P1", "A paragraph about C1.", &["C1"], 200)];
        let old_digest = crate::evidence::sha256_hex("the wording");
        let acks = vec![an_ack("P1", "A paragraph about C1.", &["C1"], &[&old_digest], "ws-1", "read against the old wording", 250)];
        // claims.jsonl's own C1 has since been rewritten to a different
        // proposition — the ack's digest was taken over the old wording.
        let ack_claims = vec![an_ack_claim("C1", "a completely different wording")];

        let (listed, acknowledged) =
            prose_after_proof(&prose, &[claim], &evidence, &acks, &ack_claims, Some("ws-1"));
        assert_eq!(
            listed.len(),
            1,
            "a rewritten cited claim must void the old ack and stay listed, not be silently \
suppressed by an ack keyed on text and citations alone: acknowledged = {:?}",
            acknowledged.iter().map(|a| a.block_id.clone()).collect::<Vec<_>>()
        );
        assert!(acknowledged.is_empty());
    }

    /// No evidence ledger at all: no cited claim is in proof, so there is
    /// no first proof to compare against and nothing is listed — the
    /// vacuous case falls out of the in-proof test without a special rule.
    #[test]
    fn nothing_listed_with_no_evidence() {
        let claim = a_claim("C1", "the wording");
        let prose = vec![create_ev("P1", "Rests on C1, ungrounded.", &["C1"], 100)];
        assert!(prose_after_proof(&prose, &[claim], &[], &[], &[], None).0.is_empty());
    }

    /// A paragraph that cites nothing is never listed. Note this guard
    /// (`if cur_cite.is_empty() { continue }`) is not independently
    /// observable by mutation: an empty `cur_cite` always produces an
    /// empty `cited` from the existential-gate `filter_map` two lines
    /// later, so the downstream `if cited.is_empty() { continue }` check
    /// already skips this case on its own. Removing the early guard
    /// changes no test outcome — it is kept for readability (naming the
    /// population the memo names explicitly, "paragraphs that cite
    /// nothing by choice") rather than as a load-bearing gate of its own.
    #[test]
    fn uncited_paragraph_is_never_listed() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 10)];
        let prose = vec![create_ev("P2", "Cites nothing at all.", &[], 999)];
        assert!(prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0.is_empty());
    }

    /// Headings are excluded by their own guard (`if h.heading {
    /// continue }`), and — unlike the uncited-paragraph guard above —
    /// this one is load-bearing: the raw log format does not forbid a
    /// heading event from carrying citations (only the CLI path never
    /// produces one), so a heading with a citation to an in-proof claim
    /// must still be skipped, not merely happen to fall through some
    /// other check. Constructed directly against the event log (bypassing
    /// `prose::create`, which never lets the CLI attach citations to a
    /// heading) so this guard is exercised on its own.
    #[test]
    fn heading_with_citations_is_still_never_listed() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 10)];
        let heading_with_cite = ProseEvent::Create {
            id: "P1".to_string(),
            heading: true,
            level: Some(2),
            text: "A section heading".to_string(),
            cite: vec!["C1".to_string()],
            before: None,
            timestamp: 999,
        };
        assert!(prose_after_proof(&[heading_with_cite], &[claim], &evidence, &[], &[], None).0.is_empty());
    }

    /// The first refutation this design records: `prose.jsonl` is
    /// append-only, so a byte-exact revert appends a strictly later
    /// timestamp. Keying the block's timestamp on the *earliest* event
    /// whose content matches the current content — rather than on the
    /// last event — is what lets the revert actually clear the listing.
    #[test]
    fn a_byte_exact_revert_clears_the_listing() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 100)];
        let prose = vec![
            create_ev("P1", "Original wording.", &["C1"], 10), // before C1 settled
            revise_ev("P1", "Fix-round rewrite.", None, 150),  // after — would be listed
            revise_ev("P1", "Original wording.", None, 300),   // byte-exact revert
        ];
        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        assert!(
            out.is_empty(),
            "a revert to the pre-settlement wording must clear the listing, block timestamps: {:?}",
            out.iter().map(|o| o.block_timestamp).collect::<Vec<_>>()
        );
    }

    /// Pins the per-block quantifier: the anchor is the *latest* first
    /// proof among a block's in-proof citations, not the earliest. C1
    /// settles at t=100, C2 at t=200; the block is written at t=150 —
    /// after C1 settled but before C2 did. Under the correct `max`
    /// anchor (200) this must NOT be listed: the block is not anchored
    /// until *every* claim it leans on has settled, and C2 has not yet.
    /// Under a `min` anchor (100) it would wrongly be listed, since
    /// t=150 > 100. This is the case a coordinator's mutation run found
    /// unpinned: with a single citation, `min` and `max` over a
    /// one-element set are extensionally equal, so every other fixture
    /// in this module passes under both.
    #[test]
    fn anchor_is_the_latest_first_proof_not_the_earliest() {
        let claims = vec![a_claim("C1", "wording one"), a_claim("C2", "wording two")];
        let evidence =
            vec![a_record_at("C1", "k1", "wording one", 100), a_record_at("C2", "k2", "wording two", 200)];
        let prose = vec![create_ev("P1", "Rests on both C1 and C2.", &["C1", "C2"], 150)];

        let out = prose_after_proof(&prose, &claims, &evidence, &[], &[], None).0;
        assert!(
            out.is_empty(),
            "the block is not anchored until every in-proof citation has settled — C2 settles \
after this block was written, so this must not be listed: {:?}",
            out.iter().map(|o| o.block_timestamp).collect::<Vec<_>>()
        );
    }

    /// Pins the per-claim quantifier: a claim's first proof is the
    /// *earliest* in-proof record, not the latest. C1 carries two
    /// records grading its current wording — one at t=200 (pass "kA"),
    /// one at t=50 (pass "kB"). The block is written at t=100, strictly
    /// after the earliest (50) but strictly before the later one (200).
    /// Under the correct `min` this is listed, anchored at 50 by "kB".
    /// Under a `max` it would not be (100 is not > 200).
    #[test]
    fn per_claim_first_proof_is_the_earliest_record_not_the_latest() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![
            a_record_at("C1", "kA", "the wording", 200),
            a_record_at("C1", "kB", "the wording", 50),
        ];
        let prose = vec![create_ev("P1", "Rests on C1.", &["C1"], 100)];

        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        assert_eq!(out.len(), 1, "written after the claim's earliest in-proof record, must be listed");
        assert_eq!(
            out[0].cited,
            vec![("C1".to_string(), 50, "kB".to_string())],
            "the earliest in-proof record anchors the claim, not the latest"
        );
    }

    /// Round-2 code review (A), consequence 1: a claim graded only by a
    /// refutation must not print as "first entered proof" — that is a
    /// false statement about a claim that was rejected, not confirmed.
    /// Skipping `Verdict::Refutes` records means C1 has no first proof at
    /// all here (the same non-finding an ungrounded claim gets), so a
    /// block resting only on C1 is not listed.
    ///
    /// Mutation-run: with the `r.verdict == Verdict::Refutes { continue }`
    /// guard removed (reverting to the pre-fix, verdict-blind filter),
    /// this test fails — `out.len()` is 1, and `out[0].cited` carries
    /// `("C1", 100, "k1")`, printing exactly the false "entered proof"
    /// claim this test exists to catch. With the guard restored it
    /// passes. See the fix report for the pasted `cargo test` output of
    /// both runs.
    #[test]
    fn a_refuting_record_does_not_count_as_first_proof() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at_verdict("C1", "k1", "the wording", 100, Verdict::Refutes)];
        let prose = vec![create_ev("P1", "Rests only on C1.", &["C1"], 200)];

        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        assert!(
            out.is_empty(),
            "a claim graded only by a refutation must have no first proof to anchor on \
(nothing here confirms the wording), got {:?}",
            out.iter().map(|o| (o.block_id.clone(), o.cited.clone())).collect::<Vec<_>>()
        );
    }

    /// Round-2 code review (A), consequence 2: a *later* refutation of one
    /// cited claim must not raise the block's `max` anchor and silence a
    /// listing that rests on another claim's real (supported) proof. C1
    /// is supported at t=100; C2 is refuted at t=500 — later than the
    /// paragraph's own t=300. Under the pre-fix, verdict-blind filter, C2
    /// would still contribute a first-proof entry at 500, the `max`
    /// anchor would be 500, and 300 > 500 is false — the block would be
    /// silenced, even though it rests on C1's genuine proof from t=100 and
    /// was written squarely after it. With the fix, C2 contributes
    /// nothing (refuted, no first proof), the anchor is C1's 100 alone,
    /// and 300 > 100 — listed, anchored only on the claim that actually
    /// entered proof.
    ///
    /// Mutation-run: with the `Verdict::Refutes` guard removed, this test
    /// fails (`out` is empty — silenced). With the guard restored it
    /// passes (`out.len() == 1`, anchored only on C1). See the fix report
    /// for the pasted `cargo test` output of both runs.
    #[test]
    fn a_later_refutation_does_not_silence_a_block_resting_on_real_proof() {
        let claims = vec![a_claim("C1", "wording one"), a_claim("C2", "wording two")];
        let evidence = vec![
            a_record_at("C1", "k1", "wording one", 100),
            a_record_at_verdict("C2", "k2", "wording two", 500, Verdict::Refutes),
        ];
        let prose = vec![create_ev("P1", "Rests on both C1 and C2.", &["C1", "C2"], 300)];

        let out = prose_after_proof(&prose, &claims, &evidence, &[], &[], None).0;
        assert_eq!(
            out.len(),
            1,
            "C2's later refutation must not raise the anchor and silence a block resting on \
C1's real proof, got {:?}",
            out.iter().map(|o| (o.block_id.clone(), o.cited.clone())).collect::<Vec<_>>()
        );
        assert_eq!(
            out[0].cited,
            vec![("C1".to_string(), 100, "k1".to_string())],
            "C2 must contribute nothing to the anchor — it was only ever refuted"
        );
    }

    /// Round-2 code review (B): a legacy empty-digest record must not be
    /// read as a timestamp for "this wording settled" — it predates the
    /// digest field and does not attest to which wording it graded. C1's
    /// current wording is graded for the first time at t=600; a
    /// pre-digest record for the *same claim id* (necessarily some
    /// earlier, unknown wording) sits at t=10. A paragraph written at
    /// t=550 — before the current wording was ever actually graded — must
    /// not be listed, even though 550 > 10.
    ///
    /// Verified this reproduces before fixing it: on the pre-fix code
    /// (empty digest grandfathered into the `min`), this test fails —
    /// `out` carries one entry anchored at t=10 via the legacy record,
    /// falsely flagging a paragraph that in fact predates the only real
    /// grading of its citation. See the fix report for the pasted
    /// `cargo test` output of both runs.
    #[test]
    fn empty_digest_grandfathering_does_not_anchor_before_current_wording_was_ever_graded() {
        let claim = a_claim("C1", "the wording, v2");
        let evidence = vec![
            // Pre-digest record: some earlier wording, timestamp unrelated
            // to when v2 was graded.
            EvidenceRecord {
                proposition_digest: String::new(),
                ..a_record_at("C1", "legacy", "the wording, v1", 10)
            },
            // The only record that actually grades the current wording.
            a_record_at("C1", "k2", "the wording, v2", 600),
        ];
        let prose = vec![create_ev(
            "P1",
            "Written before the current wording was ever graded.",
            &["C1"],
            550,
        )];

        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        assert!(
            out.is_empty(),
            "the block (t=550) predates the only record that actually grades the current \
wording (t=600); anchoring at the legacy empty-digest record's t=10 would falsely list it, \
got {:?}",
            out.iter().map(|o| (o.block_timestamp, o.cited.clone())).collect::<Vec<_>>()
        );
    }

    /// Pins the strict inequality at the final comparison. A block
    /// written in the *same second* as its only citation's first proof
    /// is not listed — the rule is "strictly later," not "at or after."
    /// A `>=` mutation would list this.
    #[test]
    fn equal_timestamps_are_not_listed() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 100)];
        let prose = vec![create_ev("P1", "Rests on C1.", &["C1"], 100)];

        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        assert!(
            out.is_empty(),
            "a block timestamped exactly at its anchor must not be listed, got {} entries",
            out.len()
        );
    }

    /// Pins the cite-carried-forward behaviour on a `Revise` that leaves
    /// `cite` unstated (`cite: None`): the block keeps its previous
    /// citation list rather than losing it. P1 is created citing C1
    /// before C1 settles, then rewritten (text only, `cite: None`) after
    /// C1's first proof. If the carry-forward silently dropped to
    /// empty instead, the block would fail the cites-something gate and
    /// vanish from the list with no citation ever having been removed by
    /// any author action — the exact silent-drop failure mode this
    /// design's existential/pair-gate work was built to avoid elsewhere.
    #[test]
    fn revise_without_cite_carries_the_previous_citation_list_forward() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 100)];
        let prose = vec![
            create_ev("P1", "Original wording.", &["C1"], 10), // before C1 settled
            revise_ev("P1", "Rewritten wording.", None, 150),  // text only; cite carried forward
        ];

        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        assert_eq!(
            out.len(),
            1,
            "a text-only revision must keep citing C1 via carry-forward, and must be listed \
since it postdates C1's settlement"
        );
        assert_eq!(out[0].cited, vec![("C1".to_string(), 100, "k1".to_string())]);
    }

    /// Round-2 code review (C): `ProseEvent::Revise` carries `why` — the
    /// author's own stated reason for the edit — and a listed block whose
    /// current version came from a `Revise` must carry it through rather
    /// than discard it.
    #[test]
    fn a_listed_block_carries_the_revision_s_why() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 100)];
        let prose = vec![
            create_ev("P1", "Original wording.", &["C1"], 10), // before C1 settled
            revise_ev_why("P1", "Rewritten wording.", "tightened the claim after review", None, 150),
        ];

        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].why.as_deref(), Some("tightened the claim after review"));
    }

    /// The other half of the same fix: a block listed on its *first*
    /// version — a `Create`, never revised — has no `why` to carry, and
    /// this must come back as `None`, not a fabricated empty string.
    #[test]
    fn a_block_listed_on_its_create_has_no_why() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 100)];
        let prose = vec![create_ev("P1", "Never revised.", &["C1"], 200)];

        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].why, None, "a Create has no why; the field must not be fabricated");
    }

    /// A repeated `Create` for one block id — reachable via a
    /// hand-edited log, and via `workspace::next_id`'s unlocked
    /// read-modify-write of `counters.json` under concurrent `tetel
    /// prose` invocations — must be reported at most once, matching
    /// `prose::load_all`'s own dedup (single entry, last Create's
    /// content wins). Without the `blocks.contains_key` guard, the same
    /// merged block was emitted once per `Create` event for that id.
    #[test]
    fn a_duplicate_create_id_is_reported_at_most_once() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 100)];
        let prose = vec![
            create_ev("P1", "First create.", &["C1"], 10),
            create_ev("P1", "Second create (duplicate id).", &["C1"], 200),
        ];
        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        assert_eq!(
            out.len(),
            1,
            "a duplicate Create id must be reported at most once, got {:?}",
            out.iter().map(|o| o.block_id.clone()).collect::<Vec<_>>()
        );
        assert_eq!(
            out[0].block_timestamp, 200,
            "load_all-consistent: the last Create's content is what survives, not the first"
        );
    }

    /// Entries must follow *document* order, honouring `--before`, not
    /// the order blocks were authored in. P1 is created first; P2 is
    /// created second but inserted before P1 — so the document (and
    /// `load_all`) puts P2 first. Both cite C1 and are written after C1
    /// settles, so both are listed; the order they come back in must be
    /// [P2, P1], not the authoring order [P1, P2].
    #[test]
    fn entries_follow_document_order_not_authoring_order() {
        let claim = a_claim("C1", "the wording");
        let evidence = vec![a_record_at("C1", "k1", "the wording", 10)];
        let prose = vec![
            create_ev("P1", "Written first, belongs second.", &["C1"], 100),
            ProseEvent::Create {
                id: "P2".to_string(),
                heading: false,
                level: None,
                text: "Written second, belongs first.".to_string(),
                cite: vec!["C1".to_string()],
                before: Some("P1".to_string()),
                timestamp: 100,
            },
        ];
        let out = prose_after_proof(&prose, &[claim], &evidence, &[], &[], None).0;
        let ids: Vec<&str> = out.iter().map(|o| o.block_id.as_str()).collect();
        assert_eq!(ids, vec!["P2", "P1"], "must follow document order (P2 before P1), not authoring order");
    }
}

/// Re-verifies a rendered document's modification targets against the
/// snapshot shipped beside it, in both directions.
///
/// `tetel target` refuses an uncensused declaration at authoring time, so
/// a memo this crate wrote cannot fail this. What it catches is a
/// document that never passed through that verb — hand-authored, or
/// edited after rendering. Tampering can go either way, so both are
/// checked: a target row the snapshot does not have, and a snapshot
/// target whose cited fact does not census it.
///
/// Reading the document's rows rather than trusting the snapshot alone is
/// the point. A reviewer sees the rendered table; if the table can say
/// something the record does not support, the table is the lie that
/// matters.
pub fn census_findings(
    doc_body: &[String],
    snapshot_targets: &[crate::targets::Target],
    snapshot_facts: &[crate::facts::Fact],
) -> Vec<String> {
    let mut out = Vec::new();
    let live: Vec<&crate::targets::Target> = snapshot_targets.iter().filter(|t| !t.withdrawn).collect();

    // Direction 1: every live target the snapshot carries must still be
    // censused by the fact it cites.
    for t in &live {
        match snapshot_facts.iter().find(|f| f.id == t.from) {
            None => out.push(format!(
                "{}: declares `{}` censused by {}, but no such fact is in the snapshot",
                t.id, t.symbol, t.from
            )),
            Some(f) if !f.extent.iter().any(|e| e.censuses(&t.symbol)) => out.push(format!(
                "{}: declares `{}` censused by {}, but that fact's captured extent contains no whole-worktree search for `{}`",
                t.id, t.symbol, t.from, t.symbol
            )),
            Some(_) => {}
        }
    }

    // Direction 2: every target row the *document* renders must be one of
    // those. A row invented in the file is the tampering case a snapshot
    // exists to catch.
    for symbol in rendered_target_symbols(doc_body) {
        if !live.iter().any(|t| t.symbol == symbol) {
            out.push(format!(
                "the document declares `{symbol}` a modification target, but its snapshot has no such target — the row was not written by `tetel target`"
            ));
        }
    }
    out
}

/// Re-verify the premise inventory against the shipped snapshot, both
/// directions — the same tampering-goes-both-ways discipline
/// [`census_findings`] applies.
///
/// Nothing here re-reads the donor's source. The premise is compared
/// against the captured bytes `facts.jsonl` already carries, so this runs
/// no command and opens nothing the document names.
pub fn premise_findings(
    doc_body: &[String],
    snapshot_transplants: &[crate::transplants::Transplant],
    snapshot_facts: &[crate::facts::Fact],
    snapshot_claims: &[crate::claims::Claim],
) -> Vec<String> {
    let mut out = Vec::new();
    let live: Vec<&crate::transplants::Transplant> =
        snapshot_transplants.iter().filter(|t| !t.withdrawn).collect();

    // Direction 1: every premise the snapshot carries is still the
    // donor's words, and still answered.
    for t in &live {
        let donor = snapshot_facts.iter().find(|f| f.id == t.from);
        for p in t.live_premises() {
            match donor {
                None => out.push(format!(
                    "{}: quotes donor fact {}, but no such fact is in the snapshot",
                    p.id, t.from
                )),
                Some(f) if !f.quotes(&p.text) => out.push(format!(
                    "{}: its text is not in {}'s captured output — the premise is not the donor's words",
                    p.id, t.from
                )),
                Some(_) => {}
            }
        }
    }
    for (t, p, _) in crate::transplants::undischarged(snapshot_transplants, snapshot_claims) {
        out.push(format!(
            "{p} (on {t}): no live claim answers this premise at the destination"
        ));
    }

    // Direction 2: every transplant the document shows is one the
    // snapshot has.
    for id in rendered_transplant_ids(doc_body) {
        if !live.iter().any(|t| t.id == id) {
            out.push(format!(
                "the document shows transplant {id}, but its snapshot has no such record — the row was not written by `tetel transplant`"
            ));
        }
    }
    out
}

/// The transplant ids in a rendered `## Transplants` section.
///
/// Deliberately narrow, exactly like [`rendered_target_symbols`]: it
/// reads the headings this crate emits, in the shape it emits them, and
/// is not a markdown parser. A section it cannot read yields no rows and
/// the snapshot direction above still runs.
pub fn rendered_transplants(body: &[String]) -> Vec<String> {
    rendered_transplant_ids(body)
}

fn rendered_transplant_ids(body: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    // Premise text is inlined verbatim, so a donor comment containing a
    // line that looks like a heading is ordinary rather than exotic.
    // Fenced content is skipped entirely: reading a quoted `### …` as a
    // transplant id would invent a machine failure out of the donor's own
    // words.
    let mut fence: Option<usize> = None;
    for line in body {
        let t = line.trim();
        let backticks = t.len() - t.trim_start_matches('`').len();
        match fence {
            Some(open) => {
                if backticks >= open && t.trim_matches('`').is_empty() {
                    fence = None;
                }
                continue;
            }
            None => {
                if backticks >= 3 {
                    fence = Some(backticks);
                    continue;
                }
            }
        }
        if let Some(rest) = t.strip_prefix("###") {
            if inside {
                if let Some(id) = rest.trim().split_whitespace().next() {
                    if !id.is_empty() {
                        out.push(id.to_string());
                    }
                }
                continue;
            }
        }
        if t.starts_with('#') {
            inside = t.trim_start_matches('#').trim().eq_ignore_ascii_case("transplants");
        }
    }
    out
}

/// The symbols in a rendered `## Modification targets` table.
///
/// Deliberately narrow: it reads the one table this crate emits, in the
/// shape it emits, and stops at the next heading. It is not a markdown
/// parser and makes no attempt to be — a document whose section this
/// cannot read yields no rows here, and the snapshot direction above
/// still runs.
pub fn rendered_targets(body: &[String]) -> Vec<String> {
    rendered_target_symbols(body)
}

fn rendered_target_symbols(body: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in body {
        let t = line.trim();
        if t.starts_with('#') {
            inside = t.trim_start_matches('#').trim().eq_ignore_ascii_case("modification targets");
            continue;
        }
        if !inside || !t.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
        let Some(first) = cells.first() else { continue };
        // Skip the header and its separator row.
        if first.eq_ignore_ascii_case("target") || first.chars().all(|c| c == '-' || c == ':') {
            continue;
        }
        let symbol = first.trim_matches('`').trim();
        if !symbol.is_empty() {
            out.push(symbol.to_string());
        }
    }
    out
}
