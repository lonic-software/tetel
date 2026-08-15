//! The mint-time verifier — compare what the author just wrote against
//! the evidence the tool already holds, and say so when the two disagree.
//!
//! See `docs/design/tet-verifier-mint-warning.md` for the argument. What
//! follows is what the code has to get right.
//!
//! # It is not a refusal and it never blocks a mint
//!
//! Every finding here is human-owed, in [`crate::scope`]'s posture and for
//! [`crate::scope`]'s reason: naming a location is normal, concluding
//! about it is the failure, and the two are not separable by a machine.
//! Nothing in this module can fail a mint, delay a reply or move an exit
//! code. It does not run inside `check`, does not enter the record, the
//! memo, the snapshot or the evidence ledger, and does not appear in
//! either partition of [`crate::report`] — those arrays state what `check`
//! covers, and adding a non-repeating model call to one of them would make
//! a scope string promise coverage `check` does not have.
//!
//! # Nothing in the reply path waits
//!
//! A mint result is one object. It already carries findings that arrive
//! with certainty *because* a mint is instant — `attention`, `folded`,
//! `refused_since_previous_fact`, `overlap` — and putting a provider call
//! in front of the reply would make every one of them contingent on the
//! provider. So the mint returns immediately, the comparison runs on a
//! detached thread, and its outcome is delivered on the author's next
//! authoring call in the same workspace, which is the pattern
//! [`crate::facts::refusals_since_last_mint`] already establishes here.
//!
//! # `verify` is an object, not a third array
//!
//! `attention` and `overlap` recompute out of files on disk, so for them
//! an empty array and an absent one say the same thing. This does not: a
//! model call can be switched off, refused for want of a key, fail in
//! transport, time out, or come back unreadable, and in every one of those
//! "found nothing" is a different fact from "did not look". Hence a
//! mandatory [`Status`], and `findings` meaningful only under
//! [`Status::Ok`].
//!
//! Two fields name mints rather than statuses, because one reply can both
//! deliver an earlier verification and start a new one, and a reader who
//! cannot tell which mint a finding concerns will attach it to the wrong
//! text: `for_mint` names the mint the status and findings are about, and
//! `queued_for` names the mint whose verification this call started.
//!
//! # A quotation nothing checked is worse than no quotation
//!
//! A finding names the clause it judges and quotes the captured span it
//! judged that clause against, and carries no confidence score — a number
//! invites deference, a quotation invites checking, and checking is the
//! only safe response when the check is wrong. But the quoted span is a
//! string a model produced, and a fabricated one sends the reader to
//! verify against text that does not exist, defeating the very response it
//! was for. So every span goes through [`crate::facts::Fact::quotes`] —
//! the same plain substring relation that makes `transplant` refuse a
//! premise that is not the donor's own words — before the finding enters
//! the payload. A span it rejects is dropped and its finding is downgraded
//! to one that says it carries no quotation. Nothing is refused; the check
//! runs on the model's output, never on the author's.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config;
use crate::facts;
use crate::workspace;

/// Where a completed verification is written, and where the next
/// authoring call reads it from.
///
/// Deliberately **not** in [`crate::snapshot::SNAPSHOT_FILES`]. That
/// constant is an enumeration and `write` copies exactly the names in it,
/// so a workspace file whose name is absent cannot be shipped at all —
/// which is what keeps non-reproducible model output out of a snapshot a
/// reader is entitled to recompute. Shipping it later would take a
/// deliberate line in that array.
const LOG_FILE: &str = "verify.log";

/// The last delivered sequence number. Separate from the log so the log
/// stays append-only and a delivery cannot rewrite a finding.
const CURSOR_FILE: &str = "verify.cursor";

/// The provider the eval measured against, and the only endpoint this
/// module knows. Not a configuration key: five keys are registered and a
/// sixth would have to earn its place under the rule that every setting be
/// visible in the output it affects.
const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Environment variables holding the credential, in order. Never a config
/// key and never written anywhere: config files are shared, committed and
/// pasted into issues.
const KEY_VARS: [&str; 2] = ["OPENROUTER_API_KEY", "TETEL_API_KEY"];

/// The retry budget the eval's own harness used: a truncated draw is
/// retried with a tripled token cap, up to three attempts total. The
/// wall-clock bound is enforced across all of them, never per attempt — a
/// per-attempt bound would quietly license three times the declared spend.
const MAX_ATTEMPTS: u32 = 3;
const FIRST_TOKEN_CAP: u32 = 4000;

/// How much captured output one verification may send.
///
/// The same bound the harness that measured this feature used. Without it
/// the numbers on the page describe a different input from the one the
/// code sends: a fact folded from a large `run` would go whole, and the
/// cost, the latency and the truncation rate would all be figures nobody
/// measured. It also keeps a promise the prompt already makes — "the
/// captured output may have been truncated, and says so where it was" —
/// which nothing was emitting.
const MAX_EVIDENCE_BYTES: usize = 14_000;

/// Every terminal state of a verification, and the three decidable
/// without a call. Total by construction: a state that maps to nothing
/// here would reproduce the defect the whole object exists to prevent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Disabled by configuration.
    Off,
    /// Enabled, but no credential in the environment.
    Unauthorized,
    /// A verification was started for this mint; ask again next call.
    Queued,
    /// The verb is on and could have run, but this call wrote nothing a
    /// captured record can be compared against — a heading, a block
    /// citing no claim, a withdrawal.
    ///
    /// Distinct from `off` because it has to be. Reporting `off` here
    /// tells an author who has just turned the feature on that it is
    /// disabled, which is the confusion the whole status vocabulary
    /// exists to remove, and it is the common case for anyone who sets
    /// `verify.verbs = "prose"`.
    Skipped,
    /// A verification completed and `findings` is meaningful.
    Ok,
    /// Transport failure, a non-2xx reply, or a truncated draw whose body
    /// came back empty. Never `Ok` — an empty body is trivially easy to
    /// mistake for a clean result, which is exactly the mistake the eval's
    /// first harness made.
    Unavailable,
    /// The end-to-end budget expired.
    Timeout,
    /// A 2xx reply, non-empty and non-truncated, whose content is not a
    /// usable answer: no brace-delimited substring, a substring that does
    /// not parse, or a decoded object whose verdict is outside the
    /// permitted vocabulary. None of those is a transport failure and none
    /// of them is a clean bill.
    Unparsable,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Off => "off",
            Status::Unauthorized => "unauthorized",
            Status::Queued => "queued",
            Status::Skipped => "skipped",
            Status::Ok => "ok",
            Status::Unavailable => "unavailable",
            Status::Timeout => "timeout",
            Status::Unparsable => "unparsable",
        }
    }
}

/// One disagreement, in the form that invites checking rather than
/// deference. No confidence score, by design.
///
/// This is the **log** shape. What the author receives is
/// [`Finding::payload`], which is the same thing minus
/// `rejected_span` — see that method for why the two differ.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Finding {
    /// `contradicts`, `overreaches` or `unevidenced`. Anything else the
    /// model returns makes the whole reply [`Status::Unparsable`].
    pub kind: String,
    /// The author's own clause being judged.
    pub clause: String,
    /// Whether [`clause`](Self::clause) is a verbatim substring of the text
    /// that was verified.
    ///
    /// Both system prompts demand *both* quotations verbatim, and for a
    /// long time only one of the two was checked. The asymmetry mattered in
    /// the direction least likely to be noticed: a fabricated evidence span
    /// sends the reader to text that does not exist, which they discover
    /// immediately, whereas a paraphrased clause reads as a quotation of
    /// prose the author is already looking at.
    ///
    /// Unlike a rejected span this is reported rather than withheld. The
    /// span points *outside* the finding, so an unverified one is worse
    /// than none; the clause points at the author's own visible text, where
    /// a paraphrase is still a usable pointer and deleting it would leave a
    /// finding with nothing to attach to.
    #[serde(default)]
    pub clause_quoted: bool,
    /// Every fact whose captured output contains
    /// [`evidence`](Self::evidence) — not the first, and not the model's
    /// word for it.
    ///
    /// The model is never asked which fact it read, because it is never
    /// asked to track ids. That is the right division of labour, but it
    /// used to be resolved by searching for the span and keeping the
    /// *first* fact that contained it, falling back to the first fact
    /// overall when none did. Two states were thereby collapsed into a
    /// confident-looking answer: a span living in several captures got
    /// attributed by cite order rather than by truth, and a span living in
    /// none got attributed anyway.
    ///
    /// Containment is the honest relation and it is set-valued, so this
    /// says so. Empty means no capture shown to the model contained the
    /// span, which is exactly [`quoted`](Self::quoted) being false.
    #[serde(default)]
    pub facts: Vec<String>,
    /// Which half of the captured record the verified span came from —
    /// `output` or `extent`. Absent when nothing verified.
    ///
    /// Worth reporting rather than flattening, because the two mean
    /// different things to whoever reads the finding: an `output` span is
    /// the capture disagreeing with the text, an `extent` span is the
    /// capture's *reach* disagreeing with it — which is usually what an
    /// `overreaches` finding is actually about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quoted_from: Option<String>,
    /// The pre-set-valued spelling of [`facts`](Self::facts), read from
    /// logs written before it and never written again. Folded in by
    /// [`read_log`] so an existing log keeps its history instead of
    /// arriving as unparsable lines.
    #[serde(default, rename = "fact", skip_serializing)]
    pub legacy_fact: Option<String>,
    /// The captured span, present only when some fact in
    /// [`facts`](Self::facts) contains it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// For `unevidenced`: the literal in the author's text that no cited
    /// capture carries. Absent on the two disagreement kinds, which quote
    /// the captured side instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal: Option<String>,
    /// Why the two disagree, in the model's words.
    pub why: String,
    /// False when a span was offered and rejected — said in the payload
    /// rather than left for the reader to notice an absence.
    ///
    /// Always false on an `unevidenced` finding, where nothing was quoted
    /// from the capture because the whole assertion is that there is
    /// nothing there to quote. [`report_text`] scores quote fidelity over
    /// the evidence-bearing kinds alone for that reason.
    pub quoted: bool,
    /// The span [`crate::facts::Fact::quotes`] refused, kept here and
    /// nowhere else.
    ///
    /// Withholding a fabricated quotation from the *author* is the whole
    /// point of the check — it would send them to verify against text that
    /// does not exist. Deleting it from the *log* is a different thing and
    /// a mistake: quote fidelity was 73% in the eval, which makes this one
    /// of the strongest tuning signals available, and a count of
    /// fabrications you can never look at is not one you can act on. The
    /// log is a local file that no snapshot copies, so keeping it here
    /// costs nothing the author can be misled by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_span: Option<String>,
}

impl Finding {
    /// The author-facing shape: everything except the span that failed
    /// verification.
    ///
    /// Built by transforming rather than by a second struct, so a field
    /// added to the log cannot reach the payload by forgetting to exclude
    /// it — the payload names what it emits.
    pub fn payload(&self) -> serde_json::Value {
        let mut out = json!({
            "kind": self.kind,
            "clause": self.clause,
            "clause_quoted": self.clause_quoted,
            "facts": self.facts,
            "why": self.why,
            "quoted": self.quoted,
        });
        if let Some(e) = &self.evidence {
            out["evidence"] = json!(e);
        }
        if let Some(w) = &self.quoted_from {
            out["quoted_from"] = json!(w);
        }
        if let Some(l) = &self.literal {
            out["literal"] = json!(l);
        }
        out
    }

    /// Whether this kind quotes the captured side at all — true for the two
    /// disagreement kinds, false for `unevidenced`, whose entire content is
    /// that the capture holds nothing to quote.
    pub fn quotes_evidence(&self) -> bool {
        self.kind != KIND_UNEVIDENCED
    }
}

/// The three kinds, named once. `parse_findings` refuses a reply carrying
/// anything else, so a fourth cannot arrive by a model inventing it.
const KIND_CONTRADICTS: &str = "contradicts";
const KIND_OVERREACHES: &str = "overreaches";
const KIND_UNEVIDENCED: &str = "unevidenced";

/// Why this reply has no verification of its own to report.
///
/// Three answers, not two. Collapsing "nothing to compare" into "not
/// attempted" made an enabled verifier answer `off` on every heading and
/// every uncited block, which reads as "you did not turn it on" to
/// exactly the author who just did.
pub enum Trigger<'a> {
    /// A verification started for this mint.
    Queued(&'a str),
    /// The verb is on, and this call wrote nothing comparable.
    NothingToCompare,
    /// Nothing was started; [`block`] works out why from the settings.
    NotAttempted,
}

