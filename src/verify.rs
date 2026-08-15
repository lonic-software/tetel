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
    /// `contradicts` or `overreaches` — the only two kinds. Anything else
    /// the model returns makes the whole reply [`Status::Unparsable`].
    pub kind: String,
    /// The author's own clause being judged.
    pub clause: String,
    /// The fact whose captured output it was judged against.
    pub fact: String,
    /// The captured span, present only when
    /// [`crate::facts::Fact::quotes`] accepted it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// Why the two disagree, in the model's words.
    pub why: String,
    /// False when a span was offered and rejected — said in the payload
    /// rather than left for the reader to notice an absence.
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
            "fact": self.fact,
            "why": self.why,
            "quoted": self.quoted,
        });
        if let Some(e) = &self.evidence {
            out["evidence"] = json!(e);
        }
        out
    }
}

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
}

pub fn settings(workspace_dir: &Path) -> Settings {
    let d = Some(workspace_dir);
    Settings {
        enabled: config::verify_enabled(d),
        model: config::verify_model(d),
        approach: config::verify_approach(d),
        timeout_ms: config::verify_timeout_ms(d),
        verbs: config::verify_verbs(d),
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
against what the tool captured, and the two look inconsistent to it. Read the quoted \
evidence and decide: fix the wording, look at something you have not opened, or leave \
it alone because the finding is wrong. It is wrong a meaningful fraction of the time.";

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
            Ok(r) => records.push(r),
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
    let budget = Duration::from_millis(settings.timeout_ms);
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut tel = Telemetry::default();
        let (status, findings) = run(&key, &model, &approach, &subject, started, budget, &mut tel);
        let record = Record {
            seq: next_seq(&dir),
            mint: subject.mint.clone(),
            verb: subject.verb.clone(),
            status: status.as_str().to_string(),
            model,
            approach,
            findings: verified_quotes(&dir, findings),
            at: workspace::now_unix(),
            cost: tel.cost,
            elapsed_ms: started.elapsed().as_millis() as u64,
            attempts: tel.attempts,
            // Only when something went wrong: a clean run has nothing to
            // explain, and a detail line on every record would train a
            // reader to skip the field.
            detail: if status == Status::Ok { None } else { tel.detail },
        };
        let _ = workspace::append_jsonl(&log_path(&dir), &record);
    });
    true
}

/// Put every finding's quoted span through the same relation `transplant`
/// uses, and downgrade the ones it rejects.
fn verified_quotes(dir: &Path, findings: Vec<Finding>) -> Vec<Finding> {
    let facts = facts::load_all(dir).unwrap_or_default();
    findings
        .into_iter()
        .map(|mut f| {
            let accepted = f
                .evidence
                .as_deref()
                .filter(|span| !span.is_empty())
                .is_some_and(|span| {
                    facts
                        .iter()
                        .find(|fact| fact.id == f.fact)
                        .is_some_and(|fact| fact.quotes(span))
                });
            if !accepted {
                // Moved, not deleted: withheld from the author, kept for
                // whoever later asks how often the model invents a
                // quotation.
                f.rejected_span = f.evidence.take().filter(|s| !s.is_empty());
                f.quoted = false;
            } else {
                f.quoted = true;
            }
            f
        })
        .collect()
}

/// What one verification spent getting to its answer, accumulated across
/// however many calls the approach and the retries required.
#[derive(Default)]
pub struct Telemetry {
    pub cost: f64,
    pub attempts: u32,
    pub detail: Option<String>,
}