/// The effective settings, resolved once and echoed into every `verify`
/// object this module builds.
///
/// All five are echoed, including on the calls where the setting made
/// nothing happen. `config.rs` admits a key only if it is visible in the
/// output it affects, and three of these five would otherwise be invisible
/// — a timeout surfaces only in the state that trips it, a verb list only
/// by inference from an absence, and the approach, the choice between two
/// materially different mechanisms, nowhere at all. Widening the echo is
/// the answer to that; keeping a key in defiance of the rule is not.
#[derive(Clone, Debug)]
pub struct Settings {
    pub enabled: bool,
    pub model: Option<String>,
    pub approach: String,
    pub timeout_ms: u64,
    pub verbs: Vec<String>,
    pub literals: bool,
}

/// How long one provider call is allowed, when nothing configured a budget.
///
/// The default is per *leg* rather than per verification, because a
/// verification is one, two or three calls in series and a flat number
/// silently means three different things. Measured over the corpus, a
/// single call's median is under 10 seconds and its p90 around 50 — so the
/// flat 60 seconds this replaced gave a two-call `split` run no headroom at
/// all, and a three-call run with `literals` on almost none. Nothing waits
/// on this budget: it bounds a detached thread whose only job is to write a
/// log line, so being generous costs a slow failure, while being tight
/// costs findings.
const DEFAULT_MS_PER_LEG: u64 = 60_000;

pub fn settings(workspace_dir: &Path) -> Settings {
    let d = Some(workspace_dir);
    let approach = config::verify_approach(d);
    let literals = config::verify_literals(d);
    Settings {
        enabled: config::verify_enabled(d),
        model: config::verify_model(d),
        timeout_ms: config::verify_timeout_ms(d)
            .unwrap_or(DEFAULT_MS_PER_LEG * u64::from(expected_calls(&approach, literals))),
        approach,
        verbs: config::verify_verbs(d),
        literals,
    }
}

fn api_key() -> Option<String> {
    KEY_VARS
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.trim().is_empty()))
}

fn log_path(dir: &Path) -> PathBuf {
    dir.join(LOG_FILE)
}

fn cursor_path(dir: &Path) -> PathBuf {
    dir.join(CURSOR_FILE)
}

/// What a completed verification left behind for the next call to find,
/// and what a later analysis has to work from.
///
/// The operational half — cost, elapsed, attempts, detail — is here
/// because it is the only place it can be. Everything else about a
/// verification is recoverable after the fact: the wording compared is
/// `mint` plus `at` replayed against `claims.jsonl`, and the verdicts a
/// later pass reached are in the memo's own evidence ledger, joinable by
/// claim id. What a call cost, how long it took and how many attempts it
/// needed are gone the moment the thread ends unless they are written
/// down here.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Record {
    pub seq: u64,
    /// The mint this verification concerns — no longer the id sitting
    /// beside the findings when they are delivered.
    pub mint: String,
    pub verb: String,
    pub status: String,
    pub model: String,
    pub approach: String,
    /// Whether the literal check ran. Recorded for the same reason
    /// `approach` is: it changes how many calls a clean verification makes,
    /// so `expected_calls` cannot tell a retry from a configuration without
    /// it, and it changes which kinds could appear at all.
    #[serde(default)]
    pub literals: bool,
    pub findings: Vec<Finding>,
    pub at: u64,
    /// What the provider reported this verification cost, summed across
    /// every call it made. Zero when the provider reported nothing.
    #[serde(default)]
    pub cost: f64,
    /// Wall-clock milliseconds, end to end across retries — the same
    /// span `verify.timeout_ms` bounds, so the two are comparable.
    #[serde(default)]
    pub elapsed_ms: u64,
    /// How many provider calls were actually made, retries included. The
    /// eval measured reasoning length swinging between 516 and 2000
    /// tokens with ceiling-hitting draws returning empty bodies, so a
    /// retry count is the difference between "slow model" and "we paid
    /// three times for one answer".
    #[serde(default)]
    pub attempts: u32,
    /// Why a non-`ok` status happened, in as much detail as was
    /// available. A 429, a 500 and a name-resolution failure are all
    /// `unavailable`, and they are three different problems.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// [`Telemetry::not_verbatim`], persisted. A finding that reached the
    /// author carries its own fidelity marks; these two count what was
    /// dropped *before* anything reached them, and a drop nobody can see
    /// is the same un-actionable silence `rejected_span` exists to break.
    #[serde(default)]
    pub not_verbatim: u32,
    /// [`Telemetry::literals_refuted`], persisted — the only accuracy
    /// signal the `unevidenced` kind has, since no eval has scored it.
    #[serde(default)]
    pub literals_refuted: u32,
    /// [`Telemetry::not_a_quantity`], persisted.
    #[serde(default)]
    pub not_a_quantity: u32,
    /// The status of the literal leg when it ran and did **not** complete.
    ///
    /// `None` means either that the leg was off or that it finished, and
    /// those two are told apart by [`literals`](Self::literals). Present
    /// only on the failure, because that is the case where `findings` is
    /// complete for the disagreement kinds and silent for this one — a
    /// distinction the author cannot infer from an absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literals_status: Option<String>,
}

// ---------------------------------------------------------------------
// The shared block. Every result that carries a `verify` key builds it
// here — `fact_result` and the inline `ClaimOutcome`/`ProseOutcome` arms
// alike — so the status vocabulary, the model name, the guidance string
// and the non-determinism marker cannot drift between verbs. This is the
// discipline `scope::advice` already enforces for `attention`.
// ---------------------------------------------------------------------

/// The guidance that travels with the finding rather than only in the
/// tool description.
///
/// The precedent keeps its register in both places but not in the same
/// words, and the sharpest statement of it — "It is not an error — read it
/// and decide" — sits on the `claim` tool's description, a surface a
/// caller loads once and may never read again, while the finding that
/// needs it arrives alone.
const GUIDANCE: &str = "Not an error and not a refusal. A model compared what you wrote \
against what the tool captured, and the two look inconsistent to it. Read the quoted span \
— or, on an `unevidenced` finding, the literal it names, which is there precisely because \
no capture carried it — and decide: fix the wording, look at something you have not opened, \
or leave it alone because the finding is wrong. It is wrong a meaningful fraction of the time.";

/// The `verify` object for one reply.
///
/// `delivered` is a verification that finished before this call;
/// `queued_for` is the mint whose verification this call just started.
/// Either may be absent, and when both are the status is whichever
/// pre-call state applies.
pub fn block(
    settings: &Settings,
    verb: &str,
    delivered: Option<&Record>,
    trigger: Trigger<'_>,
) -> serde_json::Value {
    let status = match (delivered, &trigger) {
        (Some(r), _) => r.status.clone(),
        (None, Trigger::Queued(_)) => Status::Queued.as_str().to_string(),
        (None, Trigger::NothingToCompare) => Status::Skipped.as_str().to_string(),
        // Nothing was attempted, and the reasons are worth telling apart:
        // an author who turned the feature on and is silently getting
        // nothing needs to know whether the switch, the verb list or a
        // missing credential is why.
        (None, Trigger::NotAttempted) => {
            if !settings.enabled || !settings.verbs.iter().any(|v| v == verb) {
                Status::Off.as_str().to_string()
            } else {
                // Enabled and listed, yet nothing started: the only thing
                // left that stops `spawn` is having nothing to call with.
                Status::Unauthorized.as_str().to_string()
            }
        }
    };
    // `unauthorized` covers two different gaps, and the obvious first
    // step — `config verify.enabled true` with a key already exported —
    // hits the one that is *not* about the credential. Naming which is
    // missing is the difference between a one-line fix and an afternoon
    // spent debugging a key that was fine all along.
    let missing = if status == Status::Unauthorized.as_str() {
        match (settings.model.is_none(), api_key().is_none()) {
            (true, true) => Some("`verify.model` is not set and no API key is in the environment"),
            (true, false) => Some("`verify.model` is not set — `tetel config verify.model <vendor/model>`"),
            (false, true) => Some("no API key in the environment — export OPENROUTER_API_KEY"),
            (false, false) => None,
        }
    } else {
        None
    };
    let mut out = json!({
        "status": status,
        // Stated in every response, including the silent ones. The two
        // findings this one sits beside recompute to the same answer every
        // time because their inputs are files on disk; this one does not,
        // and a reader who sees three fields together will assume parity
        // unless told otherwise.
        "deterministic": false,
        "model": settings.model.clone().unwrap_or_default(),
        "approach": settings.approach.clone(),
        "timeout_ms": settings.timeout_ms,
        "verbs": settings.verbs.clone(),
        "literals": settings.literals,
        "guidance": GUIDANCE,
    });
    let map = out.as_object_mut().expect("json object");
    if let Some(r) = delivered {
        map.insert("for_mint".into(), json!(r.mint));
        // Absent rather than empty under any status but `ok`. A 429 gives
        // a record whose `findings` is `[]` because the comparison never
        // happened, and emitting that alongside `"status":"unavailable"`
        // hands a caller the empty disagreement list it would read as a
        // clean bill — the one confusion this whole object exists to
        // prevent. The status guard is the mechanism; the comment is not.
        if r.status == Status::Ok.as_str() {
            // Through `payload`, never `to_value` on the record: the log
            // shape carries a span that failed verification and the
            // author must not receive it.
            let shown: Vec<serde_json::Value> = r.findings.iter().map(Finding::payload).collect();
            map.insert("findings".into(), json!(shown));
            // The one qualification an `ok` can carry. Without it, a
            // verification whose literal leg timed out is indistinguishable
            // from one that ran it and found nothing — which is the exact
            // "found nothing versus did not look" confusion this object
            // exists to prevent, reappearing one level down.
            if let Some(s) = &r.literals_status {
                map.insert("literals_incomplete".into(), json!(s));
            }
        }
    }
    if let Trigger::Queued(m) = trigger {
        map.insert("queued_for".into(), json!(m));
    }
    if let Some(m) = missing {
        map.insert("detail".into(), json!(m));
    }
    out
}

/// Whether this verb is verified at all, given the effective settings.
pub fn verb_enabled(settings: &Settings, verb: &str) -> bool {
    settings.enabled && settings.verbs.iter().any(|v| v == verb)
}

/// How many log entries have already been shown to the author.
///
/// A count of delivered records, **not** a high-water mark over `seq`.
/// The distinction is load-bearing. `seq` is chosen inside the spawned
/// thread by reading the log and adding one, which is neither atomic nor
/// ordered: two verifications in flight can finish out of order, so the
/// log can hold seq 2 before seq 1, and can even hold two records that
/// both took seq 1 because neither saw the other's append. A cursor that
/// remembered the largest seq delivered would then skip a real finding on
/// a real claim forever. Counting positions in an append-only file has
/// neither problem, and needs no coordination between threads.
fn delivered_count(dir: &Path) -> usize {
    std::fs::read_to_string(cursor_path(dir))
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

/// The next verification owed to the author, without consuming it.
///
/// Oldest first, one per call. Verifications are per-mint and mints are
/// sequential in a workspace, so more than one outstanding is unusual; the
/// rest keep until the calls after this one rather than being merged into
/// a single object that could only name one mint.
/// Returns the record and the position it sits at, which
/// [`commit_delivered`] needs so a concurrent commit cannot skip past it.
pub fn peek_delivered(dir: &Path) -> Option<(usize, Record)> {
    let at = delivered_count(dir);
    read_log(dir).0.into_iter().nth(at).map(|r| (at, r))
}

/// Every readable record in the log, and how many lines were not.
///
/// [`workspace::read_jsonl`] fails the whole file on one malformed line,
/// which is right for a ledger and wrong here. This log is appended to
/// from a detached thread; a crash mid-append leaves a truncated line,
/// and under the strict reader that one line would stop every future
/// delivery — permanently, silently, and with `verify-report` announcing
/// that the verifier had never been enabled. Skipping what will not parse
/// costs nothing that matters: positions stay stable because nothing ever
/// rewrites the log, and the count of skipped lines is reported rather
/// than swallowed.
fn read_log(dir: &Path) -> (Vec<Record>, usize) {
    let Ok(text) = std::fs::read_to_string(log_path(dir)) else {
        return (Vec::new(), 0);
    };
    let mut records = Vec::new();
    let mut skipped = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<Record>(line) {
            Ok(mut r) => {
                // Fold the pre-set-valued attribution forward. A log
                // written before `facts` existed carries `fact` instead,
                // and the alternative to reading it is a report that
                // announces every past record as an unparsable line —
                // discarding exactly the history the report is for.
                for f in &mut r.findings {
                    if let Some(one) = f.legacy_fact.take() {
                        if f.facts.is_empty() && !one.is_empty() {
                            f.facts.push(one);
                        }
                    }
                }
                records.push(r);
            }
            Err(_) => skipped += 1,
        }
    }
    (records, skipped)
}

/// Mark the peeked record as delivered.
///
/// Separate from [`peek_delivered`] so that a call which never reaches the
/// author — a refusal, and refusals are routine — cannot consume the
/// finding it was carrying. Committing only where the payload is actually
/// built means the worst case is showing a finding twice, not losing one.
/// Mark everything up to and including `index` as delivered.
///
/// Takes the position that was peeked rather than incrementing whatever
/// the cursor happens to hold now, and never moves the cursor backwards.
/// Peek and commit are separated by a whole tool call and nothing
/// serialises the MCP handlers per workspace, so two authoring calls can
/// interleave: both peek position N, and a blind `count + 1` from each
/// would land the cursor at N+2, skipping N+1 — the record that may carry
/// the finding. Writing `max(current, index + 1)` from both leaves it at
/// N+1, so the worst case is the stated one: a finding shown twice, never
/// one lost.
pub fn commit_delivered(dir: &Path, index: usize) {
    let next = std::cmp::max(delivered_count(dir), index + 1);
    let _ = std::fs::write(cursor_path(dir), next.to_string());
}

/// A sequence number for the log's own readability. Nothing depends on it
/// being unique or ordered — see [`delivered_count`] for why it must not.
fn next_seq(dir: &Path) -> u64 {
    workspace::read_jsonl::<Record>(&log_path(dir))
        .map(|rs| rs.iter().map(|r| r.seq).max().unwrap_or(0) + 1)
        .unwrap_or(1)
}

// ---------------------------------------------------------------------
// Running one.
// ---------------------------------------------------------------------

/// What is being compared: the author's text, and the captured side.
///
/// The captured side is never typeable and never narrowable by selection
/// — for a claim it is the cited facts **together with the overlap set**,
/// because an overreaching proposition could otherwise be made to agree
/// with its evidence by citing only the facts that agree with it. Both
/// halves of `scope.rs`'s construction have to survive or the comparison
/// is the author's diligence checking the author's diligence.
pub struct Subject {
    pub mint: String,
    pub verb: String,
    pub text: String,
    /// `(fact id, extent labels, each observation's captured output)`.
    ///
    /// Per observation rather than joined, so that what the model is
    /// shown is exactly what `Fact::quotes` can accept back.
    pub evidence: Vec<(String, Vec<String>, Vec<String>)>,
}

/// Where in the captured record a verified span was found.
///
/// Both halves are shown to the model by [`evidence_text`] and both are the
/// tool's own record rather than the author's text — an extent label is
/// generated from the designator `look`/`run` resolved, not typed — so a
/// span from either is an honest quotation. They answer different questions
/// and the payload says which: an output span shows what the capture
/// *contains*, an extent span shows what it *covers*, which is the natural
/// thing to point at when the disagreement is that a claim ranges wider
/// than the capture does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuotedFrom {
    Output,
    Extent,
}

impl QuotedFrom {
    fn as_str(self) -> &'static str {
        match self {
            QuotedFrom::Output => "output",
            QuotedFrom::Extent => "extent",
        }
    }
}

impl Subject {
    /// Every fact whose captured record contains `span`, and where.
    ///
    /// Per observation and unnormalised, the relation
    /// [`crate::facts::Fact::quotes`] applies, over the material actually
    /// put in front of the model rather than the whole workspace: a span
    /// occurring only in some fact this comparison never showed is not a
    /// quotation of the evidence, and crediting it would credit the model
    /// for text it could not have read.
    ///
    /// # Why the extent labels count
    ///
    /// They used to not, and that was a bug of exactly the kind
    /// [`evidence_text`] documents itself against — "The two have to agree
    /// or the quote check punishes honesty." That doc comment fixed the
    /// joined-versus-per-observation half and missed this one: the labels
    /// block is shown to the model under the heading "what was opened or
    /// run", the model is told to quote the captured evidence, and a span it
    /// copied from that block was then stripped as a fabrication.
    ///
    /// Measured over 123 real fact notes, **15 of the 25 rejected spans were
    /// verbatim in the labels block** — so the fabrication rate the tool
    /// reported was more than twice the real one, and two thirds of what it
    /// called invention was the model quoting what it was shown. A rate that
    /// wrong is worse than no rate, because `rejected_span` exists to be
    /// tuned on.
    ///
    /// This does not touch [`crate::facts::Fact::quotes`], which stays the
    /// output-only relation `transplant` refuses premises with. A premise is
    /// a donor's own words and an extent label is not; the two checks want
    /// different answers and now give them.
    fn containing(&self, span: &str) -> Vec<(String, QuotedFrom)> {
        if span.is_empty() {
            return Vec::new();
        }
        self.evidence
            .iter()
            .filter_map(|(id, extent, obs)| {
                if obs.iter().any(|o| o.contains(span)) {
                    Some((id.clone(), QuotedFrom::Output))
                } else if extent.iter().any(|e| e.contains(span)) {
                    Some((id.clone(), QuotedFrom::Extent))
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Start a verification on a detached thread and return at once.
///
/// Returns whether a thread actually started. The caller needs that
/// answer and not a guess: a reply saying `queued` for a verification
/// that never began is a promise of a finding that can never arrive, and
/// an author polling for it waits forever. The two ways this returns
/// `false` — no credential, no model configured — are exactly the states
/// [`Status::Unauthorized`] and [`Status::Off`] exist to name.
///
/// Every failure *inside* the thread is written to the log as a status,
/// never propagated: a mint has already been committed and replied to by
/// the time it runs, and nothing there may reach back into it.
#[must_use]
pub fn spawn(dir: &Path, settings: &Settings, subject: Subject) -> bool {
    let Some(key) = api_key() else { return false };
    let Some(model) = settings.model.clone() else { return false };
    let dir = dir.to_path_buf();
    let approach = settings.approach.clone();
    let literals = settings.literals;
    let budget = Duration::from_millis(settings.timeout_ms);
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut tel = Telemetry::default();
        let (status, findings, literals_status) =
            run(&key, &model, &approach, literals, &subject, started, budget, &mut tel);
        let record_literals_ok = literals_status.is_none();
        let record = Record {
            seq: next_seq(&dir),
            mint: subject.mint.clone(),
            verb: subject.verb.clone(),
            status: status.as_str().to_string(),
            model,
            approach,
            literals,
            findings,
            at: workspace::now_unix(),
            cost: tel.cost,
            elapsed_ms: started.elapsed().as_millis() as u64,
            attempts: tel.attempts,
            not_verbatim: tel.not_verbatim,
            literals_refuted: tel.literals_refuted,
            not_a_quantity: tel.not_a_quantity,
            literals_status: literals_status.map(|s| s.as_str().to_string()),
            // Only when something went wrong: a clean run has nothing to
            // explain, and a detail line on every record would train a
            // reader to skip the field.
            // An `ok` run with a failed literal leg is the one case where a
            // clean status still has something to explain, so the guard
            // asks about both rather than about the status alone.
            detail: if status == Status::Ok && record_literals_ok {
                None
            } else {
                tel.detail
            },
        };
        let _ = workspace::append_jsonl(&log_path(&dir), &record);
    });
    true
}

/// What one verification spent getting to its answer, accumulated across
/// however many calls the approach and the retries required.
#[derive(Default)]
pub struct Telemetry {
    pub cost: f64,
    pub attempts: u32,
    pub detail: Option<String>,
    /// Text the model attributed to the author that the author did not
    /// write, dropped rather than passed on: a classify assertion that was
    /// not a substring of the claim, or a literal the literal check could
    /// not find in the text. One counter for both because it is one
    /// failure — the model returning its own words as a quotation.
    pub not_verbatim: u32,
    /// `unevidenced` findings dropped because the literal turned out to be
    /// in the capture after all. The model's claim was checkable and
    /// checked; this counts how often it was wrong, which is the only
    /// accuracy signal that kind has.
    pub literals_refuted: u32,
    /// Findings dropped because the literal named no quantity — the model
    /// reaching for a symbol, a flag, a path or a quantifier. Counted
    /// rather than silently discarded: this is the rate that says whether
    /// [`is_checkable`] is carrying the check or fighting it.
    pub not_a_quantity: u32,
}

#[allow(clippy::too_many_arguments)]
fn run(
    key: &str,
    model: &str,
    approach: &str,
    literals: bool,
    subject: &Subject,
    started: Instant,
    budget: Duration,
    tel: &mut Telemetry,
) -> (Status, Vec<Finding>, Option<Status>) {
    // `split` classifies the claim's assertions before checking them, so
    // the check can be told which ones the captured evidence is even
    // able to speak to. It costs a second call and it is the default,
    // because it is the configuration the retrodiction measured. One-call
    // comparisons have been run over the same corpus, but not this arm's
    // prompt pairing, and none of their numbers were carried into the
    // decision to ship. `direct` is cheaper, but not by the half a
    // reader would assume from "one call instead of two": the call it
    // drops carries the claim text alone, while the one it keeps carries
    // the evidence blob. The ratio is measured nowhere.
    let labelled = if approach == "split" {
        let body = match call(key, model, CLASSIFY_SYSTEM, &classify_prompt(subject), started, budget, tel)
        {
            Ok(b) => b,
            Err(s) => return (s, Vec::new(), None),
        };
        // Decoded, not forwarded. The classify reply used to be spliced
        // into the check prompt as whatever string came back, which meant
        // its declared schema was documentation rather than a contract: a
        // refusal, a preamble, a half-written object or a reasoning dump
        // all went into the second call verbatim, and `split` could
        // silently degrade to `direct`-plus-noise with no status to show
        // for it. It is re-emitted in the same shape the eval fed, so
        // decoding it changes what the check call sees only where what it
        // used to see was not an answer.
        match parse_assertions(&body, &subject.text, tel) {
            Ok(canonical) => Some(canonical),
            Err(why) => {
                tel.detail = Some(why);
                return (Status::Unparsable, Vec::new(), None);
            }
        }
    } else {
        None
    };
    let prompt = check_prompt(subject, labelled.as_deref());
    match call(key, model, CHECK_SYSTEM, &prompt, started, budget, tel) {
        Ok(body) => match parse_findings(&body, subject) {
            Some(mut f) => {
                // A failing literal leg no longer takes the disagreement
                // findings down with it. It used to, on the argument that
                // `ok` must mean the configured comparison happened — right
                // principle, wrong trade. Combined over the corpus, the two
                // disagreement kinds carry 30% recall and this one 13%, so
                // discarding the stronger half because the weaker half
                // returned a 429 loses more than it protects. The principle
                // survives by being *reported* instead of enforced: the
                // status stays `ok` because the comparison it names did
                // complete, and `literals_incomplete` says the other leg did
                // not, so "found no literals" and "never asked" remain
                // different payloads.
                let mut lit_status = None;
                if literals {
                    match literal_findings(key, model, subject, started, budget, tel) {
                        Ok(mut l) => f.append(&mut l),
                        Err(s) => lit_status = Some(s),
                    }
                }
                (Status::Ok, f, lit_status)
            }
            None => {
                // Say what could not be read. "Unparsable" alone sends
                // whoever is tuning this back to the provider to guess.
                tel.detail = Some(format!(
                    "reply was not a usable answer; {} bytes beginning: {}",
                    body.len(),
                    body.chars().take(200).collect::<String>()
                ));
                (Status::Unparsable, Vec::new(), None)
            }
        },
        Err(s) => (s, Vec::new(), None),
    }
}

/// One provider call, retried on a truncated draw within the shared
/// budget. Returns the assistant's content or the status that ends the
/// verification.
fn call(
    key: &str,
    model: &str,
    system: &str,
    user: &str,
    started: Instant,
    budget: Duration,
    tel: &mut Telemetry,
) -> Result<String, Status> {
    let mut cap = FIRST_TOKEN_CAP;
    for _ in 0..MAX_ATTEMPTS {
        let Some(left) = budget.checked_sub(started.elapsed()) else {
            tel.detail = Some(format!(
                "budget of {}ms expired before an attempt could start",
                budget.as_millis()
            ));
            return Err(Status::Timeout);
        };
        tel.attempts += 1;
        let body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": 0.0,
            "max_tokens": cap,
            "reasoning": {"effort": "high"},
        });
        let Ok(payload) = serde_json::to_string(&body) else {
            tel.detail = Some("request body would not serialise".into());
            return Err(Status::Unavailable);
        };
        let reply = ureq::post(ENDPOINT)
            .header("Authorization", &format!("Bearer {key}"))
            .header("Content-Type", "application/json")
            .config()
            .timeout_global(Some(left))
            .build()
            .send(payload.as_str());
        let mut reply = match reply {
            Ok(r) => r,
            // A timeout inside the client is still the budget expiring;
            // anything else is transport or a non-2xx.
            Err(ureq::Error::Timeout(_)) => {
                tel.detail = Some("provider did not answer within the remaining budget".into());
                return Err(Status::Timeout);
            }
            // The distinction that makes this field worth having: a 429,
            // a 500 and a name-resolution failure are all `unavailable`
            // and call for three different responses.
            Err(ureq::Error::StatusCode(code)) => {
                tel.detail = Some(format!("provider replied {code}"));
                return Err(Status::Unavailable);
            }
            Err(e) => {
                tel.detail = Some(format!("transport failure: {e}"));
                return Err(Status::Unavailable);
            }
        };
        let Ok(text) = reply.body_mut().read_to_string() else {
            tel.detail = Some("reply body could not be read".into());
            return Err(Status::Unavailable);
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            tel.detail = Some(format!("provider envelope was not JSON ({} bytes)", text.len()));
            return Err(Status::Unparsable);
        };
        // Whatever the provider says it charged, summed across attempts.
        // Absent on providers that report none, which is why it defaults
        // to zero rather than being an Option nobody would branch on.
        tel.cost += v["usage"]["cost"].as_f64().unwrap_or(0.0);
        let choice = &v["choices"][0];
        let content = choice["message"]["content"].as_str().unwrap_or("");
        let truncated = choice["finish_reason"].as_str() == Some("length");
        if !truncated && !content.trim().is_empty() {
            return Ok(content.to_string());
        }
        tel.detail = Some(format!(
            "draw {} came back {} at a {cap}-token cap",
            tel.attempts,
            if truncated { "truncated" } else { "empty" }
        ));
        // The state the eval already met: a draw that reaches the token
        // ceiling comes back with a body of length zero. Trivially easy to
        // treat as a clean result; it is not one.
        cap = cap.saturating_mul(3);
    }
    Err(Status::Unavailable)
}

/// Recover the reply's JSON object and read the disagreements out of it.
///
/// Three distinct shapes are unusable and none of them is a transport
/// failure: no brace-delimited substring at all, a substring that does not
/// parse, and a decoded object whose `kind` is outside the two permitted
/// values. `None` here becomes [`Status::Unparsable`], which is what keeps
/// an unreadable answer from arriving as a clean bill.
fn parse_findings(body: &str, subject: &Subject) -> Option<Vec<Finding>> {
    let v = json_object(body)?;
    let rows = v.get("disagreements")?.as_array()?;
    let mut out = Vec::new();
    for row in rows {
        let kind = str_field(row, "kind");
        if kind != KIND_CONTRADICTS && kind != KIND_OVERREACHES {
            return None;
        }
        let span = str_field(row, "evidence");
        // One containment search, not two. It used to run here to pick an
        // attribution and again in a separate pass to "verify" the
        // quotation — the same predicate over the same text, so the second
        // could only ever reject what the first had failed to match, and
        // the fidelity number it produced measured nothing the first had
        // not already decided. Computing both from one pass makes them
        // agree by construction and states the relation once.
        let found = subject.containing(&span);
        let quoted = !found.is_empty();
        // Named once, from the first match, because a span in two facts'
        // records is in the same kind of place in both often enough that a
        // per-fact answer would be noise. `facts` already carries the set.
        let quoted_from = found.first().map(|(_, w)| *w);
        let facts: Vec<String> = found.into_iter().map(|(id, _)| id).collect();
        let clause = str_field(row, "clause");
        out.push(Finding {
            kind,
            clause_quoted: !clause.is_empty() && subject.text.contains(&clause),
            clause,
            facts,
            quoted_from: quoted_from.map(|w| w.as_str().to_string()),
            legacy_fact: None,
            evidence: quoted.then(|| span.clone()),
            literal: None,
            why: str_field(row, "why"),
            quoted,
            // Moved, not deleted: withheld from the author, kept for
            // whoever later asks how often the model invents a quotation.
            rejected_span: (!quoted && !span.is_empty()).then_some(span),
        });
    }
    Some(out)
}

/// The three labels [`CLASSIFY_SYSTEM`] may return, in one place so the
/// prompt and the validator cannot drift.
const CLASSIFY_LABELS: [&str; 3] = ["current", "proposed", "argument"];

/// Decode the classify reply, drop the assertions it did not quote
/// verbatim, and re-emit the rest in the shape the prompt declared.
///
/// The verbatim rule is the whole value of the step. `CLASSIFY_SYSTEM` says
/// "You are only sorting the author's own words", and an assertion that is
/// not a substring of the claim is not a sorting of them — it is a
/// paraphrase that the check call will then be told is the author's text,
/// and may report a disagreement against. Dropped rather than corrected,
/// and counted in [`Telemetry::paraphrased`] so the rate is visible.
///
/// `Err` is [`Status::Unparsable`]: no object, no `assertions` array, a
/// label outside [`CLASSIFY_LABELS`], or nothing left after the verbatim
/// filter. The last is the one worth stating — a `split` run whose split
/// produced nothing usable has not done what `split` means, and returning
/// `None` there would quietly run the `direct` comparison under the
/// `split` name.
fn parse_assertions(body: &str, claim: &str, tel: &mut Telemetry) -> Result<String, String> {
    let v = json_object(body).ok_or("classify reply held no JSON object")?;
    let rows = v
        .get("assertions")
        .and_then(|a| a.as_array())
        .ok_or("classify reply had no `assertions` array")?;
    let mut kept = Vec::new();
    for row in rows {
        let text = str_field(row, "text");
        let label = str_field(row, "label");
        if !CLASSIFY_LABELS.contains(&label.as_str()) {
            return Err(format!(
                "classify returned the label {label:?}, which is not one of {}",
                CLASSIFY_LABELS.join(", ")
            ));
        }
        if text.is_empty() || !claim.contains(&text) {
            tel.not_verbatim += 1;
            continue;
        }
        kept.push(json!({"text": text, "label": label}));
    }
    if kept.is_empty() {
        return Err(format!(
            "classify returned {} assertion(s), none of them quoted verbatim from the claim",
            rows.len()
        ));
    }
    Ok(json!({"assertions": kept}).to_string())
}

/// The cardinal number words a quantity may be spelled with. Small and
/// closed on purpose: a memo writes "four" and "one unit test", and a
/// filter that only accepted digits would drop a genuine count for being
/// spelled out. Beyond twelve, prose uses digits.
const NUMBER_WORDS: [&str; 13] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve",
];