fn run(
    key: &str,
    model: &str,
    approach: &str,
    subject: &Subject,
    started: Instant,
    budget: Duration,
    tel: &mut Telemetry,
) -> (Status, Vec<Finding>) {
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
        match call(key, model, CLASSIFY_SYSTEM, &classify_prompt(subject), started, budget, tel) {
            Ok(body) => Some(body),
            Err(s) => return (s, Vec::new()),
        }
    } else {
        None
    };
    let prompt = check_prompt(subject, labelled.as_deref());
    match call(key, model, CHECK_SYSTEM, &prompt, started, budget, tel) {
        Ok(body) => match parse_findings(&body, subject) {
            Some(f) => (Status::Ok, f),
            None => {
                // Say what could not be read. "Unparsable" alone sends
                // whoever is tuning this back to the provider to guess.
                tel.detail = Some(format!(
                    "reply was not a usable answer; {} bytes beginning: {}",
                    body.len(),
                    body.chars().take(200).collect::<String>()
                ));
                (Status::Unparsable, Vec::new())
            }
        },
        Err(s) => (s, Vec::new()),
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
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    let v: serde_json::Value = serde_json::from_str(body.get(start..=end)?).ok()?;
    let rows = v.get("disagreements")?.as_array()?;
    let mut out = Vec::new();
    for row in rows {
        let kind = row.get("kind").and_then(|k| k.as_str()).unwrap_or_default();
        if kind != "contradicts" && kind != "overreaches" {
            return None;
        }
        let evidence = row
            .get("evidence")
            .and_then(|e| e.as_str())
            .unwrap_or_default()
            .to_string();
        // The model is not asked which fact it read, because it is not
        // asked to track ids; the span is attributed to whichever cited
        // fact actually contains it, and to the first when none does —
        // where `verified_quotes` will then drop the quotation.
        let fact = subject
            .evidence
            .iter()
            .find(|(_, _, obs)| {
                !evidence.is_empty() && obs.iter().any(|o| o.contains(&evidence))
            })
            .or_else(|| subject.evidence.first())
            .map(|(id, _, _)| id.clone())
            .unwrap_or_default();
        out.push(Finding {
            kind: kind.to_string(),
            clause: row
                .get("clause")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_string(),
            fact,
            evidence: Some(evidence),
            why: row.get("why").and_then(|w| w.as_str()).unwrap_or_default().to_string(),
            quoted: false,
            rejected_span: None,
        });
    }
    Some(out)
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
    let retried = records.iter().filter(|r| r.attempts > expected_calls(&r.approach)).count();

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
            findings: 0,
            later: None,
        });
        row.flagged |= !r.findings.is_empty();
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

    // ---- quotations ----
    let all: Vec<&Finding> = records.iter().flat_map(|r| r.findings.iter()).collect();
    let quoted = all.iter().filter(|f| f.quoted).count();
    let rejected: Vec<&&Finding> = all.iter().filter(|f| f.rejected_span.is_some()).collect();
    out.push_str(&format!("\nQUOTATIONS\n  findings         {}\n", all.len()));
    if !all.is_empty() {
        out.push_str(&format!(
            "  quoted verbatim  {quoted}   ({:.0}%)\n  span rejected    {}\n",
            100.0 * quoted as f64 / all.len() as f64,
            rejected.len()
        ));
    }
    if show_spans {
        for f in &rejected {
            out.push_str(&format!(
                "\n  [{}] {}\n    the model offered: {}\n",
                f.fact,
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

    Ok(out)
}

/// How many calls an approach makes when nothing is retried, so a retry
/// can be counted rather than inferred.
fn expected_calls(approach: &str) -> u32 {
    match approach {
        "split" => 2,
        _ => 1,
    }
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
    fn every_block_echoes_all_five_settings() {
        // `config.rs` admits a key only if it is visible in the output it
        // affects. Three of the five are invisible without this echo.
        for delivered in [None, Some(&record_fixture())] {
            let b = block(&settings_fixture(), "claim", delivered, Trigger::NotAttempted);
            for key in ["model", "approach", "timeout_ms", "verbs"] {
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
            kind: "contradicts".into(),
            clause: "the function returns early".into(),
            fact: "F1".into(),
            evidence: None,
            why: "invented".into(),
            quoted: false,
            rejected_span: Some("text that is in no captured output".into()),
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
            kind: "overreaches".into(),
            clause: "every call site".into(),
            fact: "F2".into(),
            evidence: Some("fn a() {}".into()),
            why: "one file was opened".into(),
            quoted: true,
            rejected_span: None,
        };
        let payload = f.payload();
        assert_eq!(payload["evidence"], "fn a() {}");
        assert_eq!(payload["quoted"], true);
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
        let subject = Subject {
            mint: "C1".into(),
            verb: "claim".into(),
            text: "x".into(),
            evidence: vec![
                ("F1".into(), vec![], vec!["alpha".to_string()]),
                ("F2".into(), vec![], vec!["beta".to_string()]),
            ],
        };
        let f = parse_findings(
            r#"{"disagreements":[{"kind":"contradicts","clause":"c","evidence":"beta","why":"w"}]}"#,
            &subject,
        )
        .expect("parsed");
        assert_eq!(f[0].fact, "F2");
    }
}