/// File extensions that make a literal a **path** — something a capture
/// either carries or does not, checkable the same way a count is.
const PATH_SUFFIXES: [&str; 10] =
    [".rs", ".py", ".md", ".json", ".jsonl", ".toml", ".log", ".txt", ".sh", ".html"];

/// Whether a literal is **checkable** — a quantity or a path — rather than
/// a bare name.
///
/// Measured, and the two halves were measured separately because they are
/// two different arguments.
///
/// The quantity half exists because the corpus's noise was designators:
/// `look_grep`, `facts::mint`, `--why`, `2.6.0-FreeBSD`, `check 5`, TET-36.
/// Instructing the model to skip them helped and did not hold — it stopped
/// naming symbols and started naming *quantifiers* ("any depth", "a single
/// event", "no exclusions at all"), which is where `overreaches` already
/// works with numbers behind it. So the constraint is mechanical. The model
/// may drift wherever it likes; a finding naming nothing checkable does not
/// survive, and no prompt wording can make it.
///
/// The path half exists because the first version of this function did not
/// have it, and that was an error of category rather than of evidence. Paths
/// were swept in with "designators" on the strength of the word, not of a
/// measurement. The measurement says the opposite: of 67 surviving findings
/// over the corpus exactly two were path-shaped, both `acks.jsonl`, and
/// **neither was a false positive** — one landed on a claim a later pass
/// refuted and one on a claim that needed work. Nothing in the measured
/// noise carries a `/` or one of these suffixes, so admitting paths buys
/// those two back at no cost the corpus can show.
///
/// Word boundaries by hand, because "one" is inside "money", "none" and
/// "someone", and a substring test would readmit exactly the prose this
/// exists to exclude.
fn is_checkable(literal: &str) -> bool {
    if literal.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    let lower = literal.to_ascii_lowercase();
    if lower.contains('/') || PATH_SUFFIXES.iter().any(|s| lower.contains(s)) {
        return true;
    }
    let bytes = lower.as_bytes();
    NUMBER_WORDS.iter().any(|w| {
        lower.match_indices(w).any(|(at, _)| {
            let before = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
            let end = at + w.len();
            let after = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
            before && after
        })
    })
}

/// The literal check: one call, then three machine filters over what it says.
///
/// The model's job here is the judgement — is this number, path or name
/// asserted as *current* fact, and could a capture have carried it. Every
/// factual component of the finding is decided in code afterwards:
///
///   1. the literal must be verbatim in the author's own text, or the
///      finding points at nothing;
///   2. no observation shown to the model may contain it, because that is
///      the entire assertion, and it is one [`Subject::containing`] already
///      answers exactly.
///
/// Filter 2 is what makes this kind cheap to trust relative to the other
/// two: a `contradicts` finding rests on the model's reading, while an
/// `unevidenced` one rests on a substring search anyone can rerun. It also
/// biases hard toward silence — a literal that occurs incidentally
/// anywhere in the capture is dropped, so `40` inside a line range
/// suppresses a genuine finding about a different `40`. Under-reporting is
/// the right direction for an advisory that costs the author attention.
fn literal_findings(
    key: &str,
    model: &str,
    subject: &Subject,
    started: Instant,
    budget: Duration,
    tel: &mut Telemetry,
) -> Result<Vec<Finding>, Status> {
    // With nothing captured, every literal in the text is trivially
    // unevidenced and `containing` has nothing to search, so the filter that
    // makes this kind worth trusting cannot reject anything. Measured: 37
    // such claims in the corpus raised 506 literals and the filter rejected
    // none of them, flagging 78% of draws. That is a noise generator, and it
    // is also a call worth not paying for. The two disagreement kinds are
    // unaffected — they have a prompt telling them not to report on material
    // they were not shown, and evidence they can still read labels from.
    if subject.evidence.iter().all(|(_, _, obs)| obs.is_empty()) {
        return Ok(Vec::new());
    }
    let prompt = format!("TEXT:\n{}\n\n{}", subject.text, evidence_text(subject));
    let body = call(key, model, LITERALS_SYSTEM, &prompt, started, budget, tel)?;
    let Some(v) = json_object(&body) else {
        tel.detail = Some(format!(
            "literal check replied with no JSON object; {} bytes beginning: {}",
            body.len(),
            body.chars().take(200).collect::<String>()
        ));
        return Err(Status::Unparsable);
    };
    let Some(rows) = v.get("unevidenced").and_then(|u| u.as_array()) else {
        tel.detail = Some("literal check reply had no `unevidenced` array".into());
        return Err(Status::Unparsable);
    };
    let mut out = Vec::new();
    for row in rows {
        let literal = str_field(row, "literal");
        if literal.is_empty() || !subject.text.contains(&literal) {
            tel.not_verbatim += 1;
            continue;
        }
        if !subject.containing(&literal).is_empty() {
            tel.literals_refuted += 1;
            continue;
        }
        if !is_checkable(&literal) {
            tel.not_a_quantity += 1;
            continue;
        }
        let clause = str_field(row, "clause");
        out.push(Finding {
            kind: KIND_UNEVIDENCED.to_string(),
            clause_quoted: !clause.is_empty() && subject.text.contains(&clause),
            clause,
            facts: Vec::new(),
            quoted_from: None,
            legacy_fact: None,
            evidence: None,
            literal: Some(literal),
            why: str_field(row, "why"),
            // Nothing was quoted from the capture, and nothing could be:
            // the finding is that the capture is silent. Distinct from a
            // rejected span, which is a quotation that failed.
            quoted: false,
            rejected_span: None,
        });
    }
    Ok(out)
}

/// The largest brace-delimited substring of a reply, decoded.
///
/// Models wrap the object in prose or a fence often enough that finding it
/// is part of reading the answer rather than a leniency. `None` here always
/// becomes [`Status::Unparsable`] — never a clean bill.
fn json_object(body: &str) -> Option<serde_json::Value> {
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    serde_json::from_str(body.get(start..=end)?).ok()
}

fn str_field(row: &serde_json::Value, name: &str) -> String {
    row.get(name).and_then(|v| v.as_str()).unwrap_or_default().to_string()
}

/// What the model is shown — and it must be exactly what
/// [`crate::facts::Fact::quotes`] can later accept.
///
/// The two have to agree or the quote check punishes honesty. `quotes`
/// searches each observation's captured slice separately and deliberately
/// not the joined whole, because text straddling the seam between two
/// observations was never contiguous in anything anyone looked at. Show
/// the model one joined blob per fact and it can quote across that seam
/// in perfect good faith, whereupon the span is stripped as a fabrication
/// and the "span rejected" count — the very signal `rejected_span` exists
/// to give — fills up with quotations nobody invented. So each
/// observation is presented on its own, under its own label.
fn evidence_text(subject: &Subject) -> String {
    let mut labels = String::new();
    let mut blob = String::new();
    let mut budget = MAX_EVIDENCE_BYTES;
    let mut withheld = 0usize;
    for (id, extent, observations) in &subject.evidence {
        for e in extent {
            labels.push_str(&format!("  - [{id}] {e}\n"));
        }
        for (n, output) in observations.iter().enumerate() {
            blob.push_str(&format!("--- {id} observation {} ---\n", n + 1));
            // Cut on a character boundary, and say what was cut. An
            // undisclosed truncation is worse than a small budget: the
            // prompt forbids reporting a disagreement resting on material
            // the model was not shown, and it can only obey that if the
            // absence is visible.
            let take = if output.len() <= budget {
                output.len()
            } else {
                let mut t = budget;
                while t > 0 && !output.is_char_boundary(t) {
                    t -= 1;
                }
                t
            };
            if take < output.len() {
                withheld += output.len() - take;
                blob.push_str(&output[..take]);
                blob.push_str(&format!(
                    "\n[... {} bytes of captured output not shown]\n",
                    output.len() - take
                ));
            } else {
                blob.push_str(output);
                blob.push('\n');
            }
            budget = budget.saturating_sub(take);
        }
    }
    if withheld > 0 {
        blob.push_str(&format!(
            "\n[{withheld} bytes of captured output withheld in total — this comparison saw a bounded view]\n"
        ));
    }
    format!("EVIDENCE — what was opened or run:\n{labels}\nEVIDENCE — captured output:\n{blob}")
}

fn classify_prompt(subject: &Subject) -> String {
    format!("CLAIM:\n{}", subject.text)
}

fn check_prompt(subject: &Subject, labelled: Option<&str>) -> String {
    match labelled {
        Some(l) => format!(
            "CLAIM:\n{}\n\nASSERTIONS:\n{}\n\n{}",
            subject.text,
            l,
            evidence_text(subject)
        ),
        None => format!("CLAIM:\n{}\n\n{}", subject.text, evidence_text(subject)),
    }
}

// The prompts are the eval's, verbatim in substance: the configuration
// that produced the gate's numbers was classify-then-check with the whole
// claim in view, over the cited facts together with the overlap set.
// Rewording them here would ship something the retrodiction never
// measured.

const CLASSIFY_SYSTEM: &str = r#"You are given one claim from a software design memo. Split it into its separate
assertions and label each.

  current   — asserts how the code, files or tools behave TODAY. Checkable against captured evidence.
  proposed  — asserts what THIS DESIGN will build, add, change, or recommend. The evidence was
              captured before that change exists, so it cannot speak to this.
  argument  — a reason, a decision, an entailment, or a statement about what is right or necessary.
              Nothing captured can settle it.

One sentence often carries more than one assertion, with different labels. Split them.

Quote each assertion VERBATIM from the claim — character for character, never paraphrased, never
merged, never invented. You are only sorting the author's own words.

Reply with one JSON object and nothing else:
{"assertions": [{"text": "", "label": "current"|"proposed"|"argument"}]}"#;

const CHECK_SYSTEM: &str = r#"You are given a claim from a design memo and the evidence a tool
captured for it. Report only DISAGREEMENTS.

A claim mixes three kinds of assertion, and only the first is checkable here:

  current   — asserts how the system behaves TODAY. THESE ARE THE ONLY ONES YOU MAY REPORT AGAINST.
  proposed  — asserts what this design will build. The evidence predates it, so its absence from the
              captured code is expected and is never a finding.
  argument  — a reason, decision or entailment. Nothing captured can settle it.

Read the whole claim, because a contradiction often needs the other kinds for context: a bound the
design *recommends* can be what makes a *current* assertion about byte counts wrong, and you cannot
see that if you only read the current ones. Report only where the failing assertion is a `current`
one.

There are exactly two kinds of disagreement:

  contradicts — the captured evidence shows something incompatible with the assertion: a different
                number, name, type, line, or behaviour.
  overreaches — the assertion ranges wider than what was captured. It says "every", "never", "only",
                "no", "always", "any" or "cannot" about a population the evidence samples rather
                than covers.

Nothing else is a disagreement. In particular:

  * Evidence that does not fully ESTABLISH an assertion is NOT a disagreement. Reporting that is
    noise, not a finding.
  * "The captured material does not touch X" is NOT a disagreement. That is insufficiency phrased
    as a missing scope, and it is still insufficiency.
  * An assertion saying LESS than the evidence shows is not a disagreement.
  * Prose describing code is not a disagreement with the code.
  * Your own uncertainty is not a disagreement.

The captured output may have been truncated, and says so where it was. Never report a disagreement
resting on material you were not shown.

For each disagreement, name the failing assertion and quote the span of captured evidence that shows
it. Both VERBATIM — copied character for character. A finding whose quotation cannot be found in the
evidence is worse than none, because it sends the reader to check against text that does not exist.

Reply with one JSON object and nothing else:
{"disagreements": [{"kind": "contradicts"|"overreaches", "clause": "", "evidence": "", "why": ""}]}

An empty list is the common and correct answer."#;

// The literal check is a separate call with a separate prompt, and that is
// not an accident of layering. `CHECK_SYSTEM` above is the eval's prompt,
// and the precision and recall `docs/verify.md` quotes describe the two
// kinds it returns. Adding a third kind to it would have changed the
// measured configuration, so the numbers on the page would no longer be
// numbers about the thing that shipped. Off by default, its own call, its
// own prompt: what the retrodiction measured stays byte-identical when this
// is off, and when it is on the new kind's accuracy is separately unknown
// rather than blended into a figure that was earned by something else.
const LITERALS_SYSTEM: &str = r#"You are given text from a software design memo and the evidence a tool captured for it. Report
QUANTITIES the text states as current fact that the captured evidence does not carry.

A quantity is a value that could be wrong by counting or arithmetic: a count, a size, a byte or line
count, a duration, a percentage or proportion, a threshold, an index range used as a measurement.

Read for the VALUE, not the spelling. A quantity the evidence carries in another form is carried:

  * `14_000` in the capture backs "14,000 bytes"
  * a capture of lines 1-40 backs "40 lines"
  * `MAX_ATTEMPTS: u32 = 3` backs "retries three times"
  * two timestamps 910 apart back "910 seconds", and CONTRADICT "918 seconds"
  * 5 of 6 visible in the capture backs "83%", where the arithmetic is the author's to do

A FILE THE TEXT SAYS IT READ is also reportable. If the text asserts that a named file or path
carries something, and no capture opened that path, say so — `acks.jsonl`, `src/verify.rs`.

A BARE NAME IS NEITHER, and naming one is the most common way to be wrong here. Never report:

  * a symbol, function, type, module or field name — `look_grep`, `facts::mint`
  * a flag, option or setting name — `--why`, `verify.enabled`
  * a version string — `2.6.0-FreeBSD`
  * an identifier for a ticket, section, check or numbered item — TET-36, "check 5"
  * a line or byte range used to say WHERE something is rather than HOW MUCH — "lines 1-4"
  * a quoted phrase the text is discussing rather than measuring
  * a quantifier — "any", "every", "no", "only", "always". Overreach is someone else's job here.

A name appearing in the text but not in the capture is the ordinary condition of prose that
discusses a system. It is not a finding. Only a quantity or a path is.

Also never report:

  * a quantity in an assertion about what this design WILL build — the evidence predates it
  * a quantity inside a reason, a decision or an entailment
  * a number that measures nothing: "two reasons", "the first of three", "one call"
  * your own uncertainty

The captured output may have been truncated, and says so where it was. A quantity that may lie in
material you were not shown is not a finding.

Quote the quantity VERBATIM from the text, and quote the whole clause containing it VERBATIM.
Character for character, both. A value that cannot be found in the text is worse than none.

Reply with one JSON object and nothing else:
{"unevidenced": [{"literal": "", "clause": "", "why": ""}]}

An empty list is the common and correct answer."#;

// ---------------------------------------------------------------------
// Assembling the subject for each verb.
// ---------------------------------------------------------------------

/// The captured side for a claim: the facts it cites **together with** the
/// overlap set, which is what keeps the author from narrowing the
/// comparison by selection.
pub fn claim_subject(
    dir: &Path,
    id: &str,
    prop: &str,
    cited: &[String],
    overlap: &[(String, Vec<String>)],
) -> io::Result<Subject> {
    let all = facts::load_all(dir)?;
    let mut wanted: Vec<String> = cited.to_vec();
    for (fid, _) in overlap {
        if !wanted.contains(fid) {
            wanted.push(fid.clone());
        }
    }
    Ok(Subject {
        mint: id.to_string(),
        verb: "claim".to_string(),
        text: prop.to_string(),
        evidence: collect(&all, &wanted),
    })
}

/// The captured side for a fact is its own captured output — the one verb
/// where the two sides were never separable.
pub fn fact_subject(dir: &Path, id: &str) -> io::Result<Subject> {
    let all = facts::load_all(dir)?;
    let note = all
        .iter()
        .find(|f| f.id == id)
        .map(|f| f.note.clone())
        .unwrap_or_default();
    Ok(Subject {
        mint: id.to_string(),
        verb: "fact".to_string(),
        text: note,
        evidence: collect(&all, std::slice::from_ref(&id.to_string())),
    })
}

/// The captured side for a prose block: the facts under the claims it
/// cites. The least-evidenced of the three comparisons — neither case
/// file in the eval contains one — which is why the verb is off unless
/// asked for.
pub fn prose_subject(dir: &Path, id: &str, text: &str, cites: &[String]) -> io::Result<Subject> {
    let all_facts = facts::load_all(dir)?;
    let all_claims = crate::claims::load_all(dir)?;
    let mut wanted: Vec<String> = Vec::new();
    for cid in cites {
        if let Some(c) = all_claims.iter().find(|c| &c.id == cid) {
            for f in &c.from {
                if !wanted.contains(f) {
                    wanted.push(f.clone());
                }
            }
        }
    }
    Ok(Subject {
        mint: id.to_string(),
        verb: "prose".to_string(),
        text: text.to_string(),
        evidence: collect(&all_facts, &wanted),
    })
}

fn collect(all: &[facts::Fact], wanted: &[String]) -> Vec<(String, Vec<String>, Vec<String>)> {
    wanted
        .iter()
        .filter_map(|id| all.iter().find(|f| &f.id == id))
        .map(|f| {
            // `observation_outputs` returns None on a record whose
            // boundaries cannot be trusted. Falling back to the joined
            // whole there would put text in front of the model that
            // `quotes` can never accept back, so such a fact contributes
            // nothing rather than something unverifiable.
            let observations = f
                .observation_outputs()
                .map(|obs| obs.into_iter().map(str::to_string).collect())
                .unwrap_or_default();
            (
                f.id.clone(),
                f.extent.iter().map(|e| e.label.clone()).collect(),
                observations,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------
// Reading the log back.
//
// The design ends by saying what this corpus could not settle and what
// would: the verifier's flags recorded beside the graders' verdicts on
// memos written after it ships. That is a join, and a join nobody runs is
// not a measurement. This is the command that runs it.
// ---------------------------------------------------------------------

/// Find the workspace that authored `memo`, by matching the identity its
/// snapshot carries against the identities of the workspaces on this
/// machine.
///
/// There is no record anywhere of what a workspace rendered to — `render`
/// writes the snapshot and keeps no note of it — so the join has to run
/// the other way, from the memo back. `identity.json` is in
/// `SNAPSHOT_FILES` and in the workspace, carrying the same opaque id in
/// both, which makes it the only thing that ties the two together.
pub fn authoring_workspace(memo: &Path) -> Option<(String, PathBuf)> {
    let want = workspace::identity_of(&crate::snapshot::snapshot_path(memo))?;
    for summary in workspace::list().ok()? {
        let dir = workspace::workspace_dir(&summary.name);
        if workspace::identity_of(&dir).as_deref() == Some(want.as_str()) {
            return Some((summary.name, dir));
        }
    }
    None
}

/// Every verification the workspace logged, newest last, with the count
/// of lines that would not parse.
pub fn log_records(workspace_dir: &Path) -> (Vec<Record>, usize) {
    read_log(workspace_dir)
}

/// One claim, as the verifier saw it and as the graders later judged it.
struct Row {
    claim: String,
    flagged: bool,
    /// Flagged by at least one of the two *measured* kinds, as opposed to
    /// by an `unevidenced` finding alone.
    ///
    /// The precision and recall below were earned by `contradicts` and
    /// `overreaches`. Turning `verify.literals` on adds a kind no eval has
    /// scored, and counting its flags into the same fraction would quietly
    /// restate an unmeasured check's accuracy as the measured one's — the
    /// exact contamination that keeping it in a separate call avoids
    /// upstream. The rows stay joined over everything the author actually
    /// saw, which is the honest denominator; this says how much of it is
    /// the new kind.
    disagreed: bool,
    findings: usize,
    /// None when no pass has graded this claim yet — which is not a miss
    /// and not a false positive, and leaves every denominator.
    later: Option<Verdicts>,
}

#[derive(Default)]
struct Verdicts {
    supports: usize,
    qualifies: usize,
    refutes: usize,
}

impl Verdicts {
    fn supports_only(&self) -> bool {
        self.supports > 0 && self.qualifies == 0 && self.refutes == 0
    }
}

/// The report `tetel verify-report` prints.
pub fn report_text(memo: &Path, show_spans: bool) -> io::Result<String> {
    let mut out = String::new();
    out.push_str(&format!("memo         {}\n", memo.display()));

    let Some((name, dir)) = authoring_workspace(memo) else {
        out.push_str(
            "\nNo workspace on this machine matches this memo's snapshot identity, so there is\n\
             nothing to join its ledger against. That is the ordinary state for a memo written\n\
             elsewhere: the verifier's log stays in the workspace and never travels with the\n\
             document.\n",
        );
        return Ok(out);
    };
    out.push_str(&format!("workspace    {name}\n"));

    let (records, unreadable) = log_records(&dir);
    if unreadable > 0 {
        // Said before anything else and never conflated with absence: a
        // log with unreadable lines is a log with data in it, and the
        // counts below are computed over what could be read.
        out.push_str(&format!(
            "\nWARNING: {unreadable} line(s) of the verification log could not be parsed and are\n\
             not counted below. Every number in this report is over the rest.\n"
        ));
    }
    if records.is_empty() {
        out.push_str(if unreadable > 0 {
            "\nNo readable records remain, so there is nothing to join.\n"
        } else {
            "\nThe workspace has no verification log. Either the verifier was never enabled\n\
             here (`tetel config verify.enabled true`), or no verified verb has run since.\n"
        });
        return Ok(out);
    }

    // ---- operational half: what the calls did ----
    let mut by_status: std::collections::BTreeMap<&str, usize> = Default::default();
    for r in &records {
        *by_status.entry(r.status.as_str()).or_default() += 1;
    }
    let cost: f64 = records.iter().map(|r| r.cost).sum();
    let mut times: Vec<u64> = records.iter().map(|r| r.elapsed_ms).collect();
    times.sort_unstable();
    let median = times.get(times.len() / 2).copied().unwrap_or(0);
    let retried =
        records.iter().filter(|r| r.attempts > expected_calls(&r.approach, r.literals)).count();

    out.push_str(&format!("\nVERIFICATIONS   {}\n", records.len()));
    for (status, n) in &by_status {
        out.push_str(&format!("  {status:<14} {n}\n"));
    }
    out.push_str(&format!(
        "\n  cost           {cost:.4} total, {:.5} each\n  elapsed        {median}ms median\n  retried        {retried}\n",
        cost / records.len() as f64,
    ));
    // A failure with no explanation is the thing this field exists to
    // end, so every distinct one is named rather than counted.
    let mut details: Vec<&str> =
        records.iter().filter_map(|r| r.detail.as_deref()).collect();
    details.sort_unstable();
    details.dedup();
    if !details.is_empty() {
        out.push_str("\n  why the non-ok ones failed:\n");
        for d in details {
            out.push_str(&format!("    - {}\n", d.chars().take(160).collect::<String>()));
        }
    }

    out.push_str(&fidelity_text(&records, show_spans));

    // ---- the join: flags against verdicts ----
    let (evidence, _) = crate::evidence::load(memo)?;
    let mut verdicts: std::collections::BTreeMap<String, Verdicts> = Default::default();
    for e in &evidence {
        let v = verdicts.entry(e.claim_id.clone()).or_default();
        match e.verdict {
            crate::evidence::Verdict::Supports => v.supports += 1,
            crate::evidence::Verdict::Qualifies => v.qualifies += 1,
            crate::evidence::Verdict::Refutes => v.refutes += 1,
        }
    }

    // One row per claim the verifier actually looked at. A claim verified
    // more than once (a revision is a new comparison) counts as flagged
    // if any of its verifications flagged it, which is the reading that
    // matches what an author saw.
    let mut rows: std::collections::BTreeMap<String, Row> = Default::default();
    for r in records.iter().filter(|r| r.status == "ok" && r.verb == "claim") {
        let row = rows.entry(r.mint.clone()).or_insert_with(|| Row {
            claim: r.mint.clone(),
            flagged: false,
            disagreed: false,
            findings: 0,
            later: None,
        });
        row.flagged |= !r.findings.is_empty();
        row.disagreed |= r.findings.iter().any(Finding::quotes_evidence);
        row.findings += r.findings.len();
    }
    for row in rows.values_mut() {
        row.later = verdicts.remove(&row.claim).map(|v| v);
    }

    let graded: Vec<&Row> = rows.values().filter(|r| r.later.is_some()).collect();
    let ungraded = rows.len() - graded.len();
    let flagged: Vec<&&Row> = graded.iter().filter(|r| r.flagged).collect();
    let sound_flagged = flagged
        .iter()
        .filter(|r| r.later.as_ref().is_some_and(|v| v.supports_only()))
        .count();
    let worked_flagged = flagged.len() - sound_flagged;
    let ever_worked = graded
        .iter()
        .filter(|r| r.later.as_ref().is_some_and(|v| !v.supports_only()))
        .count();
    let missed_refutes = graded
        .iter()
        .filter(|r| !r.flagged && r.later.as_ref().is_some_and(|v| v.refutes > 0))
        .count();

    out.push_str(&format!(
        "\nFLAGS AGAINST WHAT THE GRADERS LATER SAID\n  claims verified  {}\n  of those graded  {}   (ungraded so far: {ungraded}, entering no denominator)\n",
        rows.len(),
        graded.len()
    ));
    if graded.is_empty() {
        out.push_str(
            "\n  Nothing has been graded yet, so there is no ground truth to join against.\n\
             Run a grounding pass and ask again.\n",
        );
        return Ok(out);
    }
    out.push_str(&format!(
        "  flagged          {}\n    later needed work  {worked_flagged}\n    later only supported  {sound_flagged}   <- flags on claims that were already sound\n  never flagged, later refuted   {missed_refutes}   <- what it did not catch\n",
        flagged.len()
    ));
    if !flagged.is_empty() {
        out.push_str(&format!(
            "\n  precision        {:.0}%   ({worked_flagged}/{})\n",
            100.0 * worked_flagged as f64 / flagged.len() as f64,
            flagged.len()
        ));
    }
    if ever_worked > 0 {
        out.push_str(&format!(
            "  recall           {:.0}%   ({worked_flagged}/{ever_worked})\n",
            100.0 * worked_flagged as f64 / ever_worked as f64
        ));
    }
    // Said only when it is true, and then said plainly. The two fractions
    // above describe the kinds an eval scored; a claim flagged solely by
    // the literal check is inside them without any of that behind it.
    let literal_only = flagged.iter().filter(|r| !r.disagreed).count();
    if literal_only > 0 {
        out.push_str(&format!(
            "\n  {literal_only} of those {} flags came only from `unevidenced` findings, a kind no\n  \
             evaluation has scored. The two fractions above were earned by `contradicts`\n  \
             and `overreaches`; read them knowing that.\n",
            flagged.len()
        ));
    }

    Ok(out)
}

/// The fidelity half of the report: how faithfully the model quoted the
/// two sides it was told to quote, and what was dropped before anything
/// reached the author.
///
/// Split out and called *before* the ledger join, because it needs no
/// ledger. It used to sit after it, behind the early return for a memo
/// nobody has graded yet — so the numbers that exist from the very first
/// verification were withheld until a grounding pass had run, which is
/// precisely the period when you are deciding whether the settings are
/// right.
fn fidelity_text(records: &[Record], show_spans: bool) -> String {
    let mut out = String::new();
    let all: Vec<&Finding> = records.iter().flat_map(|r| r.findings.iter()).collect();
    // Scored over the kinds that quote the captured side. An `unevidenced`
    // finding is `quoted: false` by construction — its whole content is
    // that the capture holds nothing to quote — so counting it here would
    // read as a fidelity collapse the moment the setting was turned on.
    let evidential: Vec<&&Finding> = all.iter().filter(|f| f.quotes_evidence()).collect();
    let quoted = evidential.iter().filter(|f| f.quoted).count();
    let rejected: Vec<&&&Finding> = evidential.iter().filter(|f| f.rejected_span.is_some()).collect();
    let unevidenced = all.len() - evidential.len();
    let ambiguous = evidential.iter().filter(|f| f.facts.len() > 1).count();
    let clause_ok = all.iter().filter(|f| f.clause_quoted).count();
    out.push_str(&format!("\nQUOTATIONS\n  findings         {}\n", all.len()));
    if !evidential.is_empty() {
        out.push_str(&format!(
            "  quoted verbatim  {quoted}   ({:.0}% of {} evidence-bearing)\n  span rejected    {}\n  span in >1 fact  {ambiguous}   <- attributed to all of them, not the first\n",
            100.0 * quoted as f64 / evidential.len() as f64,
            evidential.len(),
            rejected.len(),
        ));
    }
    if !all.is_empty() {
        // The other half of the same discipline. Both prompts demand the
        // author's own words back verbatim; for a long time only the
        // captured side was checked, so this number did not exist and its
        // absence looked like a clean one.
        out.push_str(&format!(
            "  clause verbatim  {clause_ok}   ({:.0}% of all findings)\n",
            100.0 * clause_ok as f64 / all.len() as f64
        ));
    }
    // What never reached a finding at all. Dropped material is the half of
    // the fidelity picture the findings themselves cannot show, and a drop
    // nobody can count is the same silence a deleted `rejected_span` would
    // have been.
    let not_verbatim: u32 = records.iter().map(|r| r.not_verbatim).sum();
    let refuted: u32 = records.iter().map(|r| r.literals_refuted).sum();
    if not_verbatim > 0 {
        out.push_str(&format!(
            "  dropped, not the author's words   {not_verbatim}   <- returned as a quotation, absent from the text\n"
        ));
    }
    let not_quantity: u32 = records.iter().map(|r| r.not_a_quantity).sum();
    let raised = unevidenced + refuted as usize + not_quantity as usize;
    if raised > 0 {
        out.push_str(&format!(
            "\nLITERALS\n  unevidenced      {unevidenced}   <- stated as current fact, in no capture\n  \
             machine-refuted  {refuted}   <- the literal was in the capture after all\n  \
             not a quantity   {not_quantity}   <- a name, flag or quantifier, not a countable value\n  \
             {:.0}% of what it raised was dropped by a check anyone can rerun\n",
            100.0 * (refuted + not_quantity) as f64 / raised as f64
        ));
    }
    if show_spans {
        for f in &rejected {
            out.push_str(&format!(
                "\n  [{}] {}\n    the model offered: {}\n",
                if f.facts.is_empty() { "no fact contained it".to_string() } else { f.facts.join(", ") },
                f.clause.chars().take(120).collect::<String>(),
                f.rejected_span
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(200)
                    .collect::<String>()
            ));
        }
    } else if !rejected.is_empty() {
        out.push_str("  (--spans prints the spans that failed verification)\n");
    }


    out
}

/// How many calls an approach makes when nothing is retried, so a retry
/// can be counted rather than inferred.
fn expected_calls(approach: &str, literals: bool) -> u32 {
    let base = match approach {
        "split" => 2,
        _ => 1,
    };
    base + u32::from(literals)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_fixture() -> Settings {
        Settings {
            enabled: true,
            model: Some("openai/gpt-5.6-luna".into()),
            approach: "direct".into(),
            timeout_ms: 60_000,
            verbs: vec!["claim".into()],
            literals: false,
        }
    }

    fn finding_fixture() -> Finding {
        Finding {
            kind: KIND_CONTRADICTS.into(),
            clause: "the function returns early".into(),
            clause_quoted: true,
            facts: vec!["F1".into()],
            quoted_from: Some("output".into()),
            legacy_fact: None,
            evidence: Some("return".into()),
            literal: None,
            why: "it does not".into(),
            quoted: true,
            rejected_span: None,
        }
    }

    fn subject_fixture(text: &str, evidence: &[(&str, &[&str])]) -> Subject {
        Subject {
            mint: "C1".into(),
            verb: "claim".into(),
            text: text.into(),
            evidence: evidence
                .iter()
                .map(|(id, obs)| {
                    ((*id).to_string(), Vec::new(), obs.iter().map(|o| (*o).to_string()).collect())
                })
                .collect(),
        }
    }

    #[test]
    fn every_status_has_a_word_and_they_are_distinct() {
        // Hand-typed, so it is worth saying what keeps it honest: the
        // count below is asserted, and `as_str`'s own match is
        // exhaustive, so a new variant fails to compile there and fails
        // the count here.
        let all = [
            Status::Off,
            Status::Unauthorized,
            Status::Queued,
            Status::Skipped,
            Status::Ok,
            Status::Unavailable,
            Status::Timeout,
            Status::Unparsable,
        ];
        assert_eq!(all.len(), 8, "a status was added without being listed here");
        let mut words: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
        words.sort_unstable();
        let before = words.len();
        words.dedup();
        assert_eq!(before, words.len(), "two statuses share a word");
    }

    #[test]
    fn a_block_with_nothing_delivered_carries_no_findings_key() {
        // The whole reason `verify` is an object: "found nothing" and
        // "did not look" must not be the same payload.
        let b = block(&settings_fixture(), "claim", None, Trigger::Queued("C7"));
        assert_eq!(b["status"], "queued");
        assert!(b.get("findings").is_none(), "{b}");
        assert_eq!(b["queued_for"], "C7");
        assert_eq!(b["deterministic"], false);
    }

    #[test]
    fn every_block_echoes_all_six_settings() {
        // `config.rs` admits a key only if it is visible in the output it
        // affects. Four of the six are invisible without this echo —
        // `literals` most of all, since with it off the author sees
        // findings of two kinds and nothing saying a third exists.
        for delivered in [None, Some(&record_fixture())] {
            let b = block(&settings_fixture(), "claim", delivered, Trigger::NotAttempted);
            for key in ["model", "approach", "timeout_ms", "verbs", "literals"] {
                assert!(b.get(key).is_some(), "{key} missing from {b}");
            }
            assert_eq!(b["deterministic"], false, "{b}");
        }
    }

    fn record_fixture() -> Record {
        Record {
            seq: 1,
            mint: "C3".into(),
            verb: "claim".into(),
            status: "ok".into(),
            model: "openai/gpt-5.6-luna".into(),
            approach: "split".into(),
            literals: false,
            literals_status: None,
            not_verbatim: 0,
            literals_refuted: 0,
            not_a_quantity: 0,
            findings: Vec::new(),
            at: 0,
            cost: 0.0,
            elapsed_ms: 0,
            attempts: 1,
            detail: None,
        }
    }

    #[test]
    fn a_delivered_block_names_the_mint_it_is_about() {
        // It is no longer the id sitting beside the findings.
        let b = block(&settings_fixture(), "claim", Some(&record_fixture()), Trigger::Queued("C4"));
        assert_eq!(b["status"], "ok");
        assert_eq!(b["for_mint"], "C3");
        assert_eq!(b["queued_for"], "C4");
        assert!(b.get("findings").is_some());
    }

    #[test]
    fn one_unreadable_line_does_not_stop_every_future_delivery() {
        // The strict reader fails a whole file on one bad line. Here that
        // would mean a single truncated append — a crash mid-write —
        // silently ending deliveries forever, with `verify-report`
        // announcing the verifier had never been enabled.
        let dir = std::env::temp_dir().join(format!("tetel-verify-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let good = serde_json::to_string(&record_fixture()).unwrap();
        std::fs::write(log_path(&dir), format!("{good}\n{{\"seq\":2,\"mint\":\n{good}\n")).unwrap();

        let (records, skipped) = read_log(&dir);
        assert_eq!(records.len(), 2, "readable records were lost");
        assert_eq!(skipped, 1, "the unreadable line was not counted");
        assert!(peek_delivered(&dir).is_some(), "deliveries stopped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn committing_a_delivery_never_moves_the_cursor_backwards_or_past_a_record() {
        // Two authoring calls can interleave: both peek position N, and
        // a blind `count + 1` from each lands the cursor at N+2, skipping
        // the record at N+1 — which may be the one carrying a finding.
        let dir = std::env::temp_dir().join(format!("tetel-verify-cur-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        commit_delivered(&dir, 0);
        commit_delivered(&dir, 0); // the second of two interleaved calls
        assert_eq!(delivered_count(&dir), 1, "a record was skipped");
        commit_delivered(&dir, 4);
        commit_delivered(&dir, 1); // a straggler must not rewind
        assert_eq!(delivered_count(&dir), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rejected_span_is_kept_in_the_log_and_withheld_from_the_author() {
        let f = Finding {
            evidence: None,
            facts: Vec::new(),
            quoted_from: None,
            why: "invented".into(),
            quoted: false,
            rejected_span: Some("text that is in no captured output".into()),
            ..finding_fixture()
        };
        // The author must not be sent to check against text that does not
        // exist...
        let payload = f.payload();
        assert_eq!(payload["quoted"], false);
        assert!(payload.get("evidence").is_none(), "{payload}");
        assert!(
            !payload.to_string().contains("text that is in no captured output"),
            "the fabricated span reached the author: {payload}"
        );
        // ...and whoever is tuning the verifier must still be able to
        // read it, or a fabrication rate is a number with nothing behind
        // it.
        let logged = serde_json::to_value(&f).expect("record serialises");
        assert_eq!(logged["rejected_span"], "text that is in no captured output");
    }

    #[test]
    fn a_clean_finding_carries_its_span_to_the_author() {
        let f = Finding {
            kind: KIND_OVERREACHES.into(),
            clause: "every call site".into(),
            facts: vec!["F2".into()],
            evidence: Some("fn a() {}".into()),
            why: "one file was opened".into(),
            quoted: true,
            ..finding_fixture()
        };
        let payload = f.payload();
        assert_eq!(payload["evidence"], "fn a() {}");
        assert_eq!(payload["quoted"], true);
        assert_eq!(payload["facts"], json!(["F2"]));
        assert!(payload.get("rejected_span").is_none());
    }

    #[test]
    fn a_delivered_failure_carries_no_findings_key() {
        // The whole reason `verify` is an object. A 429 produces a record
        // with an empty `findings`, and emitting that beside
        // `"status":"unavailable"` hands a caller the empty disagreement
        // list it would read as a clean bill.
        for status in ["unavailable", "timeout", "unparsable"] {
            let mut r = record_fixture();
            r.status = status.into();
            let b = block(&settings_fixture(), "claim", Some(&r), Trigger::NotAttempted);
            assert_eq!(b["status"], status);
            assert!(b.get("findings").is_none(), "{status} carried findings: {b}");
            // The mint is still named — the reader has to know which
            // comparison failed.
            assert_eq!(b["for_mint"], "C3", "{b}");
        }
        // And `ok` still carries them, including when empty.
        let b = block(&settings_fixture(), "claim", Some(&record_fixture()), Trigger::NotAttempted);
        assert_eq!(b["status"], "ok");
        assert_eq!(b["findings"], serde_json::json!([]));
    }

    #[test]
    fn a_call_with_nothing_to_compare_is_skipped_not_off() {
        // With the verb on, a heading or an uncited block must not answer
        // "off" — that tells an author who has just enabled the feature
        // that it is disabled.
        let b = block(&settings_fixture(), "claim", None, Trigger::NothingToCompare);
        assert_eq!(b["status"], "skipped", "{b}");
        assert!(b.get("queued_for").is_none(), "{b}");
    }

    #[test]
    fn an_enabled_verb_with_no_credential_says_so_rather_than_queueing() {
        // The failure this replaces: `queued` reported for a verification
        // that never started, so the author polls for a finding that can
        // never arrive. `unauthorized` was unreachable at the same time,
        // which is how the two defects hid each other.
        let mut s = settings_fixture();
        s.model = None; // stands in for "nothing to call with"
        let b = block(&s, "claim", None, Trigger::NotAttempted);
        assert_eq!(b["status"], "unauthorized", "{b}");
        assert!(b.get("queued_for").is_none(), "{b}");
    }

    #[test]
    fn a_verb_outside_the_verb_list_is_off_not_unauthorized() {
        // `verbs` is `claim` alone in the fixture, so `prose` is off even
        // with the feature enabled — the switch, the verb list and a
        // missing credential are three different answers to "why am I
        // getting nothing".
        let b = block(&settings_fixture(), "prose", None, Trigger::NotAttempted);
        assert_eq!(b["status"], "off", "{b}");
    }

    #[test]
    fn an_unreadable_reply_is_not_a_clean_bill() {
        let subject = Subject {
            mint: "C1".into(),
            verb: "claim".into(),
            text: "x".into(),
            evidence: vec![("F1".into(), vec![], vec!["captured".to_string()])],
        };
        assert!(parse_findings("no braces here", &subject).is_none());
        assert!(parse_findings("{not json}", &subject).is_none());
        assert!(parse_findings(r#"{"other": []}"#, &subject).is_none());
        // A verdict outside the vocabulary fails the whole reply rather
        // than being dropped to an empty list.
        assert!(parse_findings(
            r#"{"disagreements":[{"kind":"unsure","clause":"c","evidence":"e","why":"w"}]}"#,
            &subject
        )
        .is_none());
        // The common, correct answer.
        let empty = parse_findings(r#"{"disagreements": []}"#, &subject).expect("parsed");
        assert!(empty.is_empty());
    }

    #[test]
    fn a_large_capture_is_bounded_and_says_that_it_was() {
        // Unbounded, the input would be a different thing from the one
        // the cost and accuracy figures were measured on — and a big
        // enough fact would overrun the model's context into a truncated
        // draw, three retries and triple the spend.
        let big = "x".repeat(MAX_EVIDENCE_BYTES * 2);
        let subject = Subject {
            mint: "C1".into(),
            verb: "claim".into(),
            text: "a claim".into(),
            evidence: vec![("F1".into(), vec!["big.txt".into()], vec![big])],
        };
        let text = evidence_text(&subject);
        assert!(text.len() < MAX_EVIDENCE_BYTES * 2, "the bound did not apply: {} bytes", text.len());
        // Disclosed, not silent: the prompt forbids reporting a
        // disagreement resting on material the model was not shown, and
        // it can only obey that if the absence is visible.
        assert!(text.contains("bytes of captured output not shown"), "truncation was silent");
        assert!(text.contains("withheld in total"), "no summary of what was withheld");
    }

    #[test]
    fn a_capture_that_fits_is_sent_whole_and_unmarked() {
        let subject = Subject {
            mint: "C1".into(),
            verb: "claim".into(),
            text: "a claim".into(),
            evidence: vec![("F1".into(), vec![], vec!["fn a() {}".into()])],
        };
        let text = evidence_text(&subject);
        assert!(text.contains("fn a() {}"));
        assert!(!text.contains("not shown"), "an untruncated capture claimed truncation");
    }

    #[test]
    fn a_span_is_attributed_to_the_fact_that_contains_it() {
        let subject = subject_fixture("x", &[("F1", &["alpha"]), ("F2", &["beta"])]);
        let f = parse_findings(
            r#"{"disagreements":[{"kind":"contradicts","clause":"c","evidence":"beta","why":"w"}]}"#,
            &subject,
        )
        .expect("parsed");
        assert_eq!(f[0].facts, vec!["F2".to_string()]);
        assert!(f[0].quoted);
    }

    #[test]
    fn a_span_copied_from_the_extent_block_is_a_quotation_and_not_a_fabrication() {
        // The model is shown the extent labels under "what was opened or
        // run" and told to quote the captured evidence. Searching only the
        // observations then stripped what it had honestly copied: measured
        // over 123 real fact notes, 15 of the 25 spans called fabrications
        // were verbatim in that block. A fabrication rate more than twice
        // the real one is worse than none, because it is the number
        // `rejected_span` exists to be tuned on.
        let subject = Subject {
            mint: "F1".into(),
            verb: "fact".into(),
            text: "the search covered every file".into(),
            evidence: vec![(
                "F1".into(),
                vec!["search: /repo (grep: look_grep) — 10 files matched".into()],
                vec!["fn look_grep() {}".into()],
            )],
        };
        let f = parse_findings(
            r#"{"disagreements":[{"kind":"overreaches","clause":"every file","evidence":"10 files matched","why":"w"}]}"#,
            &subject,
        )
        .expect("parsed");
        assert!(f[0].quoted, "a span copied from the extent block was called a fabrication");
        assert_eq!(f[0].facts, vec!["F1".to_string()]);
        assert_eq!(f[0].quoted_from.as_deref(), Some("extent"));
        assert_eq!(f[0].payload()["quoted_from"], "extent");

        // An output span still reports as one, so the two are told apart
        // rather than merged — `overreaches` usually wants the extent and
        // `contradicts` usually wants the output.
        let f = parse_findings(
            r#"{"disagreements":[{"kind":"contradicts","clause":"every file","evidence":"fn look_grep","why":"w"}]}"#,
            &subject,
        )
        .expect("parsed");
        assert_eq!(f[0].quoted_from.as_deref(), Some("output"));

        // And `Fact::quotes` is untouched: it stays the output-only
        // relation `transplant` refuses premises with, because a premise is
        // the donor's own words and an extent label is not.
        let fact = crate::facts::Fact {
            id: "F1".into(),
            note: String::new(),
            extent: Vec::new(),
            output: "fn look_grep() {}".into(),
            pin: String::new(),
            revisions: 0,
        };
        assert!(!fact.quotes("10 files matched"));
    }

    #[test]
    fn a_span_living_in_two_captures_names_both_rather_than_the_first() {
        // The defect this replaced: attribution took whichever fact came
        // first in the cite list, so a short or common span — a number, a
        // path, an identifier — sent the author to a fact chosen by the
        // order they happened to type `--cites` in.
        let subject = subject_fixture("x", &[("F1", &["shared token"]), ("F2", &["shared token"])]);
        let f = parse_findings(
            r#"{"disagreements":[{"kind":"contradicts","clause":"c","evidence":"shared","why":"w"}]}"#,
            &subject,
        )
        .expect("parsed");
        assert_eq!(f[0].facts, vec!["F1".to_string(), "F2".to_string()]);
    }

    #[test]
    fn a_span_in_no_capture_is_attributed_to_nothing_at_all() {
        // And specifically not to the first fact, which is what made a
        // placeholder attribution indistinguishable from a real one in the
        // payload.
        let subject = subject_fixture("x", &[("F1", &["alpha"])]);
        let f = parse_findings(
            r#"{"disagreements":[{"kind":"contradicts","clause":"c","evidence":"gamma","why":"w"}]}"#,
            &subject,
        )
        .expect("parsed");
        assert!(f[0].facts.is_empty(), "{:?}", f[0].facts);
        assert!(!f[0].quoted);
        assert_eq!(f[0].rejected_span.as_deref(), Some("gamma"));
        assert!(f[0].payload().get("evidence").is_none());
    }

    #[test]
    fn a_clause_the_author_never_wrote_is_marked_as_not_theirs() {
        // The other half of "both VERBATIM". Reported rather than
        // withheld: the clause points at the author's own visible text, so
        // a paraphrase is still a usable pointer.
        let subject = subject_fixture("the parser is recursive", &[("F1", &["alpha"])]);
        let f = parse_findings(
            r#"{"disagreements":[
                 {"kind":"contradicts","clause":"the parser is recursive","evidence":"alpha","why":"w"},
                 {"kind":"contradicts","clause":"the parser uses recursion","evidence":"alpha","why":"w"}]}"#,
            &subject,
        )
        .expect("parsed");
        assert!(f[0].clause_quoted, "a verbatim clause was marked as a paraphrase");
        assert!(!f[1].clause_quoted, "a paraphrase was passed off as a quotation");
        assert_eq!(f[1].payload()["clause_quoted"], false);
        assert_eq!(f[1].payload()["clause"], "the parser uses recursion");
    }

    #[test]
    fn a_classify_reply_that_paraphrases_the_claim_drops_the_paraphrase() {
        let mut tel = Telemetry::default();
        let canonical = parse_assertions(
            r#"{"assertions":[{"text":"the cache is warm","label":"current"},
                              {"text":"the cache gets warmed","label":"current"}]}"#,
            "the cache is warm and that is why it is fast",
            &mut tel,
        )
        .expect("one assertion survived");
        assert!(canonical.contains("the cache is warm"));
        assert!(!canonical.contains("gets warmed"), "{canonical}");
        assert_eq!(tel.not_verbatim, 1);
    }

    #[test]
    fn a_classify_reply_that_is_not_an_answer_fails_rather_than_becoming_direct() {
        // The old behaviour forwarded whatever came back into the check
        // prompt, so `split` degraded to `direct`-plus-noise silently.
        let mut tel = Telemetry::default();
        for body in [
            "I cannot help with that.",
            r#"{"assertions":[{"text":"nowhere in the claim","label":"current"}]}"#,
            r#"{"assertions":[{"text":"the cache is warm","label":"speculative"}]}"#,
            r#"{"result":"ok"}"#,
        ] {
            assert!(
                parse_assertions(body, "the cache is warm", &mut tel).is_err(),
                "accepted a non-answer: {body}"
            );
        }
    }

    #[test]
    fn a_classify_reply_that_answers_survives_decoding() {
        let mut tel = Telemetry::default();
        let canonical = parse_assertions(
            r#"Here you go: {"assertions":[{"text":"the cache is warm","label":"current"}]}"#,
            "the cache is warm",
            &mut tel,
        )
        .expect("decoded");
        let v: serde_json::Value = serde_json::from_str(&canonical).expect("re-emitted as JSON");
        assert_eq!(v["assertions"][0]["label"], "current");
        assert_eq!(tel.not_verbatim, 0);
    }

    #[test]
    fn an_unevidenced_finding_quotes_the_authors_literal_and_no_capture() {
        let f = Finding {
            kind: KIND_UNEVIDENCED.into(),
            clause: "the buffer is 4096 bytes".into(),
            facts: Vec::new(),
            quoted_from: None,
            evidence: None,
            literal: Some("4096".into()),
            quoted: false,
            ..finding_fixture()
        };
        let payload = f.payload();
        assert_eq!(payload["literal"], "4096");
        assert_eq!(payload["kind"], "unevidenced");
        assert!(payload.get("evidence").is_none());
        // It must not be scored as a failed quotation: nothing was quoted
        // from the capture because the finding is that there is nothing
        // there to quote.
        assert!(!f.quotes_evidence());
        assert!(finding_fixture().quotes_evidence());
    }

    #[test]
    fn a_legacy_log_line_keeps_its_attribution_instead_of_becoming_unreadable() {
        // `fact` predates `facts`. A log written before the change must
        // still be readable, or the report announces the whole history as
        // unparsable lines — which is exactly the history it exists for.
        let dir = std::env::temp_dir().join(format!("tetel-verify-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let line = r#"{"seq":1,"mint":"C3","verb":"claim","status":"ok","model":"m","approach":"split","findings":[{"kind":"contradicts","clause":"c","fact":"F7","why":"w","quoted":true,"evidence":"e"}],"at":0}"#;
        std::fs::write(log_path(&dir), format!("{line}\n")).unwrap();
        let (records, skipped) = read_log(&dir);
        assert_eq!(skipped, 0, "a legacy line was dropped");
        assert_eq!(records[0].findings[0].facts, vec!["F7".to_string()]);
        // And it is never written back out under the old name.
        let round = serde_json::to_string(&records[0]).unwrap();
        assert!(!round.contains(r#""fact":"#), "{round}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_literal_leg_keeps_the_disagreement_findings_and_says_so() {
        // It used to discard them. Combined over the corpus the two
        // disagreement kinds carry 30% recall against this one's 13%, so
        // losing the stronger half to the weaker half's 429 is the wrong
        // trade — but silently keeping them would be worse, because "found
        // no literals" and "never asked" would become one payload.
        let mut r = record_fixture();
        r.literals = true;
        r.findings = vec![finding_fixture()];
        r.literals_status = Some(Status::Timeout.as_str().to_string());
        let b = block(&settings_fixture(), "claim", Some(&r), Trigger::NotAttempted);

        assert_eq!(b["status"], "ok", "a failed literal leg must not fail the verification");
        assert_eq!(
            b["findings"].as_array().map(Vec::len),
            Some(1),
            "the disagreement findings were discarded: {b}"
        );
        assert_eq!(b["literals_incomplete"], "timeout", "{b}");

        // And a run whose literal leg finished says nothing, so the marker
        // means what it says rather than appearing on every reply.
        let mut ok = record_fixture();
        ok.literals = true;
        ok.findings = vec![finding_fixture()];
        let b = block(&settings_fixture(), "claim", Some(&ok), Trigger::NotAttempted);
        assert!(b.get("literals_incomplete").is_none(), "{b}");
    }

    #[test]
    fn the_default_budget_grows_with_the_number_of_calls_it_has_to_cover() {
        // A flat budget silently means three different things. One call's
        // p90 over the corpus is around 50 seconds, so the flat 60,000 this
        // replaced left a two-call run no headroom and a three-call run
        // none at all.
        let per_leg = DEFAULT_MS_PER_LEG;
        assert_eq!(per_leg * u64::from(expected_calls("direct", false)), per_leg);
        assert_eq!(per_leg * u64::from(expected_calls("split", false)), per_leg * 2);
        assert_eq!(per_leg * u64::from(expected_calls("split", true)), per_leg * 3);
    }

    #[test]
    fn the_literal_check_adds_a_call_that_a_retry_count_must_not_mistake() {
        assert_eq!(expected_calls("split", false), 2);
        assert_eq!(expected_calls("split", true), 3);
        assert_eq!(expected_calls("direct", false), 1);
        assert_eq!(expected_calls("direct", true), 2);
    }

    #[test]
    fn a_checkable_literal_is_a_quantity_or_a_path_and_a_bare_name_is_neither() {
        // The measured noise, every one of which the corpus produced. Note
        // what is *not* in this list: no path is here, because the corpus
        // produced no path-shaped false positive.
        for name in [
            "look_grep", "facts::mint", "--why", "clean working tree",
            "\"explicitly named\"", "any depth", "nowhere", "a single character",
            "no exclusions at all", "every cited claim's digest",
        ] {
            assert!(!is_checkable(name), "`{name}` was let through");
        }
        // Quantities worth keeping.
        for q in ["918 seconds", "28%", "four", "one unit test", "14,000 bytes", "1-4", "TET-36"] {
            assert!(is_checkable(q), "`{q}` was dropped as a name");
        }
        // Paths, which an earlier version of this filter excluded by
        // category rather than by measurement. Both corpus instances of a
        // path-shaped finding landed on a claim that later needed work.
        for p in ["acks.jsonl", "`acks.jsonl`", "src/verify.rs", "docs/design", "Cargo.toml"] {
            assert!(is_checkable(p), "`{p}` was dropped, and the corpus does not justify that");
        }
    }

    #[test]
    fn a_number_word_inside_another_word_is_not_a_quantity() {
        // The trap a substring test walks into: every one of these contains
        // a number word and none of them counts anything. This is why the
        // filter scans for word boundaries by hand rather than calling
        // `contains`.
        for s in ["money", "none", "someone", "atone", "tensor", "often", "shone", "sixty-fourth"] {
            assert!(!is_checkable(s), "`{s}` matched a number word inside another word");
        }
        // Boundaries that are not whitespace still count.
        for s in ["(four)", "one-shot", "up to twelve.", "TWO"] {
            assert!(is_checkable(s), "`{s}` should be a quantity");
        }
    }

    #[test]
    fn the_two_measured_prompts_are_untouched_by_the_literal_check() {
        // The gate's precision and recall describe `CHECK_SYSTEM`'s two
        // kinds. If the third kind ever appears in that prompt, the
        // numbers in `docs/verify.md` stop being numbers about the thing
        // that produced them.
        assert!(!CHECK_SYSTEM.contains(KIND_UNEVIDENCED), "the measured prompt grew a third kind");
        assert!(!CLASSIFY_SYSTEM.contains(KIND_UNEVIDENCED));
        assert!(LITERALS_SYSTEM.contains("unevidenced"));
    }
}
