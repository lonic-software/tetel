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

/// The provider the eval measured the LLM legs against. Not a
/// configuration key: which provider a leg calls follows from the model
/// value's vendor half ([`config::is_typed_model`]), so a key naming the
/// provider would name a choice nobody can make. [`TYPED_ENDPOINT`] is the
/// other one.
const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Environment variables holding the credential, in order. Never a config
/// key and never written anywhere: config files are shared, committed and
/// pasted into issues.
const KEY_VARS: [&str; 2] = ["OPENROUTER_API_KEY", "TETEL_API_KEY"];

/// TypeSafe's System One endpoint, which every `typesafe/` model value
/// routes to. It takes program state and a map of typed questions and
/// answers each with probabilities; there is no prompt and no sampling.
const TYPED_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

/// The TypeSafe credential. The same rule as [`KEY_VARS`]: the environment
/// only, never a config file.
const TYPED_KEY_VAR: &str = "TYPESAFE_API_KEY";

/// The version every typed leg's thresholds and results were measured
/// against: what `jev-latest` resolved to in the reply recorded at
/// `verifier-eval/jev_reply_2026-09-21.json` in the tetel-eval-data
/// repository.
///
/// Compared against the `model` each reply reports, not against the
/// configured value, because the configured value may be an alias. A reply
/// naming anything else is flagged `typed_model_unmeasured`.
const MEASURED_TYPED_VERSION: &str = "jev-1.13.0";

/// TypeSafe's published input rate, 2026-09-21: $0.042 per million input
/// tokens, output free.
///
/// A constant because the reply states tokens and no price — the recorded
/// reply's `usage` carries `input_tokens` and `output_tokens` and nothing
/// else. Without it every typed call would total zero in `cost`, the one
/// place a reader checks. The benchmark harness priced the measured runs
/// the same way. Dated so a reader can tell when it last matched the rate
/// card.
const TYPED_USD_PER_INPUT_TOKEN: f64 = 0.042 / 1_000_000.0;

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
    /// Enabled, but nothing to call with: no model, a model a settings
    /// file names and the key refuses, or no credential for a provider a
    /// leg that would run needs. `detail` names every one that applies.
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
    /// The typed gate scored the subject below its verb's threshold, so no
    /// classify, check or literal call was made and nothing was compared.
    ///
    /// Never `ok` with an empty list: that is the clean result this status
    /// vocabulary exists to keep an empty one from passing for. A gated
    /// record carries no findings at all — [`block`] emits them under `ok`
    /// alone — which is also why the literal leg does not run behind it.
    Gated,
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
            Status::Gated => "gated",
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

/// Whether a kind is reported at all for this verb.
///
/// `overreaches` is not, on a `fact`. Measured over 123 corpus facts: the
/// refined prompt returned 16 `contradicts` and 21 `overreaches`, and every
/// one of the 21 was the same objection — *the search excluded paths*, *the
/// capture covers only this range* — which is insufficiency, not
/// disagreement. The prompt already forbids it in as many words
/// (`insufficiency is not disagreement`) and the model does it anyway, so
/// this is a bound rather than a third round of wording, exactly as
/// [`is_checkable`] became one after the same lesson.
///
/// Adjudicated one by one against the full capture, the 16 that remain are
/// 10 correct — 63%, against 21% for a sample drawn from both kinds — and
/// not one of the 6 wrong ones is an insufficiency objection. The failure
/// mode lives entirely in the kind this drops.
///
/// `prose` is bounded on the same evidence and one round later. Once the
/// paragraph was announced as a paragraph (see [`PROSE_CLASSIFY_SYSTEM`]),
/// its 24 findings split 10 `overreaches` — every one wrong — against 14
/// `contradicts` carrying all 5 catches. Note the bound was *not* free on
/// the round before that one, where a catch arrived as an `overreaches`; it
/// is measured-zero only on the configuration that ships.
///
/// Why the verb and not the prompt: a `fact` note is a record of one
/// capture and a `prose` paragraph rests on facts it did not choose, so
/// "the capture does not cover the population" is always true of both and
/// never news. A `claim` ranges over a design's whole argument, where the
/// same kind carries 83% precision and stays on.
fn kind_reported_for(verb: &str, kind: &str) -> bool {
    !(matches!(verb, "fact" | "prose") && kind == KIND_OVERREACHES)
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
    pub literals: bool,
    /// Unset means no refutation leg; every finding reaches the author.
    /// A `typesafe/` value is a typed leg, and runs only on a verb whose
    /// row in [`TYPED_LEGS`] permits it — see [`refuter_leg`].
    pub refuter: Option<String>,
    /// `verify.typed_model`: the TypeSafe model that runs the typed legs
    /// the verb's row in [`TYPED_LEGS`] names — the gate, classify, the
    /// literal leg. `None` means none of them runs.
    pub typed_model: Option<String>,
    /// The default typed model was passed over because TypeSafe's key is
    /// not in the environment: `verify.typed_model` is unset, so
    /// [`config::DEFAULT_TYPED_MODEL`] would have run. Echoed as
    /// `typed_model_not_run` on a verb it would have run on, so an author
    /// without the key learns what the default costs them rather than
    /// meeting an `unauthorized` over a credential they never chose to need.
    pub typed_default_without_key: bool,
    /// Why a `verify.model` written in a settings file was refused, when
    /// one was.
    ///
    /// Resolved here, where the workspace directory is in hand, because
    /// [`block`] cannot: a refused value resolves to nothing, and without
    /// this an author looking at a file that sets the key would be told it
    /// is not set.
    pub model_refusal: Option<String>,
    /// Why a `verify.typed_model` written in a settings file was refused,
    /// when one was. Echoed as `typed_model_refused` where a typed leg
    /// would run: the refused value counts as off, so the status alone
    /// never shows it.
    pub typed_model_refusal: Option<String>,
}

/// How long one OpenRouter call is allowed, when nothing configured a budget.
///
/// The default is per *leg* rather than per verification, because a
/// verification is several calls in series and a flat number silently means
/// different things for different runs. 60 seconds a leg was too little on
/// `openai/gpt-6-luna`. In the 2026-09-23 screen, 12 of 113 answered gated
/// `fact` draws ran past that budget, the slowest at 286 seconds, and TET-98
/// found that the slow draws were more often the ones with findings
/// (`scripts/verifier-eval/README.md`, "Screen, 2026-09-23" and "The default
/// budget cuts off"). At 100 seconds the same arm gets 310, more than any
/// screened draw took. Those were measured at 20–40 concurrent requests.
/// Nothing waits on this budget: it bounds a detached thread whose only job
/// is to write a log line, so being generous costs a slow failure, while
/// being tight costs findings.
const DEFAULT_MS_PER_LEG: u64 = 100_000;

/// How long one TypeSafe call is allowed, when nothing configured a budget.
///
/// Its own constant because the two providers are not the same order of
/// magnitude: Jev answers typed questions in about a second, with no
/// reasoning to wait for, so charging it [`DEFAULT_MS_PER_LEG`] would
/// stretch a budget that exists to notice a hang. Ten times the observed
/// latency is headroom, not a measurement.
const DEFAULT_MS_PER_TYPED_LEG: u64 = 10_000;

/// The budget for `calls` when `verify.timeout_ms` is unset: each provider's
/// calls at that provider's rate.
fn default_budget_ms(calls: Calls) -> u64 {
    DEFAULT_MS_PER_LEG * u64::from(calls.llm) + DEFAULT_MS_PER_TYPED_LEG * u64::from(calls.typed)
}

/// The effective settings for `verb`.
///
/// Per verb because the default budget is: which legs are typed depends on
/// the verb's row in [`TYPED_LEGS`], and a typed leg is budgeted at its
/// own rate.
pub fn settings(workspace_dir: &Path, verb: &str) -> Settings {
    let d = Some(workspace_dir);
    let approach = config::verify_approach(d);
    let literals = config::verify_literals(d);
    let refuter = config::verify_refuter(d);
    let model = config::verify_model(d);
    let (typed_model, typed_default_without_key) =
        effective_typed_model(config::verify_typed_model(d), typed_key().is_some());
    let calls = planned_calls(verb, &approach, literals, refuter.as_deref(), model.as_deref(), typed_model.as_deref());
    Settings {
        enabled: config::verify_enabled(d),
        model,
        timeout_ms: config::verify_timeout_ms(d).unwrap_or(default_budget_ms(calls)),
        approach,
        verbs: config::verify_verbs(d),
        literals,
        refuter,
        typed_model,
        typed_default_without_key,
        model_refusal: config::verify_model_refusal(d),
        typed_model_refusal: config::verify_typed_model_refusal(d),
    }
}

/// The typed model in force, and whether the default was passed over for
/// want of TypeSafe's key. Apart from [`settings`] so the rule is tested
/// without touching the process environment.
fn effective_typed_model(choice: config::TypedModel, has_typed_key: bool) -> (Option<String>, bool) {
    match choice {
        config::TypedModel::Set(m) => (Some(m), false),
        config::TypedModel::Off => (None, false),
        config::TypedModel::Unset if has_typed_key => (Some(config::DEFAULT_TYPED_MODEL.to_string()), false),
        config::TypedModel::Unset => (None, true),
    }
}

/// The calls the default budget pays for on `verb` under these settings.
///
/// One call for each typed leg the row runs, which is what every measured
/// subject needed: a leg is split across calls only past
/// [`MAX_TYPED_QUESTIONS`] questions.
fn planned_calls(
    verb: &str,
    approach: &str,
    literals: bool,
    refuter: Option<&str>,
    model: Option<&str>,
    typed_model: Option<&str>,
) -> Calls {
    let row = typed_legs(verb);
    let on = |leg: bool| typed_model.is_some() && leg;
    let typed = TypedCalls {
        gate: u32::from(on(row.gate.is_some())),
        classify: on(row.classify).then_some(1),
        literals: on(row.literals).then_some(1),
    };
    expected_calls(approach, literals, refuter_leg(refuter, model, verb), typed)
}

fn api_key() -> Option<String> {
    KEY_VARS
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.trim().is_empty()))
}

fn typed_key() -> Option<String> {
    std::env::var(TYPED_KEY_VAR).ok().filter(|s| !s.trim().is_empty())
}

/// Which legs a verb may send to TypeSafe.
///
/// A table, in source, because routing follows the model value and not
/// the key it came from: `verify.refuter_model` becomes a typed leg the
/// moment it holds a `typesafe/` value, and it takes no verb, so without
/// an explicit row it would run on every verb — including the ones where
/// it was measured and failed. Each cell is what was measured on that
/// verb, and a verb absent from the table has no typed legs at all.
#[derive(Clone, Copy, Default)]
struct TypedLegs {
    /// Jev as the refuter. `fact` only: 8 of 10 adjudicated findings kept
    /// at 80% there, 13 of 44 at 31% on `prose`, and no run on `claim`.
    refuter: bool,
    /// Jev as a gate before the check, when `verify.typed_model` is set.
    /// None on `prose`, where no presentation separated (best AUC 0.69).
    gate: Option<Gate>,
    /// Jev labelling `split`'s assertions in place of the LLM's classify
    /// call, when `verify.typed_model` is set. `claim` only: the same check
    /// raised 11 correct warnings under either classifier over 125 claims,
    /// at 39% of the cost. `fact` shares the classify prompt, but the port
    /// was never run there.
    classify: bool,
    /// Jev judging the literal leg's candidates, when `verify.typed_model`
    /// is set and `verify.literals` is on. `claim` only: 82% precision
    /// against the LLM leg's 80% on the same 88 claims, at a sixth of the
    /// cost. It produces findings, so a row with it permits no typed
    /// refuter — the one model would be judging its own.
    literals: bool,
}

/// One verb's gate: the presentation it was measured with, and the score
/// below which a subject is not checked.
///
/// The two are one decision. Each threshold sits 0.05 under the lowest
/// adjudicated defect *that presentation* scored, fitted in-sample with
/// nothing held out, so neither threshold means anything under the other
/// presentation — `pick_cls` saves 7% on `claim` against `pick_clause`'s
/// 21%. Ported from `scripts/verifier-eval/gate_variants.py`.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Gate {
    /// What the author's text is cut into for the choice.
    unit: GateUnit,
    /// Whether each unit is also asked CURRENT, PROPOSED or ARGUMENT, and
    /// only its CURRENT share counted.
    classify: bool,
    threshold: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GateUnit {
    Sentence,
    Clause,
}

impl GateUnit {
    fn word(self) -> &'static str {
        match self {
            GateUnit::Sentence => "sentence",
            GateUnit::Clause => "clause",
        }
    }
}

const TYPED_LEGS: [(&str, TypedLegs); 3] = [
    // `pick_cls`: 60% of subjects skipped, none of 12 adjudicated defects;
    // the lowest scored 0.50. Through tetel's own path (TET-98) it skipped 54%
    // of draws, and entirely only minor defects: 3 of 14 held out.
    (
        "fact",
        TypedLegs {
            refuter: true,
            gate: Some(Gate { unit: GateUnit::Sentence, classify: true, threshold: 0.45 }),
            classify: false,
            literals: false,
        },
    ),
    // `pick_clause`: 27% skipped, none of 10 adjudicated warnings in the
    // fit. Through tetel's own path (TET-98) it skipped 2 of those 10 in
    // most draws: across the fitting runs they had scored 0.55–0.63, and the
    // subject tetel builds is not the harness's. A claim is usually one long sentence, so a
    // choice over sentences degenerates to a yes/no.
    (
        "claim",
        TypedLegs {
            refuter: false,
            gate: Some(Gate { unit: GateUnit::Clause, classify: false, threshold: 0.53 }),
            classify: true,
            literals: true,
        },
    ),
    ("prose", TypedLegs { refuter: false, gate: None, classify: false, literals: false }),
];

fn typed_legs(verb: &str) -> TypedLegs {
    TYPED_LEGS
        .iter()
        .find(|(v, _)| *v == verb)
        .map(|(_, row)| *row)
        .unwrap_or_default()
}

/// What the configured refuter does on one verb.
///
/// The one place the refuter's row is applied. The budget, the credential
/// check, the response and the refutation leg itself all ask this rather
/// than reading `verify.refuter_model` on their own, so none of them can
/// run, price, demand a key for or print a leg that another of them has
/// ruled out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RefuterLeg<'a> {
    /// `off`: no refutation leg.
    Off,
    /// An OpenRouter model, on any verb.
    Llm(&'a str),
    /// A `typesafe/` model on a verb whose row permits it.
    Typed(&'a str),
    /// A `typesafe/` model on a verb whose row does not. It runs nothing
    /// and is not replaced by [`config::DEFAULT_REFUTER`]: that would
    /// spend on a model the author had just replaced.
    NotRun(&'a str),
    /// The check model itself. A model refuting itself scored 17%, so it
    /// runs nothing either, and is reported as `NotRun` is: a name over
    /// findings nothing refuted says they were.
    Itself(&'a str),
}

impl RefuterLeg<'_> {
    /// The model that actually refutes, when one does.
    fn runs(self) -> Option<String> {
        match self {
            RefuterLeg::Llm(m) | RefuterLeg::Typed(m) => Some(m.to_string()),
            RefuterLeg::Off | RefuterLeg::NotRun(_) | RefuterLeg::Itself(_) => None,
        }
    }

    /// A refuter the author set that runs nothing, and why — so a reply
    /// that prints no refuter over findings nothing refuted does not look
    /// like one where the refuter was turned `off`.
    fn not_run(self) -> Option<RefuterNotRun> {
        let (m, reason) = match self {
            RefuterLeg::NotRun(m) => (m, "a `typesafe/` refuter runs on `fact` only".to_string()),
            RefuterLeg::Itself(m) => (
                m,
                format!(
                    "it is `verify.model` too, and a model refuting itself scored 17% — set \
                     `verify.refuter_model` to a different model, or to `{}`",
                    config::REFUTER_OFF
                ),
            ),
            RefuterLeg::Off | RefuterLeg::Llm(_) | RefuterLeg::Typed(_) => return None,
        };
        Some(RefuterNotRun { refuter_model: m.to_string(), reason })
    }
}

/// [`RefuterLeg::not_run`], persisted on the [`Record`] it applied to.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RefuterNotRun {
    pub refuter_model: String,
    pub reason: String,
}

fn refuter_leg<'a>(refuter: Option<&'a str>, model: Option<&str>, verb: &str) -> RefuterLeg<'a> {
    match refuter {
        None => RefuterLeg::Off,
        Some(m) if Some(m) == model => RefuterLeg::Itself(m),
        Some(m) if !config::is_typed_model(m) => RefuterLeg::Llm(m),
        Some(m) if typed_legs(verb).refuter => RefuterLeg::Typed(m),
        Some(m) => RefuterLeg::NotRun(m),
    }
}

/// Every typed leg that will run on `verb`, as the key that set its model
/// and the model.
///
/// `verify.typed_model` is one entry whichever of its legs the row runs —
/// the gate, classify under `split`, the literal leg when it is on.
///
/// The one place the rows are applied for the credential:
/// [`providers_for`] builds the TypeSafe endpoint exactly when this is
/// non-empty, and [`unauthorized_detail`] names a gap for each entry, so
/// the two cannot disagree about which keys a verification needs.
/// Whether `verify.typed_model`, were it set, would run a leg on `verb`
/// under the rest of these settings: the verb's row names one the
/// approach and `verify.literals` leave on.
fn typed_model_has_a_leg(settings: &Settings, verb: &str) -> bool {
    let row = typed_legs(verb);
    row.gate.is_some()
        || (row.classify && settings.approach == "split")
        || (row.literals && settings.literals)
}

fn typed_legs_that_run<'a>(settings: &'a Settings, verb: &str) -> Vec<(&'static str, &'a str)> {
    let mut legs = Vec::new();
    if let Some(m) = settings.typed_model.as_deref() {
        if typed_model_has_a_leg(settings, verb) {
            legs.push((config::KEY_VERIFY_TYPED_MODEL, m));
        }
    }
    if let RefuterLeg::Typed(m) =
        refuter_leg(settings.refuter.as_deref(), settings.model.as_deref(), verb)
    {
        legs.push((config::KEY_VERIFY_REFUTER, m));
    }
    legs
}

/// Where one provider's calls go, and the credential they carry.
///
/// A value rather than two constants read at the call site so the tests
/// can point a verification at a local listener; production builds both
/// from [`ENDPOINT`], [`TYPED_ENDPOINT`] and the environment in [`spawn`].
#[derive(Clone)]
struct Endpoint {
    url: String,
    key: String,
}

/// The providers one verification may call. `typed` is present exactly
/// when a leg that will run needs it — see [`spawn`].
struct Providers {
    llm: Endpoint,
    typed: Option<Endpoint>,
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
    /// What the model or provider returned that [`detail`](Self::detail)
    /// is about: the beginning of a reply that could not be read, or a
    /// label outside the vocabulary. Kept for the log and for
    /// `verify-report --spans`, and never sent to the author, for the
    /// reason a rejected span is withheld: text that looks like evidence
    /// sends the reader to check it, and this text is not evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    /// [`Subject::revision`], persisted. `None` on a record written before
    /// the field existed, which [`unverified`] orders behind every record
    /// of the same mint that has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    /// [`Telemetry::not_verbatim`], persisted. A finding that reached the
    /// author carries its own fidelity marks; these two count what was
    /// dropped *before* anything reached them, and a drop nobody can see
    /// is the same un-actionable silence `rejected_span` exists to break.
    #[serde(default)]
    pub not_verbatim: u32,
    /// [`Telemetry::literals_refuted`], persisted — the only accuracy
    /// signal the `unevidenced` kind has, since no eval has scored it. On a
    /// record whose literal leg was Jev's
    /// ([`typed_literal_calls`](Self::typed_literal_calls)), it counts
    /// code-proposed candidates instead, and the report leaves it out.
    #[serde(default)]
    pub literals_refuted: u32,
    /// [`Telemetry::not_a_quantity`], persisted.
    #[serde(default)]
    pub not_a_quantity: u32,
    /// [`Telemetry::kind_off_verb`], persisted — how many findings this
    /// verb declined to report on account of their kind. A drop nobody can
    /// see is the silence `rejected_span` exists to break, and this one
    /// removes whole findings rather than a quotation.
    #[serde(default)]
    pub kind_off_verb: u32,
    /// [`Telemetry::refuted`], persisted.
    #[serde(default)]
    pub refuted: u32,
    /// Which model refuted, when one did. Recorded rather than inferred
    /// from [`refuted`](Self::refuted) being non-zero: a leg that ran and
    /// dropped nothing and a leg that never ran are different states, and
    /// only this tells them apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refuter: Option<String>,
    /// The status of the literal leg when it ran and did **not** complete.
    ///
    /// `None` means either that the leg was off or that it finished, and
    /// those two are told apart by [`literals`](Self::literals). Present
    /// only on the failure, because that is the case where `findings` is
    /// complete for the disagreement kinds and silent for this one — a
    /// distinction the author cannot infer from an absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literals_status: Option<String>,
    /// The status of the first refuter call that did not complete, when
    /// one did not.
    ///
    /// Such a call keeps its finding, so `findings` is still right to
    /// deliver; what it is not is refuted, and a response printing the
    /// refuter's name over it would claim otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refuter_status: Option<String>,
    /// Every version a TypeSafe reply said answered, first seen first.
    /// Empty when no typed call returned — which is also how [`block`]
    /// knows a typed leg ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub typed_versions: Vec<String>,
    /// The refuter the author set that this record's verification did not
    /// run, and why. Kept on the record so the reply that delivers it says
    /// so for the verb it ran on, not the verb that delivered it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refuter_not_run: Option<RefuterNotRun>,
    /// How many TypeSafe calls the gate made, whatever it decided. Kept
    /// because the report counts a retry against the calls a verification
    /// should have made, and the gate's count is the one that depends on the
    /// subject — a long enough note is asked across two calls.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gate_calls: u32,
    /// The status of a gate call that did not complete. The check ran
    /// anyway — a gate that fails never skips — and this is what says the
    /// subject was checked ungated rather than gated and passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_status: Option<String>,
    /// How many TypeSafe calls Jev classify made, when it stood in for the
    /// LLM's classify call; `None` when it did not. Kept for the retry count
    /// as [`gate_calls`](Self::gate_calls) is, and an `Option` because a
    /// stand-in that asked nothing — a claim with no unit to label — still
    /// took the LLM call's place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_classify_calls: Option<u32>,
    /// The same for Jev judging the literal leg, which asks nothing when no
    /// candidate survives the filters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_literal_calls: Option<u32>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
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

/// The guidance under a status that says a verification was attempted and
/// did not complete — see [`is_failure`]. [`GUIDANCE`] describes findings,
/// and there are none: sent here, it asks the author to read a quoted span
/// that is not there.
///
/// It says the mint blocks nothing because the obvious response is the
/// wrong one. Repeating the same text starts no new verification, and a
/// claim's grounding records count only for the wording they graded, so a
/// claim reworded to clear [`unverified`] trades its grounding for a
/// model's opinion.
const UNCHECKED_GUIDANCE: &str = "Not a finding. The verification of the mint named by `for_mint` did not complete, so that mint was not checked: no `findings` means nothing was compared, not that nothing was wrong, and `detail` says why. An unchecked mint blocks nothing and is not a request to change it. Sending the same text again starts no new verification, and rewording a claim only to clear `unverified` discards the grounding recorded against its current wording.";

/// Whether a record's status is a verification that was attempted and lost:
/// `timeout`, `unavailable` or `unparsable`.
///
/// Not `gated`, which decided there was nothing to compare, nor
/// `unauthorized`, whose `detail` comes from the current settings rather
/// than from the record.
fn is_failure(status: &str) -> bool {
    [Status::Timeout, Status::Unavailable, Status::Unparsable].iter().any(|s| s.as_str() == status)
}

/// How many mints `unverified` names; `count` says how many there are.
const UNVERIFIED_SHOWN: usize = 10;

/// The mints whose latest verification failed, for the `unverified` key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unverified {
    /// Every such mint.
    pub count: usize,
    /// At most [`UNVERIFIED_SHOWN`] of them, highest revision first, then
    /// in log order.
    pub mints: Vec<String>,
}

/// The mints of a verb still verified whose latest verification ended in
/// [`is_failure`], withdrawn claims left out; `None` when there are none or
/// verification is off.
///
/// "Latest" is the highest [`Record::revision`], not the last in `log`:
/// the log is in completion order, and a revision's verification can
/// finish before the one it replaced. A tie goes to the record later in
/// the log, and a record with no revision sorts behind every one that has
/// one. A mint of a verb no longer in `verify.verbs` is not listed, since
/// nothing would ever clear it.
///
/// `log` is what [`peek_delivered`] already read, so this reads
/// verify.log no second time. claims.jsonl is read only when a failing
/// mint is a claim — the one kind that can be withdrawn.
pub fn unverified(dir: &Path, settings: &Settings, log: &[Record]) -> Option<Unverified> {
    if !settings.enabled {
        return None;
    }
    let mut latest: std::collections::BTreeMap<(&str, &str), (usize, &Record)> = Default::default();
    for (i, r) in log.iter().enumerate() {
        let key = (r.verb.as_str(), r.mint.as_str());
        match latest.get(&key) {
            // `Option`'s order puts `None` below every `Some`, which is the
            // legacy rule; `>=` hands a tie to the later record.
            Some((_, standing)) if r.revision < standing.revision => {}
            _ => {
                latest.insert(key, (i, r));
            }
        }
    }
    let mut failing: Vec<(usize, &Record)> = latest
        .into_values()
        .filter(|(_, r)| is_failure(&r.status) && settings.verbs.iter().any(|v| v == &r.verb))
        .collect();
    if failing.iter().any(|(_, r)| r.verb == "claim") {
        let withdrawn: Vec<String> = crate::claims::load_all(dir)
            .map(|cs| cs.into_iter().filter(|c| c.withdrawn).map(|c| c.id).collect())
            .unwrap_or_default();
        failing.retain(|(_, r)| !(r.verb == "claim" && withdrawn.contains(&r.mint)));
    }
    if failing.is_empty() {
        return None;
    }
    failing.sort_by_key(|(i, r)| (std::cmp::Reverse(r.revision), *i));
    Some(Unverified {
        count: failing.len(),
        mints: failing.iter().take(UNVERIFIED_SHOWN).map(|(_, r)| r.mint.clone()).collect(),
    })
}

/// The `verify` object for one reply.
///
/// `delivered` is a verification that finished before this call;
/// `queued_for` is the mint whose verification this call just started.
/// Either may be absent, and when both are the status is whichever
/// pre-call state applies. `unverified` rides on every reply that has
/// one, whatever its status and verb.
pub fn block(
    settings: &Settings,
    verb: &str,
    delivered: Option<&Record>,
    trigger: Trigger<'_>,
    unverified: Option<&Unverified>,
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
    // What refuted the findings this reply carries. A delivered record
    // answers for itself: it may be another verb's, or from before a
    // settings change, and the refuter the *current* call would run is
    // then not the one that ran on the findings below.
    let (refuter_model, not_run) = match delivered {
        Some(r) => (r.refuter.clone(), r.refuter_not_run.clone().map(|n| (r.verb.clone(), n))),
        None => {
            let leg = refuter_leg(settings.refuter.as_deref(), settings.model.as_deref(), verb);
            (leg.runs(), leg.not_run().map(|n| (verb.to_string(), n)))
        }
    };
    let missing = if status == Status::Unauthorized.as_str() {
        unauthorized_detail(settings, verb, api_key().is_some(), typed_key().is_some())
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
        // Null when `off`. A reader who sees a model name here knows every
        // finding below was put to it — unless `refuter_incomplete` says a
        // call did not return — and it is removed below for a refuter that
        // is set and runs nothing.
        "refuter_model": refuter_model,
        "guidance": if is_failure(&status) { UNCHECKED_GUIDANCE } else { GUIDANCE },
    });
    let map = out.as_object_mut().expect("json object");
    // Removed from the literal rather than kept out of it, so a
    // configuration whose refuter runs builds exactly the object it always
    // did. Printing a model name for a leg that never ran would
    // tell the author their findings were refuted when nothing refuted
    // them; saying nothing at all would leave them wondering where the
    // refuter they set went.
    if let Some((v, m)) = not_run {
        map.remove("refuter_model");
        map.insert(
            "refuter_not_run".into(),
            json!({"verb": v, "refuter_model": m.refuter_model, "reason": m.reason}),
        );
    }
    // Inserted only when set, so a configuration that never names a typed
    // model builds the object it always did — absent, not null, because a
    // new null key is still a change to every caller's field set.
    if let Some(tm) = &settings.typed_model {
        map.insert("typed_model".into(), json!(tm));
    }
    // The default passed over for want of a key, stated where it would have
    // run. Not while verification is off for the verb, where nothing would
    // have run anyway; not on `prose`, where no typed leg exists to miss;
    // and not once the author sets the key to `off`, which silences it.
    if settings.typed_default_without_key && verb_enabled(settings, verb) && typed_model_has_a_leg(settings, verb) {
        map.insert(
            "typed_model_not_run".into(),
            json!({
                "typed_model": config::DEFAULT_TYPED_MODEL,
                "reason": format!(
                    "`{}` is unset, so it defaults to `{}`, which needs {TYPED_KEY_VAR} in the \
environment; export it, or set `{}` to `{}` to stop this notice",
                    config::KEY_VERIFY_TYPED_MODEL,
                    config::DEFAULT_TYPED_MODEL,
                    config::KEY_VERIFY_TYPED_MODEL,
                    config::REFUTER_OFF,
                ),
            }),
        );
    }
    // A refused value, stated under the same conditions: it counts as off,
    // so without this the typed legs stop and nothing in the reply says why.
    if let Some(why) = &settings.typed_model_refusal {
        if verb_enabled(settings, verb) && typed_model_has_a_leg(settings, verb) {
            map.insert("typed_model_refused".into(), json!(why));
        }
    }
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
            // The same qualification for the refuter: findings a refuter
            // call never answered for are delivered, because a refutation
            // that did not happen is not one, but not under a printed
            // refuter name alone.
            if let Some(s) = &r.refuter_status {
                map.insert("refuter_incomplete".into(), json!(s));
            }
            // And for the gate: the check ran because the gate could not
            // say whether to skip, not because it said not to.
            if let Some(s) = &r.gate_status {
                map.insert("gate_incomplete".into(), json!(s));
            }
        }
        // Outside the `ok` guard, on purpose. The version flag matters most
        // on a record with nothing else to show — a typed call that
        // answered and then shaped what ran, under thresholds fitted to a
        // version that may no longer be the one answering.
        if !r.typed_versions.is_empty() {
            map.insert("typed_model_versions".into(), json!(r.typed_versions));
            if r.typed_versions.iter().any(|v| v != MEASURED_TYPED_VERSION) {
                map.insert("typed_model_unmeasured".into(), json!(true));
            }
        }
        // Why it failed, under the three statuses where the record's
        // detail belongs to the failure: the leg that failed is the last
        // one that ran. Not under `ok` or `gated`, where it is whatever leg
        // last wrote it and the `*_incomplete` keys already say which leg
        // did not finish. And only on a record that carries a revision:
        // one written before the reply text was split off may quote it in
        // its detail, and nothing else tells the two apart.
        if is_failure(&r.status) && r.revision.is_some() {
            if let Some(d) = &r.detail {
                map.insert("detail".into(), json!(d));
            }
        }
    }
    if let Some(u) = unverified {
        map.insert("unverified".into(), json!({"count": u.count, "mints": u.mints}));
    }
    if let Trigger::Queued(m) = trigger {
        map.insert("queued_for".into(), json!(m));
    }
    if let Some(m) = missing {
        map.insert("detail".into(), json!(m));
    }
    out
}

/// What an `unauthorized` status is missing, every gap named.
///
/// `unauthorized` covers several different gaps, and the obvious first
/// step — `config verify.enabled true` with a key already exported — hits
/// one that is *not* about the credential. Naming each that is missing is
/// the difference between a one-line fix and an afternoon spent debugging
/// a key that was fine all along; naming only the first would be the
/// afternoon in instalments. The mirror of [`providers_for`], which
/// decides the same question for [`spawn`].
fn unauthorized_detail(
    settings: &Settings,
    verb: &str,
    has_llm_key: bool,
    has_typed_key: bool,
) -> Option<String> {
    let mut gaps: Vec<String> = Vec::new();
    match (&settings.model_refusal, &settings.model) {
        (Some(why), _) => gaps.push(why.clone()),
        (None, None) => {
            gaps.push("`verify.model` is not set — `tetel config verify.model <vendor/model>`".into())
        }
        (None, Some(_)) => {}
    }
    if !has_llm_key {
        gaps.push("no API key in the environment — export OPENROUTER_API_KEY".into());
    }
    // Only for a typed leg that would run on this verb. A typesafe refuter
    // on a verb whose row refuses it runs nothing, so its missing key must
    // not fail a verification that never needed it.
    if !has_typed_key {
        for (key, m) in typed_legs_that_run(settings, verb) {
            gaps.push(format!(
                "`{key}` is `{m}`, which runs on `{verb}` and needs {TYPED_KEY_VAR} in the environment"
            ));
        }
    }
    (!gaps.is_empty()).then(|| gaps.join("; "))
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
/// [`commit_delivered`] needs so a concurrent commit cannot skip past it,
/// and the whole log it was read from, which [`unverified`] tallies.
pub fn peek_delivered(dir: &Path) -> Peeked {
    let at = delivered_count(dir);
    let log = read_log(dir).0;
    Peeked { delivered: log.get(at).cloned().map(|r| (at, r)), log }
}

/// What [`peek_delivered`] read.
pub struct Peeked {
    pub delivered: Option<(usize, Record)>,
    pub log: Vec<Record>,
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
    /// How many times the mint had been revised when this verification was
    /// dispatched: its ledger's `revisions` count, 0 for a create.
    ///
    /// The order between two verifications of one mint. Not `seq`, `at` or
    /// the log position, which all mark when a verification *finished*,
    /// and two of them can finish in either order. The count is replayed
    /// from an append-only ledger, so it never decreases; two overlapping
    /// revisions can still read the same one, since nothing serialises the
    /// handlers, and [`unverified`] breaks that tie by log position.
    pub revision: u64,
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
    ///
    /// Nor does it touch the literal check, which asks a third question and
    /// gets [`Self::in_captured_output`] instead. Output matches are ordered
    /// first so a span present in one fact's capture is never attributed to
    /// another fact's label.
    fn containing(&self, span: &str) -> Vec<(String, QuotedFrom)> {
        if span.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<(String, QuotedFrom)> = Vec::new();
        let mut labelled: Vec<(String, QuotedFrom)> = Vec::new();
        for (id, extent, obs) in &self.evidence {
            if obs.iter().any(|o| o.contains(span)) {
                out.push((id.clone(), QuotedFrom::Output));
            } else if extent.iter().any(|e| e.contains(span)) {
                labelled.push((id.clone(), QuotedFrom::Extent));
            }
        }
        out.append(&mut labelled);
        out
    }

    /// Whether the span is in captured output — the observations alone.
    ///
    /// The literal check needs this and [`Self::containing`] cannot serve
    /// it. That one answers "did the model quote something it was shown",
    /// where a label is a legitimate source. This answers "does the capture
    /// carry this value", where a label is not: labels are generated by the
    /// tool, not captured by it, and they are full of exactly the tokens
    /// [`is_checkable`] admits — `lines 4000-4096`, `(grep (ERE): 2)`, `(exit 0)`.
    /// Sharing the wider predicate machine-refuted a note's "4096 bytes"
    /// against a line range that merely mentioned 4096, and counted the
    /// suppression as [`Telemetry::literals_refuted`] — the one accuracy
    /// signal that kind has, so the error was self-concealing.
    fn in_captured_output(&self, span: &str) -> bool {
        !span.is_empty()
            && self.evidence.iter().any(|(_, _, obs)| obs.iter().any(|o| o.contains(span)))
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
    let Some(model) = settings.model.clone() else { return false };
    let Some(providers) = providers_for(settings, &subject.verb, api_key(), typed_key()) else {
        return false;
    };
    let dir = dir.to_path_buf();
    let approach = settings.approach.clone();
    let literals = settings.literals;
    let refuter = settings.refuter.clone();
    let typed_model = settings.typed_model.clone();
    let budget = Duration::from_millis(settings.timeout_ms);
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut tel = Telemetry::default();
        let legs = Legs { literals, refuter: refuter.as_deref(), typed_model: typed_model.as_deref() };
        let (status, findings, literals_status) =
            run(&providers, &model, &approach, legs, &subject, started, budget, &mut tel);
        let record = record_of(
            next_seq(&dir),
            &subject,
            Ran { model, approach, literals, refuter: refuter.as_deref() },
            (status, findings, literals_status),
            tel,
            started.elapsed(),
        );
        let _ = workspace::append_jsonl(&log_path(&dir), &record);
    });
    true
}

/// What [`spawn`] configured a verification with, for [`record_of`].
struct Ran<'a> {
    model: String,
    approach: String,
    literals: bool,
    refuter: Option<&'a str>,
}

/// The record a finished verification leaves in the log.
fn record_of(
    seq: u64,
    subject: &Subject,
    ran: Ran<'_>,
    (status, findings, literals_status): (Status, Vec<Finding>, Option<Status>),
    mut tel: Telemetry,
    elapsed: Duration,
) -> Record {
    let Ran { model, approach, literals, refuter } = ran;
    let record_literals_ok = literals_status.is_none();
    let record_refuter_ok = tel.refuter_status.is_none();
    let record_gate_ok = tel.gate_status.is_none();
    let leg = refuter_leg(refuter, Some(&model), &subject.verb);
    // Only when something went wrong: a clean run has nothing to explain,
    // and a detail line on every record would train a reader to skip the
    // field. An `ok` run with a failed gate, literal or refuter call is the
    // one case where a clean status still has something to explain, so the
    // guard asks about all of them rather than the status alone. `reply`
    // goes wherever `detail` goes: it is only ever the text that detail is
    // about.
    let (detail, reply) = if status == Status::Ok && record_literals_ok && record_refuter_ok && record_gate_ok {
        (None, None)
    } else {
        (tel.detail.take(), tel.reply.take())
    };
    Record {
        seq,
        mint: subject.mint.clone(),
        verb: subject.verb.clone(),
        status: status.as_str().to_string(),
        model,
        approach,
        literals,
        findings,
        at: workspace::now_unix(),
        cost: tel.cost,
        elapsed_ms: elapsed.as_millis() as u64,
        attempts: tel.attempts,
        not_verbatim: tel.not_verbatim,
        literals_refuted: tel.literals_refuted,
        not_a_quantity: tel.not_a_quantity,
        kind_off_verb: tel.kind_off_verb,
        refuted: tel.refuted,
        // The refuter that ran, not the one configured: a typed refuter
        // this verb's row refuses, or the check model refuting itself,
        // ran nothing, and a record naming it would be counted by the
        // report as a refuted run and printed by `block` over findings
        // nothing refuted.
        refuter: leg.runs(),
        refuter_not_run: leg.not_run(),
        literals_status: literals_status.map(|s| s.as_str().to_string()),
        refuter_status: tel.refuter_status.map(|s| s.as_str().to_string()),
        typed_versions: std::mem::take(&mut tel.typed_versions),
        gate_calls: tel.gate_calls,
        gate_status: tel.gate_status.map(|s| s.as_str().to_string()),
        typed_classify_calls: tel.typed_classify_calls,
        typed_literal_calls: tel.typed_literal_calls,
        detail,
        reply,
        revision: Some(subject.revision),
    }
}

/// The providers a verification of `verb` calls, or `None` when a leg
/// that would run has no credential.
///
/// A TypeSafe credential is demanded by a typed leg that will run on this
/// verb, and by nothing else. Falling back to the LLM legs without it
/// would verify under a configuration the author did not set, and
/// demanding it for a leg the verb's row refuses would fail a
/// verification over a key it never needed.
fn providers_for(
    settings: &Settings,
    verb: &str,
    llm_key: Option<String>,
    typed_key: Option<String>,
) -> Option<Providers> {
    let llm = Endpoint { url: ENDPOINT.into(), key: llm_key? };
    let typed = if typed_legs_that_run(settings, verb).is_empty() {
        None
    } else {
        Some(Endpoint { url: TYPED_ENDPOINT.into(), key: typed_key? })
    };
    Some(Providers { llm, typed })
}

/// What one verification spent getting to its answer, accumulated across
/// however many calls the approach and the retries required.
#[derive(Default)]
pub struct Telemetry {
    pub cost: f64,
    pub attempts: u32,
    /// Written through [`explain`](Self::explain) and
    /// [`explain_quoting`](Self::explain_quoting), never directly, so that
    /// [`reply`](Self::reply) always belongs to the detail beside it.
    pub detail: Option<String>,
    /// See [`Record::reply`].
    pub reply: Option<String>,
    /// Text the model attributed to the author that the author did not
    /// write, dropped rather than passed on: a classify assertion that was
    /// not a substring of the claim, or a literal the literal check could
    /// not find in the text. One counter for both because it is one
    /// failure — the model returning its own words as a quotation.
    pub not_verbatim: u32,
    /// `unevidenced` findings dropped because the literal turned out to be
    /// in the capture after all. The model's claim was checkable and
    /// checked; this counts how often it was wrong, which is the only
    /// accuracy signal that kind has. Jev's literal leg counts its
    /// code-proposed candidates here instead — see [`Record::literals_refuted`].
    pub literals_refuted: u32,
    /// Findings dropped because the literal named no quantity — the model
    /// reaching for a name, a flag or a quantifier. A literal with a `/`
    /// or a path suffix is checkable (see [`is_checkable`]). Counted
    /// rather than silently discarded: this is the rate that says whether
    /// [`is_checkable`] is carrying the check or fighting it.
    pub not_a_quantity: u32,
    /// Findings dropped because [`kind_reported_for`] does not report that
    /// kind on this verb — today, `overreaches` on a `fact`. Counted for
    /// the same reason as the field above: this is the rate that says
    /// whether the bound is still describing the model's behaviour. If it
    /// falls to zero the model has stopped reaching for the kind and the
    /// bound is dead weight; if it climbs, the prompt is drifting toward
    /// the thing the bound exists to catch.
    pub kind_off_verb: u32,
    /// Findings a second model refuted, so the author never saw them.
    ///
    /// The one number that says what the refutation leg is doing. It is a
    /// rate, not a fault count: measured over the corpus it drops about two
    /// findings in three on `fact` and three in four on `prose`, and takes
    /// roughly one true finding in five with them. A run where it drops
    /// nothing is a leg that is not earning its call.
    pub refuted: u32,
    /// The first refuter call that did not complete — see
    /// [`Record::refuter_status`].
    pub refuter_status: Option<Status>,
    /// Every version a TypeSafe reply named — see [`Record::typed_versions`].
    pub typed_versions: Vec<String>,
    /// See [`Record::gate_calls`].
    pub gate_calls: u32,
    /// See [`Record::gate_status`].
    pub gate_status: Option<Status>,
    /// See [`Record::typed_classify_calls`].
    pub typed_classify_calls: Option<u32>,
    /// See [`Record::typed_literal_calls`].
    pub typed_literal_calls: Option<u32>,
}

impl Telemetry {
    /// Say why, in words the author may be shown. Clears any reply text an
    /// earlier write quoted, because that text explained the earlier
    /// detail and not this one.
    fn explain(&mut self, why: impl Into<String>) {
        self.detail = Some(why.into());
        self.reply = None;
    }

    /// Say why, and keep what the model or provider actually returned
    /// beside it for the log alone. See [`Record::reply`].
    fn explain_quoting(&mut self, why: impl Into<String>, reply: impl Into<String>) {
        self.detail = Some(why.into());
        self.reply = Some(reply.into());
    }
}

/// The first `n` characters of a reply, for [`Telemetry::explain_quoting`].
fn beginning(text: &str) -> String {
    text.chars().take(200).collect()
}

/// The legs a verification may run beside the check, as configured. Which
/// of them actually run on a verb is the verb's row in [`TYPED_LEGS`].
#[derive(Clone, Copy)]
struct Legs<'a> {
    literals: bool,
    refuter: Option<&'a str>,
    typed_model: Option<&'a str>,
}

#[allow(clippy::too_many_arguments)]
fn run(
    providers: &Providers,
    model: &str,
    approach: &str,
    legs: Legs<'_>,
    subject: &Subject,
    started: Instant,
    budget: Duration,
    tel: &mut Telemetry,
) -> (Status, Vec<Finding>, Option<Status>) {
    let Legs { literals, refuter, typed_model } = legs;
    // First, before classify, because that is the order it was measured in
    // on `fact` — and because a subject it skips should cost nothing else.
    // Only a skip ends the verification: a gate that fails, for whatever
    // reason, lets the check run and says so, because a gate that could
    // skip on its own failure would turn every TypeSafe outage into a run
    // of mints reported as having nothing to find.
    if let (Some(tm), Some(gate)) = (typed_model, typed_legs(&subject.verb).gate) {
        // `spawn` builds the TypeSafe endpoint whenever this leg runs and
        // starts nothing without one, so its absence here is a wiring fault;
        // it is reported as a failed gate, which checks, rather than as a skip.
        let verdict = match &providers.typed {
            Some(typed) => gate_skips(typed, tm, gate, subject, started, budget, tel),
            None => Err(Status::Unauthorized),
        };
        match verdict {
            Ok(true) => return (Status::Gated, Vec::new(), None),
            Ok(false) => {}
            Err(s) => tel.gate_status = Some(s),
        }
    }
    // `split` classifies the claim's assertions before checking them, so
    // the check can be told which ones the captured evidence is even
    // able to speak to. It costs a second call and it is the default,
    // because it is the configuration the retrodiction measured. One-call
    // comparisons have been run over the same corpus, but not this arm's
    // prompt pairing, and none of their numbers were carried into the
    // decision to ship. `direct` is cheaper, but not by half: one-call
    // arms that report disagreements cost 0.63-0.65 of `split` over the
    // corpus. Not because the call it drops is the cheap one — it carries
    // the claim text alone, yet on `claim` it measured 2026-09-21 at
    // $0.0054 of the $0.011 a `split` draw costs — but because the one call
    // `direct` keeps does both jobs.
    //
    // Where the verb's row says so, Jev labels in the LLM's place, and on
    // the same row it judges the literal leg below. Either typed leg fails
    // without the endpoint `spawn` built for it — a wiring fault, as it is
    // for the gate.
    let row = typed_legs(&subject.verb);
    let labelled = if let (true, Some(tm)) = (approach == "split", typed_model.filter(|_| row.classify)) {
        let labelled = match &providers.typed {
            Some(typed) => typed_classify(typed, tm, subject, started, budget, tel),
            None => Err(Status::Unauthorized),
        };
        match labelled {
            Ok(l) => l,
            Err(s) => return (s, Vec::new(), None),
        }
    } else if approach == "split" {
        let body = match call(&providers.llm, model, classify_system_for(&subject.verb), &classify_prompt(subject), started, budget, tel)
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
            Err((why, label)) => {
                match label {
                    Some(label) => tel.explain_quoting(why, label),
                    None => tel.explain(why),
                }
                return (Status::Unparsable, Vec::new(), None);
            }
        }
    } else {
        None
    };
    let prompt = check_prompt(subject, labelled.as_deref());
    match call(&providers.llm, model, check_system_for(&subject.verb), &prompt, started, budget, tel) {
        Ok(body) => match parse_findings(&body, subject, tel) {
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
                    let found = match (typed_model.filter(|_| row.literals), &providers.typed) {
                        (Some(tm), Some(typed)) => typed_literal_findings(typed, tm, subject, started, budget, tel),
                        (Some(_), None) => Err(Status::Unauthorized),
                        (None, _) => literal_findings(&providers.llm, model, subject, started, budget, tel),
                    };
                    match found {
                        Ok(mut l) => f.append(&mut l),
                        Err(s) => lit_status = Some(s),
                    }
                }
                // Last, over everything: an `unevidenced` finding is as
                // refutable as a disagreement, and the leg's whole value is
                // that it asks about a finding rather than about a subject.
                // It cannot fail the verification — see `refute_findings`.
                if let Some(r) = refuter {
                    f = refute_findings(providers, r, model, subject, f, started, budget, tel);
                }
                (Status::Ok, f, lit_status)
            }
            None => {
                // Say what could not be read. "Unparsable" alone sends
                // whoever is tuning this back to the provider to guess.
                tel.explain_quoting(
                    format!("reply was not a usable answer ({} bytes); `--spans` shows its beginning", body.len()),
                    beginning(&body),
                );
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
    llm: &Endpoint,
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
            tel.explain(format!(
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
        let v = post(llm, &body.to_string(), left, tel)?;
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
        tel.explain(format!(
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

/// One POST of `body` to `to`, bounded by what is left of the budget, and
/// the reply's decoded envelope. Shared by both providers, so a 429, a 500,
/// a timeout and an unreadable body map to the same statuses whichever
/// provider produced them.
fn post(
    to: &Endpoint,
    payload: &str,
    left: Duration,
    tel: &mut Telemetry,
) -> Result<serde_json::Value, Status> {
    let reply = ureq::post(&to.url)
        .header("Authorization", &format!("Bearer {}", to.key))
        .header("Content-Type", "application/json")
        .config()
        .timeout_global(Some(left))
        .build()
        .send(payload);
    let mut reply = match reply {
        Ok(r) => r,
        // A timeout inside the client is still the budget expiring;
        // anything else is transport or a non-2xx.
        Err(ureq::Error::Timeout(_)) => {
            tel.explain("provider did not answer within the remaining budget");
            return Err(Status::Timeout);
        }
        // The distinction that makes this field worth having: a 429,
        // a 500 and a name-resolution failure are all `unavailable`
        // and call for three different responses.
        Err(ureq::Error::StatusCode(code)) => {
            tel.explain(format!("provider replied {code}"));
            return Err(Status::Unavailable);
        }
        Err(e) => {
            tel.explain(format!("transport failure: {e}"));
            return Err(Status::Unavailable);
        }
    };
    let text = match reply.body_mut().read_to_string() {
        Ok(text) => text,
        // The budget covers the body as well as the headers. A provider can
        // answer 200 at once and send the reply only when the model is done,
        // so this is where a slow draw usually runs out: TET-84's 19
        // `unavailable` mints each stopped at the full budget.
        Err(ureq::Error::Timeout(_)) => {
            tel.explain("provider did not finish its reply within the remaining budget");
            return Err(Status::Timeout);
        }
        Err(e) => {
            tel.explain(format!("reply body could not be read: {e}"));
            return Err(Status::Unavailable);
        }
    };
    serde_json::from_str::<serde_json::Value>(&text).map_err(|_| {
        tel.explain(format!("provider envelope was not JSON ({} bytes)", text.len()));
        Status::Unparsable
    })
}

/// One TypeSafe call: `questions` — a serialised object, its keys in the
/// order they are to be asked — asked of `state`, and the reply's `answers`
/// object.
///
/// One attempt. There is no sampling to truncate, so the retry [`call`]
/// makes on a truncated draw has no counterpart here; the attempt still
/// counts in [`Telemetry::attempts`], which counts provider calls whoever
/// the provider is. The version the reply names is recorded before its
/// answers are read, so a reply that answers nothing still says who sent
/// it, and the call is priced from its input tokens — the reply carries
/// no price of its own.
fn ask_typed(
    typed: &Endpoint,
    model: &str,
    state: &str,
    questions: &str,
    started: Instant,
    budget: Duration,
    tel: &mut Telemetry,
) -> Result<serde_json::Value, Status> {
    let Some(left) = budget.checked_sub(started.elapsed()) else {
        tel.explain(format!(
            "budget of {}ms expired before an attempt could start",
            budget.as_millis()
        ));
        return Err(Status::Timeout);
    };
    tel.attempts += 1;
    // The endpoint names models without the vendor half that routed here.
    let name = model.split_once('/').map_or(model, |(_, m)| m);
    // In the harness's order, and `questions` in the order its author
    // wrote them — see [`json_ordered`].
    let body = json_ordered(&[
        ("state", json_str(state)),
        ("model", json_str(name)),
        ("questions", questions.to_string()),
    ]);
    let v = post(typed, &body, left, tel)?;
    // An unstated version is not the measured one, so it is recorded as
    // what it is rather than skipped, and is flagged downstream.
    let version = v["model"].as_str().unwrap_or("(unstated)").to_string();
    if !tel.typed_versions.contains(&version) {
        tel.typed_versions.push(version);
    }
    tel.cost += v["usage"]["input_tokens"].as_f64().unwrap_or(0.0) * TYPED_USD_PER_INPUT_TOKEN;
    match v.get("answers") {
        Some(a) if a.is_object() => Ok(a.clone()),
        _ => {
            tel.explain_quoting(
                "typed reply carried no answers; `--spans` shows its beginning",
                beginning(&v.to_string()),
            );
            Err(Status::Unparsable)
        }
    }
}

// ---------------------------------------------------------------------
// The gate. Ported from `scripts/verifier-eval/gate_variants.py`, whose
// `pick_with` produced both measured presentations; the splitter, the
// question wording, the order of the options and the score are all part
// of what was measured, so none of them is paraphrased here.
// ---------------------------------------------------------------------

/// How many questions one TypeSafe call carries; a gate with more than
/// this asks across several calls. The harness's bound, and one it never
/// reached with the presentations shipped: `fact`'s longest subject had 18
/// sentences, so 19 questions, and `claim` asks one question however many
/// clauses it offers (35 at most).
const MAX_TYPED_QUESTIONS: usize = 40;

/// What a disagreeing unit does. `gate_jev.py`'s wording, from
/// `CHECK_SYSTEM`.
const GATE_DISAGREES: &str = "asserts something the captured evidence, or the author's own \
figures elsewhere in the text, show to be otherwise";

/// `CHECK_SYSTEM`'s own rules about what is not a disagreement, put on the
/// NONE option because they describe the population the gate has to stay
/// quiet on.
const GATE_NOT_A_DISAGREEMENT: &str = "Also false when: the evidence merely fails to establish \
the text; the capture does not touch what the text is about; the text says less than the \
evidence shows; the text describes what a design PROPOSES to build, which evidence captured \
beforehand cannot contradict; or the reader is simply uncertain. Insufficiency is not \
disagreement.";

/// The three kinds `pick_cls` asks of each sentence, in the order it asked.
const GATE_KINDS: [(&str, &str); 3] = [
    (
        "CURRENT",
        "It describes the code, a tool's output or a measurement as it exists now — something \
the captured evidence could confirm or contradict.",
    ),
    (
        "PROPOSED",
        "It describes what this design will build, add or change. Evidence captured before the \
design exists cannot contradict it.",
    ),
    (
        "ARGUMENT",
        "It is reasoning, motivation, judgement or framing rather than a factual statement about \
the code.",
    ),
];

/// A unit shorter than this is not offered as an option — the harness's
/// `len(s) > 12`, counted in characters.
const MIN_UNIT_CHARS: usize = 13;

/// The author's text cut into sentences: at whitespace after `.`, `;` or
/// `:` when an upper-case letter, a backtick, `(`, `*` or a quote follows,
/// and at any blank line. `gate_variants.py`'s `SPLIT`, which uses a
/// look-behind the `regex` crate would not support and this crate does not
/// depend on; a run of whitespace is the separator in both, and every piece
/// is trimmed, so where exactly inside the run the cut falls does not matter.
fn gate_sentences(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if !text[i..].starts_with(char::is_whitespace) {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        // A whole run of whitespace, [i, end).
        let mut end = i;
        let mut newlines = 0;
        while let Some(c) = text[end..].chars().next().filter(|c| c.is_whitespace()) {
            newlines += usize::from(c == '\n');
            end += c.len_utf8();
        }
        let after_stop = i > 0 && matches!(bytes[i - 1], b'.' | b';' | b':');
        let starts_sentence = text[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || matches!(c, '`' | '(' | '*' | '"' | '\''));
        if (after_stop && starts_sentence) || newlines >= 2 {
            out.push(&text[start..i]);
            start = end;
        }
        i = end;
    }
    out.push(&text[start..]);
    out.into_iter()
        .map(str::trim)
        .filter(|s| s.chars().count() >= MIN_UNIT_CHARS)
        .collect()
}

/// A sentence cut again at `,`, `;`, `:`, ` —` and ` –` followed by
/// whitespace — `gate_variants.py`'s `CLAUSE`. Parentheses are not a
/// boundary: a figure is only checkable with its range still attached.
fn gate_clauses(sentence: &str) -> Vec<&str> {
    clause_spans(sentence, false).into_iter().map(|(a, b)| &sentence[a..b]).collect()
}

/// Where [`gate_clauses`] cuts, as byte spans of `sentence`, separators
/// excluded. With `depth0`, never inside brackets or a code span —
/// `classify_jev.py`'s `depth0_clauses`: a backtick toggles a code span,
/// and outside one `(`, `[` and `{` open a level that `)`, `]` and `}`
/// close, never below zero. The plain cut made "{ id, proposition, cited
/// fact ids, withdrawn }" four one-word units, and Jev labelled
/// `proposition` on its own; an enumeration is one assertion.
fn clause_spans(sentence: &str, depth0: bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut start, mut depth, mut code, mut i) = (0, 0usize, false, 0);
    while i < sentence.len() {
        let rest = &sentence[i..];
        let ch = rest.chars().next().unwrap_or_default();
        if depth0 && ch == '`' {
            code = !code;
        } else if depth0 && !code && "([{".contains(ch) {
            depth += 1;
        } else if depth0 && !code && ")]}".contains(ch) {
            depth = depth.saturating_sub(1);
        } else if !code && depth == 0 {
            let sep = [",", ";", ":", " —", " –"].into_iter().find(|p| rest.starts_with(p));
            if let Some(p) = sep {
                let ws: usize = sentence[i + p.len()..]
                    .chars()
                    .take_while(|c| c.is_whitespace())
                    .map(char::len_utf8)
                    .sum();
                if ws > 0 {
                    out.push((start, i));
                    start = i + p.len() + ws;
                    i = start;
                    continue;
                }
            }
        }
        i += ch.len_utf8().max(1);
    }
    out.push((start, sentence.len()));
    out
}

/// The units the gate offers for `text`: sentences, or every sentence's
/// clauses.
fn gate_units(text: &str, unit: GateUnit) -> Vec<&str> {
    let sentences = gate_sentences(text);
    match unit {
        GateUnit::Sentence => sentences,
        GateUnit::Clause => sentences
            .into_iter()
            .flat_map(gate_clauses)
            .map(str::trim)
            .filter(|c| c.chars().count() >= MIN_UNIT_CHARS)
            .collect(),
    }
}

/// The gate's questions, in the order the harness sent them: `which` — one
/// choice over the units plus NONE, so they compete for one probability and
/// a clean subject puts it on NONE — then, for `pick_cls`, one kind
/// question per unit. Each entry is a key and its serialised question.
fn gate_questions(gate: Gate, units: &[&str]) -> Vec<(String, String)> {
    let word = gate.unit.word();
    let mut options: Vec<(String, String)> =
        units.iter().enumerate().map(|(i, u)| (format!("S{}", i + 1), json_str(u))).collect();
    options.push((
        "NONE".into(),
        json_str(&format!("No {word} disagrees with the evidence. {GATE_NOT_A_DISAGREEMENT}")),
    ));
    let which = json_ordered(&[
        ("type", json_str("choice")),
        (
            "instructions",
            json_str(&format!(
                "Which {word} of the author's text disagrees with the captured evidence? A \
                 disagreeing {word} {GATE_DISAGREES}."
            )),
        ),
        ("criteria", json_ordered(&options)),
    ]);
    let mut questions = vec![("which".to_string(), which)];
    if gate.classify {
        let kinds: Vec<(&str, String)> = GATE_KINDS.iter().map(|(k, d)| (*k, json_str(d))).collect();
        let kinds = json_ordered(&kinds);
        for (i, u) in units.iter().enumerate() {
            questions.push((
                format!("k{}", i + 1),
                json_ordered(&[
                    ("type", json_str("choice")),
                    ("criteria", kinds.clone()),
                    (
                        "instructions",
                        json_str(&format!("What kind of {word} is this?\n{}: {u}", word.to_uppercase())),
                    ),
                ]),
            ));
        }
    }
    questions
}

/// The gate's score from its answers, or `None` unless the choice named a
/// probability for every option it was offered.
///
/// `pick_cls` sums each unit's share of the choice weighted by its CURRENT
/// probability, a unit whose kind went unanswered counting whole;
/// `pick_clause` is everything the choice did not put on NONE.
///
/// The harness scored a missing probability as zero — on the pick, and
/// through its NONE default on the clause score — and so would skip on it.
/// Here that would let a malformed answer end a verification as `gated`,
/// so an answer that leaves any option out is not one. Every measured
/// reply named them all: over the 3,234 subjects scored by the `pick_cls`
/// and `pick_clause` families, none was missing.
fn gate_score(gate: Gate, units: usize, answers: &serde_json::Value) -> Option<f64> {
    let probs = answers["which"]["probabilities"].as_object()?;
    let p = |k: &str| probs.get(k).and_then(serde_json::Value::as_f64);
    let picks: Vec<f64> = (1..=units).map(|i| p(&format!("S{i}"))).collect::<Option<_>>()?;
    let none = p("NONE")?;
    if !gate.classify {
        return Some(1.0 - none);
    }
    Some(
        picks
            .iter()
            .enumerate()
            .map(|(i, pick)| {
                let current = answers[format!("k{}", i + 1)]["probabilities"]["CURRENT"].as_f64().unwrap_or(1.0);
                pick * current
            })
            .sum(),
    )
}

/// Whether the gate skips `subject`: `Ok(true)` to skip, `Ok(false)` to
/// check, and the status of the call that failed otherwise.
///
/// A subject with no unit long enough to offer is checked without asking.
/// The harness would have asked a choice with NONE as its only option and
/// skipped on the certain answer, which is a skip nothing measured.
fn gate_skips(
    typed: &Endpoint,
    typed_model: &str,
    gate: Gate,
    subject: &Subject,
    started: Instant,
    budget: Duration,
    tel: &mut Telemetry,
) -> Result<bool, Status> {
    let units = gate_units(&subject.text, gate.unit);
    if units.is_empty() {
        return Ok(false);
    }
    // `gate_jev.py`'s state, byte for byte: the author's text first and the
    // evidence after it. The reverse order was measured and lost.
    let state = format!("AUTHOR'S TEXT:\n{}\n\n{}", subject.text, evidence_text(subject));
    let questions = gate_questions(gate, &units);
    let answers =
        ask_chunked(typed, typed_model, &state, &questions, started, budget, tel, |t| t.gate_calls += 1)?;
    match gate_score(gate, units.len(), &answers) {
        Some(score) => Ok(score < gate.threshold),
        None => {
            tel.explain("the gate's reply did not give a probability for every option it offered");
            Err(Status::Unparsable)
        }
    }
}

// ---------------------------------------------------------------------
// Classify, asked of Jev. Ported from `scripts/verifier-eval/classify_jev.py`
// at the presentation the design measured (`--unit clause0`, τ 0.4): the
// split is mechanical, because `CLASSIFY_SYSTEM`'s own rule is that the
// step only sorts the author's words, and Jev only labels.
// ---------------------------------------------------------------------

/// `CLASSIFY_SYSTEM`'s three definitions, as the harness offered them.
const TYPED_CLASSIFY_CRITERIA: [(&str, &str); 3] = [
    ("current", "asserts how the code, files or tools behave TODAY. Checkable against captured evidence."),
    (
        "proposed",
        "asserts what THIS DESIGN will build, add, change, or recommend. The evidence was captured \
before that change exists, so it cannot speak to this.",
    ),
    (
        "argument",
        "a reason, a decision, an entailment, or a statement about what is right or necessary. \
Nothing captured can settle it.",
    ),
];

/// A part is labelled `current` at this probability of it, whatever the
/// choice: hiding a current clause from the check loses a correct
/// warning, which costs more than a proposal read as current.
const TYPED_CURRENT_AT: f64 = 0.4;

/// The parts Jev labels: every sentence's clauses, cut outside brackets
/// and code spans, keeping those longer than three characters.
fn classify_units(text: &str) -> Vec<&str> {
    gate_sentences(text)
        .into_iter()
        .flat_map(|s| clause_spans(s, true).into_iter().map(move |(a, b)| s[a..b].trim()))
        .filter(|u| u.chars().count() > 3)
        .collect()
}

/// One `choice` per part, keyed `u0`, `u1`, … in text order.
fn classify_questions(units: &[&str]) -> Vec<(String, String)> {
    let criteria: Vec<(&str, String)> =
        TYPED_CLASSIFY_CRITERIA.iter().map(|(k, d)| (*k, json_str(d))).collect();
    let criteria = json_ordered(&criteria);
    units
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let question = json_ordered(&[
                ("type", json_str("choice")),
                ("criteria", criteria.clone()),
                (
                    "instructions",
                    json_str(&format!(
                        "You are given one claim from a software design memo (the state). Label ONE \
                         part of it by what that part asserts.\nPART: {u}"
                    )),
                ),
            ]);
            (format!("u{i}"), question)
        })
        .collect()
}

/// One part's label from its answer, or `None` unless the answer named a
/// probability for every label and, where `current` falls short of
/// [`TYPED_CURRENT_AT`], a choice among them.
///
/// The harness read a missing probability as zero. Here that would label
/// a part by a choice made on an answer that was not whole, so it fails
/// the leg instead — the rule the gate follows.
fn classify_label(answer: &serde_json::Value) -> Option<&'static str> {
    let probs = answer["probabilities"].as_object()?;
    let p = |k: &str| probs.get(k).and_then(serde_json::Value::as_f64);
    let current = p("current")?;
    p("proposed")?;
    p("argument")?;
    if current >= TYPED_CURRENT_AT {
        return Some("current");
    }
    let choice = answer["choice"].as_str()?;
    CLASSIFY_LABELS.into_iter().find(|l| *l == choice)
}

/// `split`'s assertions, labelled by Jev, in the shape [`parse_assertions`]
/// emits — so the check prompt cannot tell which classifier ran — or
/// `None` when the claim offers no part to label, which checks it
/// unlabelled, as `direct` does, rather than asking about nothing.
fn typed_classify(
    typed: &Endpoint,
    typed_model: &str,
    subject: &Subject,
    started: Instant,
    budget: Duration,
    tel: &mut Telemetry,
) -> Result<Option<String>, Status> {
    tel.typed_classify_calls = Some(0);
    let units = classify_units(&subject.text);
    if units.is_empty() {
        return Ok(None);
    }
    let state = format!("CLAIM:\n{}", subject.text);
    let answers = ask_chunked(typed, typed_model, &state, &classify_questions(&units), started, budget, tel, |t| {
        *t.typed_classify_calls.get_or_insert(0) += 1;
    })?;
    let mut assertions = Vec::new();
    for (i, u) in units.iter().enumerate() {
        let Some(label) = classify_label(&answers[format!("u{i}")]) else {
            tel.explain(format!("typed classify gave no usable label for part u{i}"));
            return Err(Status::Unparsable);
        };
        assertions.push(json!({"text": u, "label": label}));
    }
    Ok(Some(json!({"assertions": assertions}).to_string()))
}

// ---------------------------------------------------------------------
// The literal leg, asked of Jev. Ported from
// `scripts/verifier-eval/literals_jev.py` at the presentation the design
// measured (two nouls, `--q 0.7 --c 0.5`): code proposes every candidate
// and applies the shipped filters, and Jev only judges what survives.
// ---------------------------------------------------------------------

/// What makes a literal worth reporting, asked of each candidate as `q{i}`.
const QUANTITY_TRUE: &str = "it is a QUANTITY the text states as current fact — a value that could \
be wrong by counting or arithmetic: a count, a size, a byte or line count, a duration, a percentage \
or proportion, a threshold, an index range used as a measurement — or a FILE the text says it read \
or that carries something";
const QUANTITY_FALSE: &str = "it is not: a symbol, function, type, module or field name; a flag, \
option or setting name; a version string; an identifier for a ticket, section, check or numbered \
item; a line or byte range saying WHERE something is rather than HOW MUCH; a quoted phrase the text \
discusses; a quantifier (any, every, no, only, always); a quantity in what this design WILL build; a \
quantity inside a reason, a decision or an entailment; or a number that measures nothing (\"two \
reasons\", \"one call\")";
/// Whether the capture carries it in another form, asked as `c{i}` — the
/// arithmetic the substring filter cannot do.
const CARRIED_TRUE: &str = "the captured evidence carries this value, possibly in another form: \
`14_000` backs \"14,000 bytes\"; a capture of lines 1-40 backs \"40 lines\"; `MAX_ATTEMPTS: u32 = \
3` backs \"retries three times\"; 5 of 6 visible backs \"83%\"";
const CARRIED_FALSE: &str = "it does not: the value appears nowhere in what was captured, or the \
capture shows a DIFFERENT value (two timestamps 910 apart do not carry \"918 seconds\"), or the \
value may lie in material that was truncated and not shown";

/// A candidate is reported when P(quantity) reaches this…
const TYPED_QUANTITY_AT: f64 = 0.7;
/// …and P(carried) stays below this.
const TYPED_CARRIED_BELOW: f64 = 0.5;

/// Why a Jev literal finding was raised. Jev writes no words, so the
/// finding carries this rather than a model's reason, and never its
/// probabilities.
const TYPED_LITERAL_WHY: &str =
    "The text states this as current fact, and no captured observation contains it.";

/// `\w`, as Python's `re` reads it in a `str` pattern.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every quantity- or path-shaped literal in `text`, in the harness's
/// order and without repeats: each figure with the word it counts ("918
/// seconds", "one glob"), then each path with its backticks dropped.
///
/// `literals_jev.py`'s `FIGURE`, `NOUN` and `PATH` regexes, matched by
/// hand because this crate has no regex dependency. Each is walked the
/// way `re.finditer` walks it — leftmost match, retried a character on
/// after a miss — and tries its alternatives in the order Python's
/// backtracking would, so the first match either finds is the same one.
fn literal_candidates(text: &str) -> Vec<&str> {
    let at: Vec<(usize, char)> = text.char_indices().collect();
    let byte = |k: usize| at.get(k).map_or(text.len(), |(b, _)| *b);
    let mut found = Vec::new();
    let mut k = 0;
    while k < at.len() {
        match figure_at(&at, k) {
            Some(end) => {
                found.push(text[byte(k)..noun_after(&at, end).map_or(byte(end), byte)].trim());
                k = end;
            }
            None => k += 1,
        }
    }
    let mut k = 0;
    while k < at.len() {
        match path_at(&at, k) {
            Some(end) => {
                found.push(text[byte(k)..byte(end)].trim_matches('`'));
                k = end;
            }
            None => k += 1,
        }
    }
    let mut out: Vec<&str> = Vec::new();
    for c in found {
        if !c.is_empty() && text.contains(c) && !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// `FIGURE` at char index `k`: the end of a match, or `None`.
///
/// `(?<![\w.:/#-])\d[\d,_]*(?:\.\d+)?%?(?![\w])`, then, case-insensitively,
/// `(?<![\w-])(?:zero|one|…|twelve)(?![\w-])`.
///
/// `\d` is read as an ASCII digit where Python reads any decimal digit
/// (std has no test for exactly that class). So a figure in another
/// script, `３` or `٣`, is proposed by the harness and not here. Alone it
/// is no quantity to either — [`is_checkable`], like the harness's
/// `is_quantity`, needs an ASCII digit, a path or a number word — but with
/// a counted number word ("３ ten-minute passes") or beside an ASCII digit
/// ("٣4 files") the harness would ask Jev about it and this does not. No
/// measured claim has such a figure. Likewise `trim` keeps the ASCII
/// separators `\x1c`–`\x1f` that Python's `strip` removes.
fn figure_at(at: &[(usize, char)], k: usize) -> Option<usize> {
    let ch = |i: usize| at.get(i).map(|(_, c)| *c);
    let prev = k.checked_sub(1).and_then(ch);
    let ends_word = |i: usize| !ch(i).is_some_and(is_word);
    if ch(k).is_some_and(|c| c.is_ascii_digit())
        && !prev.is_some_and(|c| is_word(c) || ".:/#-".contains(c))
    {
        let mut run = k + 1;
        while ch(run).is_some_and(|c| c.is_ascii_digit() || c == ',' || c == '_') {
            run += 1;
        }
        // Backtracking order: the greediest `[\d,_]*` first, and within it
        // the decimal part longest-first, then none; `%` before no `%`.
        for r in (k + 1..=run).rev() {
            let mut ends = Vec::new();
            if ch(r) == Some('.') && ch(r + 1).is_some_and(|c| c.is_ascii_digit()) {
                let mut d = r + 2;
                while ch(d).is_some_and(|c| c.is_ascii_digit()) {
                    d += 1;
                }
                ends.extend((r + 2..=d).rev());
            }
            ends.push(r);
            for e in ends {
                if ch(e) == Some('%') && ends_word(e + 1) {
                    return Some(e + 1);
                }
                if ends_word(e) {
                    return Some(e);
                }
            }
        }
    }
    if prev.is_some_and(|c| is_word(c) || c == '-') {
        return None;
    }
    NUMBER_WORDS.iter().find_map(|w| {
        let n = w.chars().count();
        let same = w.chars().enumerate().all(|(i, wc)| ch(k + i).is_some_and(|c| c.to_ascii_lowercase() == wc));
        (same && !ch(k + n).is_some_and(|c| is_word(c) || c == '-')).then_some(k + n)
    })
}

/// `NOUN` at char index `k`, `\s+(\(?[A-Za-z][\w'-]*\)?)`: the end of the
/// word a figure counts, unless that word opens with `(`.
fn noun_after(at: &[(usize, char)], k: usize) -> Option<usize> {
    let ch = |i: usize| at.get(i).map(|(_, c)| *c);
    let mut i = k;
    while ch(i).is_some_and(char::is_whitespace) {
        i += 1;
    }
    if i == k || !ch(i).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    i += 1;
    while ch(i).is_some_and(|c| is_word(c) || c == '\'' || c == '-') {
        i += 1;
    }
    Some(i + usize::from(ch(i) == Some(')')))
}

/// `PATH` at char index `k`: the end of a match, or `None`.
///
/// `` `?(?:[\w.-]+/)*[\w.-]+\.(?:rs|py|…|html)\b`? ``, then
/// `` `?(?:[\w.-]+/)+[\w.-]*`? ``. A segment's run cannot contain its `/`,
/// so each segment has one length and backtracking only gives whole
/// segments back.
fn path_at(at: &[(usize, char)], k: usize) -> Option<usize> {
    let ch = |i: usize| at.get(i).map(|(_, c)| *c);
    let in_class = |i: usize| ch(i).is_some_and(|c| is_word(c) || c == '.' || c == '-');
    let run_from = |mut i: usize| {
        while in_class(i) {
            i += 1;
        }
        i
    };
    let tick = |i: usize| i + usize::from(ch(i) == Some('`'));
    let start = tick(k);
    // Where each segment ends, after its `/`.
    let mut segments = vec![start];
    loop {
        let from = *segments.last().unwrap_or(&start);
        let r = run_from(from);
        if r > from && ch(r) == Some('/') {
            segments.push(r + 1);
        } else {
            break;
        }
    }
    for &t in segments.iter().rev() {
        let r = run_from(t);
        for x in (t + 1..r).rev() {
            if ch(x) != Some('.') {
                continue;
            }
            for ext in PATH_SUFFIXES {
                let ext = &ext[1..];
                let n = ext.chars().count();
                let same = ext.chars().enumerate().all(|(i, e)| ch(x + 1 + i) == Some(e));
                if same && !ch(x + 1 + n).is_some_and(is_word) {
                    return Some(tick(x + 1 + n));
                }
            }
        }
    }
    let last = *segments.last().unwrap_or(&start);
    (segments.len() > 1).then(|| tick(run_from(last)))
}

/// The clause a literal sits in, for its question and its finding:
/// `literals_jev.py`'s `clause_of`, which finds the literal's first
/// occurrence and each sentence's first occurrence, cuts that sentence as
/// the gate does, and falls back to the sentence and then the text.
fn literal_clause<'a>(text: &'a str, literal: &str) -> &'a str {
    let Some(i) = text.find(literal) else { return text };
    for s in gate_sentences(text) {
        let Some(j) = text.find(s) else { continue };
        if j <= i && i < j + s.len() {
            return clause_spans(s, false)
                .into_iter()
                .find(|&(a, b)| a <= i - j && i - j < b)
                .map_or(s, |(a, b)| s[a..b].trim());
        }
    }
    text
}

/// Two `noul`s per candidate, `q{i}` then `c{i}`.
fn literal_questions(text: &str, literals: &[&str]) -> Vec<(String, String)> {
    let criteria = |t: &str, f: &str| json_ordered(&[("true", json_str(t)), ("false", json_str(f))]);
    let noul = |criteria: String, instructions: String| {
        json_ordered(&[("type", json_str("noul")), ("criteria", criteria), ("instructions", json_str(&instructions))])
    };
    let mut out = Vec::new();
    for (i, lit) in literals.iter().enumerate() {
        let cl = literal_clause(text, lit);
        out.push((
            format!("q{i}"),
            noul(
                criteria(QUANTITY_TRUE, QUANTITY_FALSE),
                format!(
                    "In the author's text, the literal «{lit}», in the clause «{cl}», is a quantity \
                     stated as current fact or a file the text says it read."
                ),
            ),
        ));
        out.push((
            format!("c{i}"),
            noul(
                criteria(CARRIED_TRUE, CARRIED_FALSE),
                format!("The captured evidence carries the value of «{lit}», as the author's clause «{cl}» uses it."),
            ),
        ));
    }
    out
}

/// [`literal_findings`] with Jev in the LLM's place: code proposes the
/// candidates, the shipped filters run in the shipped order and are
/// counted as they are there, and Jev judges each survivor.
///
/// An answer that leaves out either probability of any candidate fails
/// the leg, as a failed LLM call does — the harness read it as a candidate
/// not kept, which would report "found nothing" for "was not answered".
fn typed_literal_findings(
    typed: &Endpoint,
    typed_model: &str,
    subject: &Subject,
    started: Instant,
    budget: Duration,
    tel: &mut Telemetry,
) -> Result<Vec<Finding>, Status> {
    tel.typed_literal_calls = Some(0);
    // Kept for the same reason as in `literal_findings`: with nothing
    // captured, the filter that makes this kind worth trusting cannot
    // reject anything.
    if subject.evidence.iter().all(|(_, _, obs)| obs.is_empty()) {
        return Ok(Vec::new());
    }
    let mut judged = Vec::new();
    for lit in literal_candidates(&subject.text) {
        if subject.in_captured_output(lit) {
            tel.literals_refuted += 1;
        } else if !is_checkable(lit) {
            tel.not_a_quantity += 1;
        } else {
            judged.push(lit);
        }
    }
    if judged.is_empty() {
        return Ok(Vec::new());
    }
    let state = format!("TEXT:\n{}\n\n{}", subject.text, evidence_text(subject));
    let questions = literal_questions(&subject.text, &judged);
    let answers = ask_chunked(typed, typed_model, &state, &questions, started, budget, tel, |t| {
        *t.typed_literal_calls.get_or_insert(0) += 1;
    })?;
    let mut out = Vec::new();
    for (i, lit) in judged.into_iter().enumerate() {
        let p = |key: String| answers[key]["noul"].as_f64();
        let (Some(quantity), Some(carried)) = (p(format!("q{i}")), p(format!("c{i}"))) else {
            tel.explain(format!("typed literal leg gave no probability for q{i} or c{i}"));
            return Err(Status::Unparsable);
        };
        if quantity < TYPED_QUANTITY_AT || carried >= TYPED_CARRIED_BELOW {
            continue;
        }
        let clause = literal_clause(&subject.text, lit).to_string();
        out.push(Finding {
            kind: KIND_UNEVIDENCED.to_string(),
            clause_quoted: subject.text.contains(&clause),
            clause,
            facts: Vec::new(),
            quoted_from: None,
            legacy_fact: None,
            evidence: None,
            literal: Some(lit.to_string()),
            why: TYPED_LITERAL_WHY.to_string(),
            quoted: false,
            rejected_span: None,
        });
    }
    Ok(out)
}

/// `questions` asked [`MAX_TYPED_QUESTIONS`] at a time, in order, and
/// their answers as one object; `count` is told of each call before it is
/// made. `GV.ask_chunked`, which every typed leg was measured through.
#[allow(clippy::too_many_arguments)]
fn ask_chunked(
    typed: &Endpoint,
    typed_model: &str,
    state: &str,
    questions: &[(String, String)],
    started: Instant,
    budget: Duration,
    tel: &mut Telemetry,
    count: fn(&mut Telemetry),
) -> Result<serde_json::Value, Status> {
    let mut answers = serde_json::Map::new();
    for chunk in questions.chunks(MAX_TYPED_QUESTIONS) {
        count(tel);
        let a = ask_typed(typed, typed_model, state, &json_ordered(chunk), started, budget, tel)?;
        if let serde_json::Value::Object(a) = a {
            answers.extend(a);
        }
    }
    Ok(serde_json::Value::Object(answers))
}

/// `s` as a JSON string.
fn json_str(s: &str) -> String {
    serde_json::Value::from(s).to_string()
}

/// A JSON object whose keys go out in the order given; values are already
/// serialised.
///
/// Needed because this crate's `serde_json` is built without
/// `preserve_order`, so a `json!` object is sent with its keys sorted — and
/// a typed question's options are its keys. The gate's would go out as
/// NONE, S1, S10, S2, … where every measurement offered S1 … Sn, NONE.
fn json_ordered<K: AsRef<str>>(pairs: &[(K, String)]) -> String {
    let body: Vec<String> =
        pairs.iter().map(|(k, v)| format!("{}:{v}", json_str(k.as_ref()))).collect();
    format!("{{{}}}", body.join(","))
}

/// Recover the reply's JSON object and read the disagreements out of it.
///
/// Three distinct shapes are unusable and none of them is a transport
/// failure: no brace-delimited substring at all, a substring that does not
/// parse, and a decoded object whose `kind` is outside the two permitted
/// values. `None` here becomes [`Status::Unparsable`], which is what keeps
/// an unreadable answer from arriving as a clean bill.
fn parse_findings(body: &str, subject: &Subject, tel: &mut Telemetry) -> Option<Vec<Finding>> {
    let v = json_object(body)?;
    let rows = v.get("disagreements")?.as_array()?;
    let mut out = Vec::new();
    // Counted locally and committed only if the whole reply parses. A later
    // row can still take the answer to `Unparsable`, and a drop attributed
    // to a verification that delivered nothing is a drop no reader can
    // account for.
    let mut off_verb = 0;
    for row in rows {
        let kind = str_field(row, "kind");
        if kind != KIND_CONTRADICTS && kind != KIND_OVERREACHES {
            return None;
        }
        // Dropped, not refused. The kind is a real one and the reply is a
        // good reply; it is this verb that has no use for it, so the row
        // goes and the rest of the answer stands.
        if !kind_reported_for(&subject.verb, &kind) {
            off_verb += 1;
            continue;
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
    tel.kind_off_verb += off_verb;
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
///
/// The error carries the explanation and, apart from it, the model's own
/// text when that is what went wrong, so that only the explanation can
/// reach the author — see [`Record::reply`].
fn parse_assertions(body: &str, claim: &str, tel: &mut Telemetry) -> Result<String, (String, Option<String>)> {
    let fixed = |why: &str| (why.to_string(), None);
    let v = json_object(body).ok_or_else(|| fixed("classify reply held no JSON object"))?;
    let rows = v
        .get("assertions")
        .and_then(|a| a.as_array())
        .ok_or_else(|| fixed("classify reply had no `assertions` array"))?;
    let mut kept = Vec::new();
    for row in rows {
        let text = str_field(row, "text");
        let label = str_field(row, "label");
        if !CLASSIFY_LABELS.contains(&label.as_str()) {
            // The label is the model's own text, so it goes beside the
            // explanation rather than into it.
            return Err((
                format!("classify returned a label that is not one of {}", CLASSIFY_LABELS.join(", ")),
                Some(label),
            ));
        }
        if text.is_empty() || !claim.contains(&text) {
            tel.not_verbatim += 1;
            continue;
        }
        kept.push(json!({"text": text, "label": label}));
    }
    if kept.is_empty() {
        return Err((
            format!("classify returned {} assertion(s), none of them quoted verbatim from the claim", rows.len()),
            None,
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
///      the entire assertion, and [`Subject::in_captured_output`] answers
///      exactly that.
///
/// Filter 2 is what makes this kind cheap to trust relative to the other
/// two: a `contradicts` finding rests on the model's reading, while an
/// `unevidenced` one rests on a substring search anyone can rerun. It also
/// biases hard toward silence — a literal occurring incidentally anywhere
/// in the captured output is dropped, so a `40` in some unrelated line of
/// the capture suppresses a genuine finding about a different `40`.
/// Under-reporting is the right direction for an advisory that costs the
/// author attention.
///
/// The predicate is deliberately **not** [`Subject::containing`], which
/// this filter did share for one commit. That one also matches the extent
/// labels, and labels are generated by the tool rather than captured by
/// it — `lines 4000-4096` is not evidence that a note's "4096 bytes" is
/// grounded in anything. Sharing it turned the incidental-match bias above
/// from a bounded conservatism into a silent one, since the suppression
/// was counted as a machine refutation.
fn literal_findings(
    llm: &Endpoint,
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
    let body = call(llm, model, LITERALS_SYSTEM, &prompt, started, budget, tel)?;
    let Some(v) = json_object(&body) else {
        tel.explain_quoting(
            format!("literal check replied with no JSON object ({} bytes); `--spans` shows its beginning", body.len()),
            beginning(&body),
        );
        return Err(Status::Unparsable);
    };
    let Some(rows) = v.get("unevidenced").and_then(|u| u.as_array()) else {
        tel.explain("literal check reply had no `unevidenced` array");
        return Err(Status::Unparsable);
    };
    let mut out = Vec::new();
    for row in rows {
        let literal = str_field(row, "literal");
        if literal.is_empty() || !subject.text.contains(&literal) {
            tel.not_verbatim += 1;
            continue;
        }
        if subject.in_captured_output(&literal) {
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

/// How a subject is announced to both calls.
///
/// A design paragraph headed `CLAIM:` was being split as an assertion about
/// today: 11 of 38 wrong prose findings objected to what the design
/// *proposes* as though it described current code. Announcing it as what it
/// is, together with [`PROSE_CLASSIFY_SYSTEM`], took that cluster to zero.
///
/// `fact` keeps `CLAIM:`. A note is not a claim either and the header is
/// just as wrong there, but every `fact` number on record was measured with
/// it, and changing it would ship something no run has scored.
fn header_for(verb: &str) -> &'static str {
    match verb {
        "prose" => "PARAGRAPH",
        _ => "CLAIM",
    }
}

fn classify_prompt(subject: &Subject) -> String {
    format!("{}:\n{}", header_for(&subject.verb), subject.text)
}

fn check_prompt(subject: &Subject, labelled: Option<&str>) -> String {
    match labelled {
        Some(l) => format!(
            "{}:\n{}\n\nASSERTIONS:\n{}\n\n{}",
            header_for(&subject.verb),
            subject.text,
            l,
            evidence_text(subject)
        ),
        None => format!(
            "{}:\n{}\n\n{}",
            header_for(&subject.verb),
            subject.text,
            evidence_text(subject)
        ),
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

/// The check prompt for a `fact`, in place of [`CHECK_SYSTEM`].
///
/// `CHECK_SYSTEM` addresses a *claim*, and the two failure clusters it
/// carried on notes are exactly where a note differs from one: a note is a
/// record of a single capture, so its clauses are terse, scope-bound, and
/// quote code loosely. Adjudicated against the full capture, its 28
/// surviving findings over the corpus were 11 right and 17 wrong — 6 of
/// them objecting to a clause read wider than its own sentence, 3 to
/// "verbatim" against a lossy UTF-8 decode. This prompt's numbered rules
/// are written from those cases and answer 11 of the 17, at a cost of 2 of
/// the 11 catches: 10 right of 16, 63% against 39%.
///
/// **It still describes `overreaches`, and that is deliberate.** The
/// measured configuration is this prompt with the kind dropped afterwards
/// by [`kind_reported_for`], not a prompt with the kind written out of it.
/// Instructing a model away from a move relocates it — measured twice on
/// the literal check and twice here — so the bound stays mechanical and
/// this text stays as it was measured. Removing these paragraphs would
/// ship something no run has scored.
const FACT_SYSTEM: &str = r#"You are given a NOTE an author wrote to summarise what they had just read, and the evidence a tool
captured at the same moment. The note is a record of that capture and nothing else. Report only
DISAGREEMENTS between the two.

There are exactly two kinds:

  contradicts — the captured evidence shows something incompatible with the note: a different
                number, name, type, line, or behaviour.
  overreaches — the note ranges wider than what was captured. It says "every", "never", "only",
                "no", "always", "any" or "cannot" about a population the capture samples rather
                than covers.

Four rules decide most cases, and each exists because reports failed on it:

1. THE CAPTURE MAY STATE THE GENERAL FACT ITSELF. Captured source carries doc comments, code
   comments and docstrings, and those often assert a general property — "Only the label is
   rendered", "every string in this list is static". A note repeating what such a comment says is
   REPORTING the capture, not generalising beyond it. That is never an overreach. Ask where the
   generality came from: if it is in the captured text, the note did not invent it.

2. READ THE NOTE'S WHOLE SENTENCE BEFORE OBJECTING TO A CLAUSE. A clause is scoped by the words
   around it. "No line is skipped" inside a sentence about parse failures is about parse failures,
   and a note that elsewhere says blank lines are skipped has not contradicted itself. Quote a
   clause only with the scope its own sentence gives it.

3. WOULD THE OBJECTION CHANGE WHAT THE NOTE TELLS A READER? If the note's substance survives it,
   it is not a disagreement. "Verbatim" where bytes pass through a lossy UTF-8 decode, "line
   boundary" where a trailing space is trimmed, "byte-equivalent" where two identical expressions
   differ in line breaks — these are word-level objections to notes that are telling the reader
   something true. Say nothing.

4. IF YOUR OWN REASONING ARRIVES AT "SO THERE IS NO DISAGREEMENT", REPORT NOTHING. Do not write the
   finding out anyway with the reasoning attached.

Also never report:

  * a difference between the note and anything other than this capture
  * that the capture fails to ESTABLISH the note — insufficiency is not disagreement
  * "the capture does not touch X" — that is insufficiency phrased as a missing scope
  * a note saying LESS than the capture shows
  * your own uncertainty

The captured output may be truncated and says so where it was. Never report a disagreement resting
on material you were not shown.

QUOTING. Name the failing clause, verbatim from the note. Then quote the evidence, verbatim, from
EITHER block you were given:

  * for `contradicts`, quote from the captured output — the text that conflicts;
  * for `overreaches`, the "what was opened or run" block is usually the right evidence and is
    fully quotable. A line like "search: /repo (grep (ERE): foo) — 10 files matched" is what shows the
    note reached past its own capture. Prefer it to inventing an output span.

Both quotations must be copied character for character. A quotation that cannot be found in what
you were shown is worse than none, because it sends the reader to check against text that does not
exist.

Reply with one JSON object and nothing else:
{"disagreements": [{"kind": "contradicts"|"overreaches", "clause": "", "evidence": "", "why": ""}]}

An empty list is the common and correct answer."#;

/// Put one finding to a second model and ask whether it is correct.
///
/// The prompt `refute.py` measured, near enough verbatim. It is the
/// adjudication brief a human pass used, not a new one written for the
/// occasion, and it is the only prompt here with per-finding ground truth
/// behind it.
///
/// **Its authority is that finding and checking are different questions.** A
/// single pass asked to FIND disagreements never exercises the judgement
/// that rejects a bad one, which is why two rounds of rewording moved
/// `fact` from 3/14 to 3/14 while asking the other question moved it to
/// 8/9. The same model asked to refute itself scored 17% — near-random —
/// so the second asker has to be a different one.
const REFUTE_SYSTEM: &str = r#"You are given a note or paragraph an author wrote, the evidence a tool captured,
and someone's assertion that a specific clause disagrees with that evidence. Decide whether the
assertion is correct.

There are two kinds it may claim:

  contradicts — the evidence shows something incompatible with the clause: a different number, name,
                type, line or behaviour.
  overreaches — the clause ranges wider than what was captured; it says "every", "never", "only",
                "no", "always", "any" or "cannot" about a population the evidence samples rather
                than covers.

Answer CORRECT only if the clause really does disagree with the captured evidence, in the way and of
the kind claimed, such that the author would have wanted to know. Answer WRONG if it does not. That
includes all of these, each of which has been observed:

  * the evidence supports the clause
  * the reason misreads the evidence
  * the reason objects to a clause other than the one quoted, or restates the clause
  * the clause's scope is fixed by its own sentence, and the objection re-reads it more broadly
  * the captured text — often a doc comment or a declaration — states the general property itself,
    so the author is reporting the capture rather than generalising past it
  * the capture disclosed an exclusion or a truncation, and the objection's whole content is that
    something was not covered; insufficiency is not disagreement
  * the objection is word-level pedantry that does not change what the text tells a reader
  * the clause describes what the design PROPOSES to build; evidence captured beforehand cannot
    contradict it
  * the text already states the limitation being reported back to it

Answer UNCLEAR only if the evidence is genuinely insufficient to settle it either way.

**Default to WRONG when you are not convinced.** This will be shown to an author as a warning, and a
wrong warning costs more than a missed one, so the burden of proof is on the assertion and not on the
author's text. Do not be generous to it.

Judge only against the evidence you were given. Do not assume facts about any codebase beyond it.

Reply with one JSON object and nothing else:
{"verdict": "CORRECT"|"WRONG"|"UNCLEAR", "why": ""}"#;

/// Drop the findings a second model can refute.
///
/// **Only `WRONG` drops.** `UNCLEAR`, an unreadable reply and a transport
/// failure all keep the finding, so no warning is ever deleted by something
/// going wrong — a refutation that did not happen is not a refutation. The
/// leg therefore cannot fail the verification either: it can only decline to
/// filter it.
///
/// Refusing to run when the refuter names the configured model is not
/// defensive tidiness. Self-refutation was measured at 17%, worse than the
/// unfiltered rate it replaces, so silently honouring that configuration
/// would make the feature actively harmful while looking configured.
///
/// The refuter's row is enforced here, on the leg itself, and not only by
/// the callers: the refuter routes on its own value, so this is the one
/// place that sees every path to a refutation. A typed refuter on a verb
/// whose row refuses it returns every finding untouched and calls no one.
/// A call that does not complete keeps its finding and is reported in
/// [`Telemetry::refuter_status`].
#[allow(clippy::too_many_arguments)]
fn refute_findings(
    providers: &Providers,
    refuter: &str,
    model: &str,
    subject: &Subject,
    findings: Vec<Finding>,
    started: Instant,
    budget: Duration,
    tel: &mut Telemetry,
) -> Vec<Finding> {
    let leg = refuter_leg(Some(refuter), Some(model), &subject.verb);
    if matches!(leg, RefuterLeg::Off | RefuterLeg::NotRun(_)) {
        return findings;
    }
    if let RefuterLeg::Itself(_) = leg {
        // Names neither setting as the culprit, because the refuter may
        // not have been set at all: it defaults to
        // `config::DEFAULT_REFUTER`, so an author who moved `verify.model`
        // onto that model reaches this with nothing of their own to
        // correct. The remedy is what the message has to carry.
        tel.explain(format!(
            "the refuter and verify.model are both {refuter}; a model refuting itself scored \
             17% and the leg is skipped — set verify.refuter_model to a different model, or to \
             `{off}` to go unrefuted deliberately",
            off = crate::config::REFUTER_OFF
        ));
        return findings;
    }
    let evidence = evidence_text(subject);
    let mut kept = Vec::new();
    for f in findings {
        let quotation = f.evidence.clone().or_else(|| f.rejected_span.clone());
        let user = format!(
            "AUTHOR'S TEXT:\n{}\n\n{}\n\nPROPOSED DISAGREEMENT:\n  kind: {}\n  clause: {}\n  \
             reason: {}\n  quotation offered: {}",
            subject.text,
            evidence,
            f.kind,
            f.clause,
            f.why,
            quotation.as_deref().unwrap_or("(none)")
        );
        let verdict = match (leg, &providers.typed) {
            (RefuterLeg::Typed(_), Some(typed)) => {
                ask_typed(typed, refuter, &user, &typed_refute_questions(), started, budget, tel)
                    .and_then(|a| {
                        a["verdict"]["choice"].as_str().map(str::to_ascii_uppercase).ok_or_else(|| {
                            tel.explain("typed refuter answered no verdict");
                            Status::Unparsable
                        })
                    })
            }
            // `spawn` builds no typed endpoint without a credential and
            // starts nothing in that case; reaching here without one is a
            // wiring fault, and it keeps the finding like any other.
            (RefuterLeg::Typed(_), None) => Err(Status::Unauthorized),
            _ => call(&providers.llm, refuter, REFUTE_SYSTEM, &user, started, budget, tel).and_then(|b| {
                json_object(&b)
                    .map(|v| str_field(&v, "verdict").to_ascii_uppercase())
                    .ok_or_else(|| {
                        tel.explain("refuter reply was not a JSON object");
                        Status::Unparsable
                    })
            }),
        };
        match verdict.as_deref() {
            Ok("WRONG") => {
                tel.refuted += 1;
                continue;
            }
            Ok("CORRECT" | "UNCLEAR") => {}
            // An answer outside the three is no more a refutation than a
            // 429 is: the finding stays, and the run says it was not put
            // to the refuter.
            Ok(_) => {
                tel.refuter_status.get_or_insert(Status::Unparsable);
            }
            Err(s) => {
                tel.refuter_status.get_or_insert(*s);
            }
        }
        kept.push(f);
    }
    kept
}

/// The refutation as Jev was measured answering it on `fact`: REFUTE_SYSTEM's
/// three verdicts as one `choice`, with that prompt's criteria, beside a
/// neutral `noul` the measurement also asked. Ported from
/// `scripts/verifier-eval/refute_jev.py` byte for byte, questions and state
/// alike — the state is the LLM refuter's user prompt unchanged — because
/// the 80% belongs to this presentation. Only the `choice` decides; the
/// `noul` is asked because the measured request asked it.
fn typed_refute_questions() -> String {
    // `verdict` first and CORRECT, WRONG, UNCLEAR in that order, as the
    // measured request sent them — see [`json_ordered`].
    let verdict = json_ordered(&[
        ("type", json_str("choice")),
        ("instructions", json_str(TYPED_REFUTE_INSTRUCTIONS)),
        (
            "criteria",
            json_ordered(&[
                ("CORRECT", json_str(TYPED_REFUTE_CORRECT)),
                ("WRONG", json_str(TYPED_REFUTE_WRONG)),
                ("UNCLEAR", json_str("The evidence is genuinely insufficient to settle it either way.")),
            ]),
        ),
    ]);
    let correct = json_ordered(&[
        ("type", json_str("noul")),
        (
            "instructions",
            json_str(
                "The proposed disagreement is correct: the quoted clause really does disagree \
with the captured evidence, in the way and of the kind claimed.",
            ),
        ),
        (
            "criteria",
            json_ordered(&[
                ("true", json_str("the clause disagrees with the evidence as claimed")),
                ("false", json_str("it does not")),
            ]),
        ),
    ]);
    json_ordered(&[("verdict", verdict), ("correct", correct)])
}

const TYPED_REFUTE_INSTRUCTIONS: &str = "An author wrote a note. A tool captured evidence. \
Someone then asserted that a specific clause of the note disagrees with that evidence. Decide \
whether that assertion is correct.";

const TYPED_REFUTE_WRONG: &str = "The assertion does not hold. Includes: the evidence supports \
the clause; the reason misreads the evidence; it objects to a clause other than the one quoted, \
or restates it; the clause's scope is fixed by its own sentence and the objection re-reads it \
more broadly; the captured text states the general property itself, so the author is reporting \
the capture; the capture disclosed an exclusion or truncation and the objection's whole content \
is that something was not covered, which is insufficiency, not disagreement; word-level pedantry \
that does not change what the text tells a reader; the clause describes what the design PROPOSES \
to build, which evidence captured beforehand cannot contradict; the text already states the \
limitation being reported back to it.";

const TYPED_REFUTE_CORRECT: &str = "The clause really does disagree with the captured evidence, \
in the way and of the kind claimed, such that the author would have wanted to know. Default to \
WRONG when not convinced: this is shown to an author as a warning, and a wrong warning costs \
more than a missed one, so the burden of proof is on the assertion and not on the author's \
text.";

/// The classify prompt for a `prose` paragraph, in place of
/// [`CLASSIFY_SYSTEM`].
///
/// The three mis-labelled shapes it names are taken from the findings that
/// failed, not invented: a paragraph describing the behaviour the design is
/// adding in the present tense, a paragraph reasoning about a record the
/// design defines, and a requirement stated flatly. Together with the
/// `PARAGRAPH:` header this took the proposal cluster from 11 findings to
/// none, and prose precision from 14% to 21% — 36% once `overreaches` is
/// bounded, and 80% with a refuter behind it.
const PROSE_CLASSIFY_SYSTEM: &str = r#"You are given one PARAGRAPH from a software design memo. Split it into its separate assertions and
label each.

  current   — asserts how the code, files or tools behave TODAY. Checkable against captured evidence.
  proposed  — asserts what THIS DESIGN will build, add, change, recommend or require. The evidence
              was captured BEFORE that exists, so it cannot speak to this.
  argument  — a reason, a decision, an entailment, or a statement about what is right or necessary.
              Nothing captured can settle it.

A design paragraph is mostly NOT about how the code behaves today. It is mostly proposal and
argument. `current` is usually the smallest of the three labels here and is often empty. Label
`current` only when the assertion is about the code as it stands, with the design not yet built.

Three shapes are `proposed` and get labelled `current` by mistake:

1. THE PARAGRAPH DESCRIBES THE BEHAVIOUR THE DESIGN IS ADDING, in the present tense. "A line longer
   than the budget is never returned whole", "the return is lines 12, 13 and 14", "it reports how
   many of the selected lines were shown" — a budget the design is introducing, described as though
   installed. The present tense is the author writing about the thing they are building. Ask whether
   the mechanism exists yet; if the paragraph is what introduces it, the assertion is `proposed`.

2. THE PARAGRAPH REASONS ABOUT A RECORD OR EVENT THE DESIGN IS ADDING. "Matching is existential over
   keys", "a later ack with a different key simply does not match" — about an event type this design
   defines. That the current code rejects it is the paragraph's premise, not its error.

3. THE PARAGRAPH STATES WHAT AN IMPLEMENTER MUST DO. A requirement, a rule the design imposes, a
   shape something "must" take. That is `proposed` however flatly it is worded.

One sentence often carries more than one assertion, with different labels, and a single sentence
frequently mixes a `current` observation with the `proposed` change it motivates. Split them.

Quote each assertion VERBATIM from the paragraph — character for character, never paraphrased,
never merged, never invented. You are only sorting the author's own words.

Reply with one JSON object and nothing else:
{"assertions": [{"text": "", "label": "current"|"proposed"|"argument"}]}"#;

/// Which classify prompt this verb is split with.
fn classify_system_for(verb: &str) -> &'static str {
    match verb {
        "prose" => PROSE_CLASSIFY_SYSTEM,
        _ => CLASSIFY_SYSTEM,
    }
}

/// Which check prompt this verb is graded with.
///
/// Only the check call varies. [`CLASSIFY_SYSTEM`] and the `CLAIM:` header
/// in [`check_prompt`] are the same for every verb, because that is the
/// shape both the retrodiction and the fact runs measured — `--prompt-file`
/// in the harness swaps this one prompt and nothing else. The header is
/// plainly wrong on a note and changing it is an untested improvement, so
/// it is left alone and written down here instead.
fn check_system_for(verb: &str) -> &'static str {
    match verb {
        "fact" => FACT_SYSTEM,
        // `prose` stays on `CHECK_SYSTEM`. A candidate exists and cuts its
        // flag rate from 35% to 23%, but no adjudication has scored what
        // survives, and the shipped prompt's own prose precision is the
        // worst number in this module (0 of 12). Shipping an unmeasured
        // prompt over a measured-bad one is still shipping an unmeasured
        // prompt.
        _ => CHECK_SYSTEM,
    }
}

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
/// comparison by selection. `revision` is the claim's `revisions` count,
/// which the caller has already replayed and this does not read.
pub fn claim_subject(
    dir: &Path,
    id: &str,
    prop: &str,
    cited: &[String],
    overlap: &[(String, Vec<String>)],
    revision: usize,
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
        revision: revision as u64,
    })
}

/// The captured side for a fact is its own captured output — the one verb
/// where the two sides were never separable.
pub fn fact_subject(dir: &Path, id: &str) -> io::Result<Subject> {
    let all = facts::load_all(dir)?;
    let (note, revision) = all
        .iter()
        .find(|f| f.id == id)
        .map(|f| (f.note.clone(), f.revisions))
        .unwrap_or_default();
    Ok(Subject {
        mint: id.to_string(),
        verb: "fact".to_string(),
        text: note,
        evidence: collect(&all, std::slice::from_ref(&id.to_string())),
        revision: revision as u64,
    })
}

/// The captured side for a prose block: the facts under the claims it
/// cites. The least-evidenced of the three comparisons — neither case
/// file in the eval contains one — which is why the verb is off unless
/// asked for. `revision` is the block's `revisions` count, as for
/// [`claim_subject`].
pub fn prose_subject(dir: &Path, id: &str, text: &str, cites: &[String], revision: usize) -> io::Result<Subject> {
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
        revision: revision as u64,
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
    let retried = retried(&records);

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
    // The reply text those details are about, kept out of them so that
    // identical causes collapse above, and shown only when asked for, as
    // the withheld spans are.
    if show_spans {
        let replies: Vec<&Record> = records.iter().filter(|r| r.reply.is_some()).collect();
        if !replies.is_empty() {
            out.push_str("\n  what those replies said:\n");
            for r in replies {
                out.push_str(&format!("    - {} {}: {}\n", r.verb, r.mint, r.reply.as_deref().unwrap_or_default().replace('\n', " ")));
            }
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
    if not_verbatim > 0 {
        out.push_str(&format!(
            "  dropped, not the author's words   {not_verbatim}   <- returned as a quotation, absent from the text\n"
        ));
    }
    let refuted_away: u32 = records.iter().map(|r| r.refuted).sum();
    if refuted_away > 0 {
        out.push_str(&format!(
            "  dropped, a second model refuted it {refuted_away}   <- asked whether the finding was right, not whether the text was\n"
        ));
    }
    let off_verb: u32 = records.iter().map(|r| r.kind_off_verb).sum();
    if off_verb > 0 {
        out.push_str(&format!(
            "  dropped, kind off this verb      {off_verb}   <- `overreaches` on a fact: insufficiency, not disagreement\n"
        ));
    }
    // Over the LLM leg's records alone. Jev's filter counts are over every
    // figure and path code proposed, not over literals a model claimed
    // were unevidenced, so summing the two would make the rates below
    // measure nothing either leg does.
    let llm_leg: Vec<&Record> = records.iter().filter(|r| r.typed_literal_calls.is_none()).collect();
    let is_unevidenced = |f: &&Finding| f.kind == KIND_UNEVIDENCED;
    let unevidenced = llm_leg.iter().flat_map(|r| r.findings.iter()).filter(is_unevidenced).count();
    let refuted: u32 = llm_leg.iter().map(|r| r.literals_refuted).sum();
    let not_quantity: u32 = llm_leg.iter().map(|r| r.not_a_quantity).sum();
    let raised = unevidenced + refuted as usize + not_quantity as usize;
    let jev_leg = records.len() - llm_leg.len();
    if jev_leg > 0 {
        let kept = records
            .iter()
            .filter(|r| r.typed_literal_calls.is_some())
            .flat_map(|r| r.findings.iter())
            .filter(is_unevidenced)
            .count();
        out.push_str(&format!(
            "\nLITERALS, judged by Jev\n  unevidenced      {kept}   over {jev_leg} verification(s) <- code proposed, Jev judged; not in the LITERALS rates\n"
        ));
    }
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

/// How many of `records` retried a call.
///
/// Only where the call count is knowable. The refutation leg makes one
/// call per finding, so `attempts` above the approach's own count means
/// "it found things", not "it retried" — and counting those as retries
/// would report a healthy run as a failing one. The gate's calls are
/// counted from the record, because how many there were depends on the
/// subject's length.
fn retried(records: &[Record]) -> usize {
    records
        .iter()
        .filter(|r| {
            r.refuter.is_none()
                && r.attempts
                    > expected_calls(
                        &r.approach,
                        r.literals,
                        RefuterLeg::Off,
                        TypedCalls {
                            gate: r.gate_calls,
                            classify: r.typed_classify_calls,
                            literals: r.typed_literal_calls,
                        },
                    )
                    .total()
        })
        .count()
}

/// How many calls an approach makes when nothing is retried, so a retry
/// can be counted rather than inferred.
///
/// The refutation leg is charged a flat two calls rather than its real
/// count, which is one per finding and unknown before the check replies.
/// Two is what the corpus says: a flagged subject carries a median of one
/// finding and 90% carry two or fewer. A subject that beats it runs the
/// remaining refutations against a budget that may expire, and a refutation
/// that times out **keeps** its finding, so the failure mode is an
/// unfiltered warning rather than a lost one. `verify.timeout_ms` overrides
/// this for anyone whose subjects flag harder.
///
/// Counted per provider, because the two are budgeted at different rates
/// and a single count cannot say which calls it is made of. A reader that
/// only needs "how many calls" — the report's retry count — takes
/// [`Calls::total`]: attempts are counted once per call whoever answers
/// it, so comparing them against the LLM count alone would report every
/// typed call as a retry.
///
/// `typed` is how many calls each typed leg makes: one per
/// [`MAX_TYPED_QUESTIONS`] questions, which the budget cannot know before
/// the subject exists and takes as one, and a record knows exactly. A typed
/// classify or literal leg takes its LLM counterpart's place.
fn expected_calls(approach: &str, literals: bool, refuter: RefuterLeg<'_>, typed: TypedCalls) -> Calls {
    let split = approach == "split";
    let mut calls = Calls { llm: 1, typed: typed.gate };
    for (runs, stand_in) in [(split, typed.classify), (literals, typed.literals)] {
        match (runs, stand_in) {
            (true, Some(n)) => calls.typed += n,
            (true, None) => calls.llm += 1,
            (false, _) => {}
        }
    }
    match refuter {
        RefuterLeg::Llm(_) => calls.llm += 2,
        RefuterLeg::Typed(_) => calls.typed += 2,
        RefuterLeg::Off | RefuterLeg::NotRun(_) | RefuterLeg::Itself(_) => {}
    }
    calls
}

/// The typed legs' calls, as [`expected_calls`] counts them: the gate's,
/// and for classify and the literal leg, `None` when the leg went to the
/// LLM and the number of TypeSafe calls when it did not.
#[derive(Clone, Copy, Default)]
struct TypedCalls {
    gate: u32,
    classify: Option<u32>,
    literals: Option<u32>,
}

/// [`expected_calls`]' answer: how many calls go to each provider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Calls {
    llm: u32,
    typed: u32,
}

impl Calls {
    fn total(self) -> u32 {
        self.llm + self.typed
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
            literals: false,
            refuter: None,
            typed_model: None,
            typed_default_without_key: false,
            model_refusal: None,
            typed_model_refusal: None,
        }
    }

    /// A URL nothing listens on. A call that reaches it fails as transport.
    const DEAD: &str = "http://127.0.0.1:9/";

    fn providers_fixture(llm: &str, typed: &str) -> Providers {
        Providers {
            llm: Endpoint { url: llm.into(), key: "llm-key".into() },
            typed: Some(Endpoint { url: typed.into(), key: "typed-key".into() }),
        }
    }

    /// A local stand-in for a provider. Answers every request with what
    /// `reply` returns for its body, and keeps every body it was sent, so a
    /// test can say which calls were made and in what order.
    struct Mock {
        url: String,
        bodies: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        /// The same bodies as sent. Parsed, a body loses its key order,
        /// and a typed question's key order is its presentation.
        raw: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl Mock {
        fn start(reply: impl Fn(&serde_json::Value) -> (u16, serde_json::Value) + Send + 'static) -> Mock {
            use std::io::{BufRead, BufReader, Read, Write};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            let url = format!("http://{}/", listener.local_addr().expect("addr"));
            let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let raw = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let seen = bodies.clone();
            let seen_raw = raw.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                    let mut len = 0usize;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        if let Some((k, v)) = line.split_once(':') {
                            if k.eq_ignore_ascii_case("content-length") {
                                len = v.trim().parse().unwrap_or(0);
                            }
                        }
                    }
                    let mut body = vec![0u8; len];
                    if reader.read_exact(&mut body).is_err() {
                        continue;
                    }
                    seen_raw.lock().expect("lock").push(String::from_utf8_lossy(&body).into_owned());
                    let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
                    let (code, answer) = reply(&body);
                    seen.lock().expect("lock").push(body);
                    let text = answer.to_string();
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                         Connection: close\r\n\r\n{text}",
                        text.len()
                    );
                }
            });
            Mock { url, bodies, raw }
        }

        fn bodies(&self) -> Vec<serde_json::Value> {
            self.bodies.lock().expect("lock").clone()
        }

        fn raw(&self) -> Vec<String> {
            self.raw.lock().expect("lock").clone()
        }
    }

    /// A server that answers 200 with a 100-byte body at once, sends two
    /// bytes of it, then either holds the connection for `hold` or closes.
    fn headers_then(hold: Option<Duration>) -> Endpoint {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/", listener.local_addr().expect("addr"));
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n\n\n");
            let _ = stream.flush();
            if let Some(hold) = hold {
                std::thread::sleep(hold);
            }
        });
        Endpoint { url, key: "k".into() }
    }

    #[test]
    fn a_body_still_arriving_when_the_budget_ends_is_a_timeout() {
        // TET-84: 19 of 61 mints came back `unavailable`, "reply body could
        // not be read", each at the full budget. Revert: map every body
        // read error to `Unavailable` — the stalled case is then red.
        let budget = Duration::from_millis(300);
        let mut tel = Telemetry::default();
        let started = Instant::now();
        let got = post(&headers_then(Some(Duration::from_secs(3))), "{}", budget, &mut tel);
        assert_eq!(got.err(), Some(Status::Timeout), "{:?}", tel.detail);
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        assert_eq!(
            tel.detail.as_deref(),
            Some("provider did not finish its reply within the remaining budget")
        );

        // The contrast: a body cut short inside the budget is not a
        // timeout. Revert: map every body read error to `Timeout`.
        let mut tel = Telemetry::default();
        let got = post(&headers_then(None), "{}", Duration::from_secs(5), &mut tel);
        assert_eq!(got.err(), Some(Status::Unavailable), "{:?}", tel.detail);
        let detail = tel.detail.unwrap_or_default();
        assert!(detail.starts_with("reply body could not be read: "), "{detail}");
    }

    /// An OpenRouter reply whose content is `content`.
    fn llm_reply(content: &str) -> serde_json::Value {
        json!({
            "choices": [{"message": {"content": content}, "finish_reason": "stop"}],
            "usage": {"cost": 0.001},
        })
    }

    /// A TypeSafe reply shaped like the one recorded at
    /// `verifier-eval/jev_reply_2026-09-21.json` in the tetel-eval-data
    /// repository, answering the refuter's two questions.
    fn typed_reply(version: &str, verdict: &str) -> serde_json::Value {
        json!({
            "model": version,
            "answers": {
                "verdict": {"type": "choice", "choice": verdict, "confidence": 1.0,
                            "probabilities": {"CORRECT": 0.0, "WRONG": 0.0, "UNCLEAR": 0.0}},
                "correct": {"type": "noul", "noul": 0.2},
            },
            "usage": {"input_tokens": 370, "output_tokens": 54},
        })
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
            revision: 0,
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
            Status::Gated,
            Status::Unavailable,
            Status::Timeout,
            Status::Unparsable,
        ];
        assert_eq!(all.len(), 9, "a status was added without being listed here");
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
        let b = block(&settings_fixture(), "claim", None, Trigger::Queued("C7"), None);
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
            let b = block(&settings_fixture(), "claim", delivered, Trigger::NotAttempted, None);
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
            kind_off_verb: 0,
            refuted: 0,
            refuter: None,
            refuter_status: None,
            typed_versions: Vec::new(),
            refuter_not_run: None,
            findings: Vec::new(),
            at: 0,
            cost: 0.0,
            elapsed_ms: 0,
            attempts: 1,
            gate_calls: 0,
            gate_status: None,
            typed_classify_calls: None,
            typed_literal_calls: None,
            detail: None,
            reply: None,
            revision: None,
        }
    }

    #[test]
    fn a_delivered_block_names_the_mint_it_is_about() {
        // It is no longer the id sitting beside the findings.
        let b = block(&settings_fixture(), "claim", Some(&record_fixture()), Trigger::Queued("C4"), None);
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
        assert!(peek_delivered(&dir).delivered.is_some(), "deliveries stopped");
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
            let b = block(&settings_fixture(), "claim", Some(&r), Trigger::NotAttempted, None);
            assert_eq!(b["status"], status);
            assert!(b.get("findings").is_none(), "{status} carried findings: {b}");
            // The mint is still named — the reader has to know which
            // comparison failed.
            assert_eq!(b["for_mint"], "C3", "{b}");
        }
        // And `ok` still carries them, including when empty.
        let b = block(&settings_fixture(), "claim", Some(&record_fixture()), Trigger::NotAttempted, None);
        assert_eq!(b["status"], "ok");
        assert_eq!(b["findings"], serde_json::json!([]));
    }

    #[test]
    fn a_call_with_nothing_to_compare_is_skipped_not_off() {
        // With the verb on, a heading or an uncited block must not answer
        // "off" — that tells an author who has just enabled the feature
        // that it is disabled.
        let b = block(&settings_fixture(), "claim", None, Trigger::NothingToCompare, None);
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
        let b = block(&s, "claim", None, Trigger::NotAttempted, None);
        assert_eq!(b["status"], "unauthorized", "{b}");
        assert!(b.get("queued_for").is_none(), "{b}");
    }

    #[test]
    fn a_verb_outside_the_verb_list_is_off_not_unauthorized() {
        // `verbs` is `claim` alone in the fixture, so `prose` is off even
        // with the feature enabled — the switch, the verb list and a
        // missing credential are three different answers to "why am I
        // getting nothing".
        let b = block(&settings_fixture(), "prose", None, Trigger::NotAttempted, None);
        assert_eq!(b["status"], "off", "{b}");
    }

    #[test]
    fn an_unreadable_reply_is_not_a_clean_bill() {
        let subject = Subject {
            mint: "C1".into(),
            revision: 0,
            verb: "claim".into(),
            text: "x".into(),
            evidence: vec![("F1".into(), vec![], vec!["captured".to_string()])],
        };
        assert!(parse_findings("no braces here", &subject, &mut Telemetry::default()).is_none());
        assert!(parse_findings("{not json}", &subject, &mut Telemetry::default()).is_none());
        assert!(parse_findings(r#"{"other": []}"#, &subject, &mut Telemetry::default()).is_none());
        // A verdict outside the vocabulary fails the whole reply rather
        // than being dropped to an empty list.
        assert!(parse_findings(
            r#"{"disagreements":[{"kind":"unsure","clause":"c","evidence":"e","why":"w"}]}"#,
            &subject,
            &mut Telemetry::default(),
        )
        .is_none());
        // The common, correct answer.
        let empty = parse_findings(r#"{"disagreements": []}"#, &subject, &mut Telemetry::default()).expect("parsed");
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
            revision: 0,
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
            revision: 0,
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
            &mut Telemetry::default(),
        )
        .expect("parsed");
        assert_eq!(f[0].facts, vec!["F2".to_string()]);
        assert!(f[0].quoted);
    }

    #[test]
    fn a_literal_the_capture_never_carried_survives_a_label_that_mentions_it() {
        // The label block is full of the tokens `is_checkable` admits —
        // line ranges, grep patterns, exit codes. Sharing `containing` with
        // the quotation check machine-refuted a note's own number against a
        // line range that merely contained the digits, and counted the
        // suppression as an accuracy signal for the kind it suppressed.
        let subject = Subject {
            mint: "C1".into(),
            revision: 0,
            verb: "claim".into(),
            text: "the buffer is 4096 bytes".into(),
            evidence: vec![(
                "F1".into(),
                vec!["src/buf.rs lines 4000-4096".into()],
                vec!["fn fill(buf: &mut [u8]) {}".into()],
            )],
        };
        assert!(!subject.in_captured_output("4096"), "a label is not the capture");
        assert!(subject.in_captured_output("fn fill"), "captured output still counts");

        // And the quotation check is unchanged: the same span is still a
        // quotation, because the model was shown the label.
        assert_eq!(subject.containing("4096").first().map(|(_, w)| *w), Some(QuotedFrom::Extent));
    }

    #[test]
    fn a_span_in_one_facts_output_is_not_attributed_to_anothers_label() {
        // `found.first()` decides `quoted_from`, so evidence order used to
        // decide it: F1's label matched, F2's captured output matched, and
        // the reader was sent to the label block for a span that really is
        // in the capture.
        let subject = Subject {
            mint: "C1".into(),
            revision: 0,
            verb: "claim".into(),
            text: "x".into(),
            evidence: vec![
                ("F1".into(), vec!["src/a.rs lines 1-40".into()], vec!["nothing here".into()]),
                ("F2".into(), vec![], vec!["lines 1-40 of the table".into()]),
            ],
        };
        let found = subject.containing("lines 1-40");
        assert_eq!(found.len(), 2, "both facts still match");
        assert_eq!(found[0], ("F2".to_string(), QuotedFrom::Output), "output is named first");
    }

    #[test]
    fn an_overreach_is_dropped_on_a_fact_and_kept_on_a_claim() {
        // The same reply, twice, differing only in the verb. Measured over
        // 123 corpus facts every `overreaches` a fact drew was an
        // insufficiency objection — "the search excluded paths" — while the
        // kind carries 83% precision on a claim. So the drop is per verb
        // and the rest of the reply survives it.
        let reply = r#"{"disagreements":[
            {"kind":"overreaches","clause":"every","evidence":"alpha","why":"the search excluded paths"},
            {"kind":"contradicts","clause":"c","evidence":"alpha","why":"w"}]}"#;

        let mut tel = Telemetry::default();
        let mut fact = subject_fixture("every x", &[("F1", &["alpha"])]);
        fact.verb = "fact".into();
        let f = parse_findings(reply, &fact, &mut tel).expect("parsed");
        assert_eq!(f.len(), 1, "the overreach should be gone");
        assert_eq!(f[0].kind, KIND_CONTRADICTS, "and the other finding should not be");
        assert_eq!(tel.kind_off_verb, 1, "the drop must be counted, not silent");

        // But not counted for a reply that is then thrown away: the second
        // row takes this to `Unparsable`, and a drop attributed to a
        // verification that delivered nothing cannot be accounted for.
        let mut tel = Telemetry::default();
        let discarded = r#"{"disagreements":[
            {"kind":"overreaches","clause":"every","evidence":"alpha","why":"w"},
            {"kind":"unsure","clause":"c","evidence":"alpha","why":"w"}]}"#;
        assert!(parse_findings(discarded, &fact, &mut tel).is_none());
        assert_eq!(tel.kind_off_verb, 0, "a discarded reply must leave no drops behind");

        let mut tel = Telemetry::default();
        let claim = subject_fixture("every x", &[("F1", &["alpha"])]);
        assert_eq!(claim.verb, "claim");
        let f = parse_findings(reply, &claim, &mut tel).expect("parsed");
        assert_eq!(f.len(), 2, "a claim still reports both kinds");
        assert_eq!(tel.kind_off_verb, 0);
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
        // Graded on a claim: the measurement was taken over fact notes, but
        // `overreaches` is no longer reported on that verb
        // ([`kind_reported_for`]), and the attribution under test is the
        // same predicate for every verb that does report it.
        let subject = Subject {
            mint: "C1".into(),
            revision: 0,
            verb: "claim".into(),
            text: "the search covered every file".into(),
            evidence: vec![(
                "F1".into(),
                vec!["search: /repo (grep (ERE): look_grep) — 10 files matched".into()],
                vec!["fn look_grep() {}".into()],
            )],
        };
        let f = parse_findings(
            r#"{"disagreements":[{"kind":"overreaches","clause":"every file","evidence":"10 files matched","why":"w"}]}"#,
            &subject,
            &mut Telemetry::default(),
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
            &mut Telemetry::default(),
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
            &mut Telemetry::default(),
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
            &mut Telemetry::default(),
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
            &mut Telemetry::default(),
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
        let b = block(&settings_fixture(), "claim", Some(&r), Trigger::NotAttempted, None);

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
        let b = block(&settings_fixture(), "claim", Some(&ok), Trigger::NotAttempted, None);
        assert!(b.get("literals_incomplete").is_none(), "{b}");
    }

    #[test]
    fn the_default_budget_grows_with_the_number_of_calls_it_has_to_cover() {
        // A flat budget silently means three different things: one
        // number would cover a one-call run and leave a four-call run
        // short.
        let per_leg = DEFAULT_MS_PER_LEG;
        let off = RefuterLeg::Off;
        assert_eq!(default_budget_ms(expected_calls("direct", false, off, TypedCalls::default())), per_leg);
        assert_eq!(default_budget_ms(expected_calls("split", false, off, TypedCalls::default())), per_leg * 2);
        assert_eq!(default_budget_ms(expected_calls("split", true, off, TypedCalls::default())), per_leg * 3);
        // The shipped defaults: `split`, and the default refuter's two legs.
        let llm = RefuterLeg::Llm(crate::config::DEFAULT_REFUTER);
        assert_eq!(default_budget_ms(expected_calls("split", false, llm, TypedCalls::default())), 400_000);
        // And with TypeSafe's key, the default typed model's legs: the
        // figures docs/verify.md quotes for the shipped configuration.
        let m = Some("openai/gpt-6-luna");
        let typed = Some(crate::config::DEFAULT_TYPED_MODEL);
        let shipped = |verb| {
            default_budget_ms(planned_calls(verb, "split", false, Some(crate::config::DEFAULT_REFUTER), m, typed))
        };
        assert_eq!((shipped("claim"), shipped("fact")), (320_000, 410_000));
    }

    #[test]
    fn a_typed_refuter_is_budgeted_at_its_own_rate_and_only_where_it_runs() {
        let typed = "typesafe/jev-latest";
        let fact = expected_calls("split", false, refuter_leg(Some(typed), None, "fact"), TypedCalls::default());
        assert_eq!(fact, Calls { llm: 2, typed: 2 });
        // A number, not the constants: a typed call charged at the LLM rate
        // would reproduce itself on both sides of an assertion written in them.
        assert_eq!(default_budget_ms(fact), 220_000);
        // Not run on `claim`, so not paid for there either.
        let claim = expected_calls("split", false, refuter_leg(Some(typed), None, "claim"), TypedCalls::default());
        assert_eq!(claim, Calls { llm: 2, typed: 0 });
    }

    #[test]
    fn the_literal_check_adds_a_call_that_a_retry_count_must_not_mistake() {
        let off = RefuterLeg::Off;
        assert_eq!(expected_calls("split", false, off, TypedCalls::default()).total(), 2);
        assert_eq!(expected_calls("split", true, off, TypedCalls::default()).total(), 3);
        assert_eq!(expected_calls("direct", false, off, TypedCalls::default()).total(), 1);
        assert_eq!(expected_calls("direct", true, off, TypedCalls::default()).total(), 2);
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
        assert!(!FACT_SYSTEM.contains(KIND_UNEVIDENCED));
        assert!(LITERALS_SYSTEM.contains("unevidenced"));
    }

    #[test]
    fn each_verb_is_checked_with_the_prompt_its_numbers_were_measured_on() {
        // A note is not a claim, and the prompt that grades it says so in
        // its first line. `claim` keeps the prompt the 125-claim
        // retrodiction ran on, or its 83%/30% stops describing anything.
        assert!(CHECK_SYSTEM.starts_with("You are given a claim"));
        assert!(FACT_SYSTEM.starts_with("You are given a NOTE"));

        assert_eq!(check_system_for("fact"), FACT_SYSTEM);
        assert_eq!(check_system_for("claim"), CHECK_SYSTEM);
        // Measured at 35% -> 23% on flag rate, never adjudicated. Until it
        // is, prose is graded by the prompt whose failures are at least
        // known.
        assert_eq!(check_system_for("prose"), CHECK_SYSTEM);
    }

    #[test]
    fn a_refuter_naming_the_same_model_is_declined_and_says_so() {
        // Self-refutation measured 17% — worse than the unfiltered rate it
        // would replace. Honouring the configuration silently would make
        // the feature harmful while looking configured, so it is refused
        // and the reason is written where a reader will meet it.
        let subject = subject_fixture("x", &[("F1", &["alpha"])]);
        let f = vec![Finding {
            kind: KIND_CONTRADICTS.into(),
            clause: "c".into(),
            clause_quoted: false,
            facts: vec![],
            quoted_from: None,
            legacy_fact: None,
            evidence: None,
            literal: None,
            why: "w".into(),
            quoted: false,
            rejected_span: None,
        }];
        let mut tel = Telemetry::default();
        let kept = refute_findings(
            &providers_fixture(DEAD, DEAD),
            "openai/gpt-5.6-luna",
            "openai/gpt-5.6-luna",
            &subject,
            f,
            Instant::now(),
            Duration::from_millis(1),
            &mut tel,
        );
        assert_eq!(kept.len(), 1, "no call is made and nothing is dropped");
        assert_eq!(tel.refuted, 0);
        let detail = tel.detail.as_deref().unwrap_or("");
        assert!(detail.contains("both"), "the skip must be legible: {detail:?}");
        // The refuter now has a default, so a reader can meet this without
        // having configured anything. Naming the state is not enough; the
        // message has to name what to do about it.
        assert!(
            detail.contains(crate::config::KEY_VERIFY_REFUTER)
                && detail.contains(crate::config::REFUTER_OFF),
            "the skip must name the remedy: {detail:?}"
        );
    }

    #[test]
    fn a_refutation_that_could_not_run_keeps_the_finding() {
        // An expired budget, an unreadable reply and a transport failure
        // all reach the same place. A refutation that did not happen is not
        // a refutation, so the warning survives and the leg can only
        // decline to filter — never fail the verification.
        let subject = subject_fixture("x", &[("F1", &["alpha"])]);
        let f = vec![Finding {
            kind: KIND_CONTRADICTS.into(),
            clause: "c".into(),
            clause_quoted: false,
            facts: vec![],
            quoted_from: None,
            legacy_fact: None,
            evidence: None,
            literal: None,
            why: "w".into(),
            quoted: false,
            rejected_span: None,
        }];
        let mut tel = Telemetry::default();
        // Zero budget: `call` returns before it can reach a provider.
        let kept = refute_findings(
            &providers_fixture(DEAD, DEAD),
            "anthropic/claude-sonnet-4.5",
            "openai/gpt-5.6-luna",
            &subject,
            f,
            Instant::now() - Duration::from_secs(60),
            Duration::from_millis(1),
            &mut tel,
        );
        assert_eq!(kept.len(), 1, "fail-open: the author still sees it");
        assert_eq!(tel.refuted, 0);
    }

    // -----------------------------------------------------------------
    // Typed legs. Each test names the invariant it carries and the revert
    // that must turn it red.
    // -----------------------------------------------------------------

    fn contradiction() -> Finding {
        Finding { clause_quoted: false, facts: vec![], quoted_from: None, evidence: None, quoted: false, ..finding_fixture() }
    }

    fn fact_subject() -> Subject {
        Subject { verb: "fact".into(), ..subject_fixture("the function returns early", &[("F1", &["return"])]) }
    }

    /// The keys a response carries for a configuration with no typesafe
    /// value, before the typed legs existed.
    fn untyped_keys(delivered_ok: bool) -> Vec<&'static str> {
        let mut keys = vec![
            "approach", "deterministic", "guidance", "literals", "model", "refuter_model",
            "status", "timeout_ms", "verbs",
        ];
        if delivered_ok {
            keys.extend(["findings", "for_mint"]);
        }
        keys.sort_unstable();
        keys
    }

    fn keys_of(b: &serde_json::Value) -> Vec<String> {
        let mut k: Vec<String> = b.as_object().expect("object").keys().cloned().collect();
        k.sort_unstable();
        k
    }

    #[test]
    fn no_typesafe_value_anywhere_means_the_same_field_set() {
        // Invariant 1. Reverts: insert the version keys whatever the record
        // says; take `refuter_model` out of the literal and insert it only
        // when set, which loses the `null` an `off` refuter prints.
        for refuter in [None, Some(crate::config::DEFAULT_REFUTER.to_string())] {
            for verb in ["fact", "claim", "prose"] {
                let s = Settings { refuter: refuter.clone(), verbs: vec![verb.into()], ..settings_fixture() };
                // Not `NotAttempted`: with the verb on, that is `unauthorized`,
                // whose `detail` depends on what the environment holds.
                let b = block(&s, verb, None, Trigger::NothingToCompare, None);
                assert_eq!(keys_of(&b), untyped_keys(false), "{verb} {refuter:?}: {b}");
                let b = block(&s, verb, Some(&record_fixture()), Trigger::NothingToCompare, None);
                assert_eq!(keys_of(&b), untyped_keys(true), "{verb} {refuter:?}: {b}");
            }
        }
        let off = Settings { refuter: None, ..settings_fixture() };
        assert!(block(&off, "claim", None, Trigger::NotAttempted, None)["refuter_model"].is_null());
    }

    #[test]
    fn no_typesafe_value_anywhere_means_the_same_calls_in_the_same_order() {
        // Invariant 1, on the calls. Revert: route the default refuter to
        // TypeSafe (e.g. `is_typed_model` true for every vendor).
        let llm = Mock::start(|body| {
            let system = body["messages"][0]["content"].as_str().unwrap_or("");
            (200, llm_reply(if system == REFUTE_SYSTEM {
                r#"{"verdict":"CORRECT","why":""}"#
            } else if system == CLASSIFY_SYSTEM {
                r#"{"assertions":[{"text":"the function returns early","label":"current"}]}"#
            } else {
                r#"{"disagreements":[{"kind":"contradicts","clause":"the function returns early","evidence":"return","why":"w"}]}"#
            }))
        });
        let typed = Mock::start(|_| (200, typed_reply(MEASURED_TYPED_VERSION, "WRONG")));
        let mut tel = Telemetry::default();
        let (status, f, _) = run(
            &providers_fixture(&llm.url, &typed.url),
            "openai/gpt-5.6-luna",
            "split",
            Legs { literals: false, refuter: Some(crate::config::DEFAULT_REFUTER), typed_model: None },
            &fact_subject(),
            Instant::now(),
            Duration::from_secs(20),
            &mut tel,
        );
        assert_eq!(status, Status::Ok, "{:?}", tel.detail);
        assert_eq!(f.len(), 1);
        let systems: Vec<String> = llm.bodies().iter().map(|b| b["messages"][0]["content"].as_str().unwrap_or("").to_string()).collect();
        assert_eq!(systems, [CLASSIFY_SYSTEM, FACT_SYSTEM, REFUTE_SYSTEM]);
        assert!(typed.bodies().is_empty(), "an untyped configuration called TypeSafe");
        assert!(tel.typed_versions.is_empty());
    }

    #[test]
    fn the_refuter_row_is_fact_alone() {
        // Invariant 8, the table. Revert: permit the typed refuter on any
        // other verb.
        for (verb, permitted) in [("fact", true), ("claim", false), ("prose", false), ("nonesuch", false)] {
            assert_eq!(typed_legs(verb).refuter, permitted, "{verb}");
        }
    }

    #[test]
    fn a_typed_refuter_off_its_row_runs_nothing_and_falls_back_to_nothing() {
        // Invariants 8 and 9, on the leg itself — the refuter routes on its
        // own value and takes no verb, so the row has to hold here and not
        // only in the callers. Reverts: drop the early return in
        // `refute_findings` (it then falls through to the LLM refuter);
        // compute the leg without the verb.
        for verb in ["claim", "prose"] {
            let llm = Mock::start(|_| (200, llm_reply(r#"{"verdict":"WRONG","why":""}"#)));
            let typed = Mock::start(|_| (200, typed_reply(MEASURED_TYPED_VERSION, "WRONG")));
            let subject = Subject { verb: verb.into(), ..fact_subject() };
            let mut tel = Telemetry::default();
            let kept = refute_findings(
                &providers_fixture(&llm.url, &typed.url),
                "typesafe/jev-latest",
                "openai/gpt-5.6-luna",
                &subject,
                vec![contradiction()],
                Instant::now(),
                Duration::from_secs(20),
                &mut tel,
            );
            assert_eq!(kept.len(), 1, "{verb}: the finding was put to a refuter");
            assert!(typed.bodies().is_empty(), "{verb}: TypeSafe was called");
            assert!(llm.bodies().is_empty(), "{verb}: fell back to an LLM refuter");
            assert_eq!(tel.refuter_status, None, "{verb}: a leg that never ran is not incomplete");
        }
    }

    #[test]
    fn a_typed_refuter_off_its_row_prints_no_model_name() {
        // Invariant 9, on the response. Revert: leave `refuter_model` in
        // the object for a `NotRun` leg.
        let s = Settings { refuter: Some("typesafe/jev-latest".into()), ..settings_fixture() };
        let b = block(&s, "claim", None, Trigger::NotAttempted, None);
        assert!(b.get("refuter_model").is_none(), "{b}");
        assert_eq!(b["refuter_not_run"]["verb"], "claim", "{b}");
        assert_eq!(b["refuter_not_run"]["refuter_model"], "typesafe/jev-latest", "{b}");
        assert!(b["refuter_not_run"]["reason"].as_str().is_some_and(|r| r.contains("`fact` only")), "{b}");
        // On its own row it is the refuter in force, printed as any other.
        let b = block(&s, "fact", None, Trigger::NotAttempted, None);
        assert_eq!(b["refuter_model"], "typesafe/jev-latest", "{b}");
        assert!(b.get("refuter_not_run").is_none(), "{b}");
    }

    #[test]
    fn a_delivered_record_names_the_refuter_that_ran_on_it_not_the_callers() {
        // A finished verification is delivered on whichever call comes
        // next, which may be another verb's. The refuter fields travel with
        // the findings, so they have to describe the record's verb. Revert:
        // build them from `settings` and the calling verb whatever was
        // delivered.
        let s = Settings { refuter: Some("typesafe/jev-latest".into()), ..settings_fixture() };
        // A `fact` record Jev refuted, delivered on a `claim` call.
        let fact = Record {
            verb: "fact".into(),
            refuter: Some("typesafe/jev-latest".into()),
            ..record_fixture()
        };
        let b = block(&s, "claim", Some(&fact), Trigger::NotAttempted, None);
        assert_eq!(b["refuter_model"], "typesafe/jev-latest", "{b}");
        assert!(b.get("refuter_not_run").is_none(), "{b}");
        // A `claim` record nothing refuted, delivered on a `fact` call.
        let claim = Record {
            refuter_not_run: RefuterLeg::NotRun("typesafe/jev-latest").not_run(),
            ..record_fixture()
        };
        let b = block(&s, "fact", Some(&claim), Trigger::NotAttempted, None);
        assert!(b.get("refuter_model").is_none(), "{b}");
        assert_eq!(b["refuter_not_run"]["verb"], "claim", "{b}");
        assert_eq!(b["refuter_not_run"]["refuter_model"], "typesafe/jev-latest", "{b}");
        assert!(b["refuter_not_run"]["reason"].as_str().is_some_and(|r| r.contains("`fact` only")), "{b}");
    }

    #[test]
    fn a_refuter_that_is_the_check_model_runs_nothing_and_is_not_printed() {
        // A model refuting itself is skipped, so a name over its findings
        // would say they were refuted, and a bare null would look like
        // `off`. Reverts: drop the `Itself` arm from `refuter_leg` (the name
        // is printed, the record names it, and the budget pays for two calls
        // that never happen); return `None` for it from `not_run`.
        let m = "openai/gpt-5.6-luna";
        assert_eq!(refuter_leg(Some(m), Some(m), "fact").runs(), None);
        let s = Settings { refuter: Some(m.into()), ..settings_fixture() };
        let b = block(&s, "claim", None, Trigger::NotAttempted, None);
        assert!(b.get("refuter_model").is_none(), "{b}");
        // Said, with the remedy, rather than looking like `off`.
        assert_eq!(b["refuter_not_run"]["refuter_model"], m, "{b}");
        assert!(b["refuter_not_run"]["reason"].as_str().is_some_and(|r| r.contains("17%")), "{b}");
        assert_eq!(
            expected_calls("split", false, refuter_leg(Some(m), Some(m), "claim"), TypedCalls::default()).total(),
            expected_calls("split", false, RefuterLeg::Off, TypedCalls::default()).total()
        );
    }

    #[test]
    fn a_typed_refuter_on_fact_asks_the_measured_questions_and_drops_only_wrong() {
        let typed = Mock::start(|b| {
            let verdict = if b["state"].as_str().unwrap_or("").contains("clause: drop me") { "WRONG" } else { "UNCLEAR" };
            (200, typed_reply(MEASURED_TYPED_VERSION, verdict))
        });
        let mut tel = Telemetry::default();
        let findings = vec![Finding { clause: "drop me".into(), ..contradiction() }, contradiction()];
        let kept = refute_findings(
            &providers_fixture(DEAD, &typed.url),
            "typesafe/jev-latest",
            "openai/gpt-5.6-luna",
            &fact_subject(),
            findings,
            Instant::now(),
            Duration::from_secs(20),
            &mut tel,
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].clause, "the function returns early");
        assert_eq!(tel.refuted, 1);
        assert_eq!(tel.attempts, 2, "one attempt per typed call");
        let sent = typed.bodies();
        // The vendor half routes; it is not the endpoint's model name.
        assert_eq!(sent[0]["model"], "jev-latest");
        let measured = include_str!("../tests/fixtures/typed_wire/refute_fact.json").trim_end();
        assert_eq!(typed_refute_questions(), measured, "not the request the harness sent");
        // And on the wire, in the harness's key order.
        assert!(typed.raw()[0].ends_with(&format!(r#","model":"jev-latest","questions":{measured}}}"#)), "{}", typed.raw()[0]);
        // The state is the LLM refuter's user prompt, unchanged.
        assert!(sent[0]["state"].as_str().unwrap_or("").starts_with("AUTHOR'S TEXT:\nthe function returns early\n\n"));
        assert!(sent[0]["state"].as_str().unwrap_or("").contains("\n\nPROPOSED DISAGREEMENT:\n  kind: contradicts\n"));
    }

    #[test]
    fn a_refuter_call_that_fails_keeps_the_finding_and_says_so() {
        for (llm, typed, refuter) in [
            (Mock::start(|_| (500, json!({}))), Mock::start(|_| (200, json!({}))), crate::config::DEFAULT_REFUTER),
            (Mock::start(|_| (200, json!({}))), Mock::start(|_| (429, json!({}))), "typesafe/jev-latest"),
            (Mock::start(|_| (200, json!({}))), Mock::start(|_| (200, json!({"model": "jev-1.13.0"}))), "typesafe/jev-latest"),
        ] {
            let mut tel = Telemetry::default();
            let kept = refute_findings(
                &providers_fixture(&llm.url, &typed.url),
                refuter,
                "openai/gpt-5.6-luna",
                &fact_subject(),
                vec![contradiction()],
                Instant::now(),
                Duration::from_secs(20),
                &mut tel,
            );
            assert_eq!(kept.len(), 1, "{refuter}");
            assert!(tel.refuter_status.is_some(), "{refuter}: a failed refutation passed for a clean one");
        }
        let r = Record {
            findings: vec![finding_fixture()],
            refuter: Some(crate::config::DEFAULT_REFUTER.into()),
            refuter_status: Some("unavailable".into()),
            ..record_fixture()
        };
        let b = block(&settings_fixture(), "claim", Some(&r), Trigger::NotAttempted, None);
        assert_eq!(b["refuter_incomplete"], "unavailable", "{b}");
        let clean = Record { refuter_status: None, ..r };
        assert!(block(&settings_fixture(), "claim", Some(&clean), Trigger::NotAttempted, None).get("refuter_incomplete").is_none());
    }

    #[test]
    fn a_typed_call_records_the_version_that_answered_and_costs_more_than_nothing() {
        // Invariants 7 (recording) and 11. Reverts: stop reading the
        // reply's `model`; stop pricing from `input_tokens`.
        let typed = Mock::start(|_| (200, typed_reply("jev-1.14.0", "CORRECT")));
        let mut tel = Telemetry::default();
        let ep = Endpoint { url: typed.url.clone(), key: "k".into() };
        ask_typed(&ep, "typesafe/jev-latest", "s", &typed_refute_questions(), Instant::now(), Duration::from_secs(20), &mut tel)
            .expect("answers");
        assert_eq!(tel.typed_versions, ["jev-1.14.0"]);
        assert!(tel.cost > 0.0, "a typed call totalled nothing");
        assert!((tel.cost - 370.0 * TYPED_USD_PER_INPUT_TOKEN).abs() < 1e-15, "{}", tel.cost);
    }

    #[test]
    fn a_different_version_is_flagged_whatever_the_status() {
        // Invariant 7. Revert: move the version keys inside the `ok` guard.
        for status in ["ok", "unavailable", "timeout"] {
            let r = Record { status: status.into(), typed_versions: vec!["jev-1.14.0".into()], ..record_fixture() };
            let b = block(&settings_fixture(), "fact", Some(&r), Trigger::NotAttempted, None);
            assert_eq!(b["typed_model_unmeasured"], true, "{status}: {b}");
            assert_eq!(b["typed_model_versions"], json!(["jev-1.14.0"]), "{status}: {b}");
        }
        let measured = Record { typed_versions: vec![MEASURED_TYPED_VERSION.into()], ..record_fixture() };
        let b = block(&settings_fixture(), "fact", Some(&measured), Trigger::NotAttempted, None);
        assert!(b.get("typed_model_unmeasured").is_none(), "{b}");
        assert_eq!(b["typed_model_versions"], json!([MEASURED_TYPED_VERSION]));
    }

    #[test]
    fn a_typesafe_credential_is_demanded_only_by_a_leg_that_runs() {
        // Invariant 6. Reverts: demand the key for any typesafe refuter
        // (`NotRun` included); demand it for none.
        let s = Settings { refuter: Some("typesafe/jev-latest".into()), ..settings_fixture() };
        let key = || Some("k".to_string());
        assert!(providers_for(&s, "fact", key(), None).is_none(), "fact ran without its key");
        assert!(providers_for(&s, "fact", key(), key()).is_some_and(|p| p.typed.is_some()));
        for verb in ["claim", "prose"] {
            let p = providers_for(&s, verb, key(), None);
            assert!(p.is_some_and(|p| p.typed.is_none()), "{verb} demanded a key it never uses");
        }
        let d = unauthorized_detail(&s, "fact", true, false).expect("a gap");
        assert!(d.contains(TYPED_KEY_VAR) && d.contains("typesafe/jev-latest"), "{d}");
        assert_eq!(unauthorized_detail(&s, "claim", true, false), None);
        // And it never displaces the OpenRouter gaps: every one is named.
        let d = unauthorized_detail(&Settings { model: None, ..s }, "fact", false, false).expect("gaps");
        assert!(d.contains("verify.model") && d.contains("OPENROUTER_API_KEY") && d.contains(TYPED_KEY_VAR), "{d}");
    }

    #[test]
    fn the_default_typed_model_runs_only_with_its_key() {
        // Reverts: default it on unconditionally (every OpenRouter-only
        // setup turns `unauthorized`); default it off (the measured saving
        // never ships); let `off` fall back to the default.
        use config::TypedModel::*;
        let default = Some(config::DEFAULT_TYPED_MODEL.to_string());
        assert_eq!(effective_typed_model(Unset, true), (default, false));
        assert_eq!(effective_typed_model(Unset, false), (None, true));
        assert_eq!(effective_typed_model(Off, true), (None, false));
        assert_eq!(effective_typed_model(Off, false), (None, false));
        let set = Set("typesafe/jev-1.13.0".into());
        assert_eq!(effective_typed_model(set.clone(), false), (Some("typesafe/jev-1.13.0".into()), false));
        // A value the author set still demands its key: the default's
        // leniency must not reach it.
        let s = Settings { typed_model: effective_typed_model(set, false).0, verbs: vec!["fact".into()], ..settings_fixture() };
        assert!(unauthorized_detail(&s, "fact", true, false).is_some_and(|d| d.contains(TYPED_KEY_VAR)));
    }

    #[test]
    fn a_default_passed_over_for_its_key_is_stated_where_it_would_run() {
        // Reverts: never state it (an author without the key cannot tell
        // the default exists); state it on every verb (prose has no typed
        // leg to miss); treat it as a gap (the verification goes
        // `unauthorized` over a credential nobody chose to need).
        let s = Settings {
            typed_default_without_key: true,
            approach: "split".into(),
            verbs: vec!["claim".into(), "fact".into(), "prose".into()],
            ..settings_fixture()
        };
        for verb in ["claim", "fact"] {
            let b = block(&s, verb, None, Trigger::NotAttempted, None);
            assert_eq!(b["typed_model_not_run"]["typed_model"], config::DEFAULT_TYPED_MODEL, "{verb}: {b}");
            let why = b["typed_model_not_run"]["reason"].as_str().unwrap_or_default();
            assert!(why.contains(TYPED_KEY_VAR) && why.contains("`off`"), "{verb}: {why}");
            assert!(b.get("typed_model").is_none(), "{verb}: {b}");
            assert_eq!(unauthorized_detail(&s, verb, true, false), None, "{verb}");
        }
        let b = block(&s, "prose", None, Trigger::NotAttempted, None);
        assert!(b.get("typed_model_not_run").is_none(), "{b}");
        let quiet = Settings { typed_default_without_key: false, ..s.clone() };
        assert!(block(&quiet, "fact", None, Trigger::NotAttempted, None).get("typed_model_not_run").is_none());
        // Verification off, or the verb not listed: nothing would have run,
        // so there is nothing to have missed.
        let off = Settings { enabled: false, ..s.clone() };
        assert!(block(&off, "fact", None, Trigger::NotAttempted, None).get("typed_model_not_run").is_none());
        let unlisted = Settings { verbs: vec!["fact".into()], ..s };
        assert!(block(&unlisted, "claim", None, Trigger::NotAttempted, None).get("typed_model_not_run").is_none());
    }

    #[test]
    fn the_default_typed_model_is_the_version_the_gate_was_fitted_on() {
        // Reverts: default to an alias such as `jev-latest`, which moves
        // the thresholds when TypeSafe moves it.
        assert_eq!(config::DEFAULT_TYPED_MODEL, format!("{}/{MEASURED_TYPED_VERSION}", config::TYPED_VENDOR));
    }

    // -----------------------------------------------------------------
    // The gate.
    // -----------------------------------------------------------------

    const JEV: &str = "typesafe/jev-latest";

    /// A TypeSafe stand-in that answers only what it is asked: `which` from
    /// `which`, every `k{i}` as CURRENT with `current`, and the refuter's
    /// questions with `verdict`.
    fn typed_mock(which: serde_json::Value, current: f64, verdict: &'static str) -> Mock {
        typed_mock_literals(which, current, verdict, Some(0.0), Some(1.0))
    }

    /// [`typed_mock`], answering the literal leg's `q{i}` and `c{i}` with
    /// these probabilities, or with none on `None`.
    fn typed_mock_literals(
        which: serde_json::Value,
        current: f64,
        verdict: &'static str,
        quantity: Option<f64>,
        carried: Option<f64>,
    ) -> Mock {
        Mock::start(move |b| {
            let mut answers = serde_json::Map::new();
            for key in b["questions"].as_object().map(|q| q.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() {
                let a = match key.as_str() {
                    "which" => json!({"type": "choice", "choice": "NONE", "confidence": 1.0, "probabilities": which}),
                    "verdict" | "correct" => typed_reply(MEASURED_TYPED_VERSION, verdict)["answers"][&key].clone(),
                    // Classify's parts, at the gate's CURRENT, and the
                    // literal leg's candidates.
                    k if k.starts_with('u') => json!({"type": "choice",
                        "choice": if current >= 0.5 { "current" } else { "proposed" }, "confidence": current,
                        "probabilities": {"current": current, "proposed": 1.0 - current, "argument": 0.0}}),
                    k if k.starts_with('q') => json!({"type": "noul", "noul": quantity}),
                    k if k.starts_with('c') => json!({"type": "noul", "noul": carried}),
                    _ => json!({"type": "choice", "choice": "CURRENT", "confidence": current,
                                "probabilities": {"CURRENT": current, "PROPOSED": 1.0 - current, "ARGUMENT": 0.0}}),
                };
                answers.insert(key, a);
            }
            (200, json!({"model": MEASURED_TYPED_VERSION, "answers": answers,
                         "usage": {"input_tokens": 370, "output_tokens": 54}}))
        })
    }

    /// An LLM stand-in that answers classify, and every check with one
    /// disagreement over the fixture's clause.
    fn llm_mock() -> Mock {
        Mock::start(|body| {
            let system = body["messages"][0]["content"].as_str().unwrap_or("");
            (200, llm_reply(if system == REFUTE_SYSTEM {
                r#"{"verdict":"CORRECT","why":""}"#
            } else if system == CLASSIFY_SYSTEM {
                r#"{"assertions":[{"text":"the function returns early","label":"current"}]}"#
            } else {
                r#"{"disagreements":[{"kind":"contradicts","clause":"the function returns early","evidence":"return","why":"w"}]}"#
            }))
        })
    }

    fn gated_run(llm: &Mock, typed: &Mock, legs: Legs<'_>, subject: &Subject, tel: &mut Telemetry) -> (Status, Vec<Finding>) {
        let (status, f, _) = run(
            &providers_fixture(&llm.url, &typed.url),
            "openai/gpt-5.6-luna",
            "split",
            legs,
            subject,
            Instant::now(),
            Duration::from_secs(20),
            tel,
        );
        (status, f)
    }

    fn gate_only(typed_model: Option<&str>) -> Legs<'_> {
        Legs { literals: true, refuter: None, typed_model }
    }

    #[test]
    fn the_gate_cuts_text_where_the_harness_did() {
        // Expected values are `gate_variants.py`'s `sentences` and, for
        // clauses, `pick_with`'s cut and filter, run on the same strings.
        // Reverts: split after `?` too; drop the blank-line rule; count
        // bytes rather than characters; make parentheses a boundary.
        let cases: [(&str, &[&str], &[&str]); 5] = [
            (
                "Version 1.2 ships. e.g. the lower-case start stays joined. See (below) for more.",
                &["Version 1.2 ships. e.g. the lower-case start stays joined.", "See (below) for more."],
                &["Version 1.2 ships. e.g. the lower-case start stays joined.", "See (below) for more."],
            ),
            (
                "First paragraph without a stop\n\nSecond paragraph, which follows a blank line, and runs on.\n   \n  Third after a whitespace-only line — with a dash clause – and an en dash.",
                &["First paragraph without a stop", "Second paragraph, which follows a blank line, and runs on.", "Third after a whitespace-only line — with a dash clause – and an en dash."],
                &["First paragraph without a stop", "Second paragraph", "which follows a blank line", "Third after a whitespace-only line", "with a dash clause", "and an en dash."],
            ),
            (
                "Short. Tiny. A sentence long enough to keep: it has a colon, a comma; and a semicolon.",
                &["A sentence long enough to keep: it has a colon, a comma; and a semicolon."],
                &["A sentence long enough to keep", "it has a colon", "and a semicolon."],
            ),
            (
                "Quoted: \"the reply\" is parsed.  *Emphasis* follows a double space.\tA tab then 'quote' ends it.",
                &["\"the reply\" is parsed.", "*Emphasis* follows a double space.", "A tab then 'quote' ends it."],
                &["\"the reply\" is parsed.", "*Emphasis* follows a double space.", "A tab then 'quote' ends it."],
            ),
            (
                "Évidence starts with a non-ASCII capital. So this does not split before it? It does split here. Ünd not here.",
                &["Évidence starts with a non-ASCII capital.", "So this does not split before it? It does split here. Ünd not here."],
                &["Évidence starts with a non-ASCII capital.", "So this does not split before it? It does split here. Ünd not here."],
            ),
        ];
        for (text, sentences, clauses) in cases {
            assert_eq!(gate_units(text, GateUnit::Sentence), sentences, "{text:?}");
            assert_eq!(gate_units(text, GateUnit::Clause), clauses, "{text:?}");
        }
        // Thirteen characters is kept and twelve is not, counted in
        // characters: `Ärger über x` is twelve and fourteen bytes.
        assert_eq!(gate_units("Ärger über x.\n\nÄrger über x", GateUnit::Sentence), ["Ärger über x."]);
    }

    #[test]
    fn the_gate_asks_what_the_harness_asked_byte_for_byte() {
        // The fixtures are `scripts/verifier-eval/wire_fixtures.py`: the
        // harness's own requests for `pick_cls` and `pick_clause`,
        // captured rather than transcribed. Reverts: build the questions
        // with `json!` (sorted keys: NONE first, S10 before S2); swap the
        // kind question's `criteria` and `instructions`; paraphrase NONE.
        let text = include_str!("../tests/fixtures/typed_wire/text.txt");
        for (verb, fixture) in [
            ("fact", include_str!("../tests/fixtures/typed_wire/gate_fact.json")),
            ("claim", include_str!("../tests/fixtures/typed_wire/gate_claim.json")),
        ] {
            let gate = typed_legs(verb).gate.expect("a gate");
            let units = gate_units(text, gate.unit);
            assert!(units.len() >= 10, "{verb}: the fixture no longer reaches S10");
            assert_eq!(json_ordered(&gate_questions(gate, &units)), fixture.trim_end(), "{verb}");
        }
    }

    #[test]
    fn the_gate_goes_out_in_the_measured_key_order_and_chunks_at_forty() {
        // 45 sentences: `which` and k1..k39 in one call, k40..k45 in the
        // next. Every kind answers CURRENT at 0.1, so the score is 0.09 and
        // the subject is gated — unless the second call's answers were
        // dropped, when k45 counts whole and the score is 0.9. Reverts:
        // keep only the last chunk's answers or only the first's; chunk
        // at 41.
        let text: Vec<String> = (1..=45).map(|i| format!("Sentence number {i} is here.")).collect();
        let subject = Subject { verb: "fact".into(), ..subject_fixture(&text.join(" "), &[("F1", &["x"])]) };
        let mut which: serde_json::Map<String, serde_json::Value> = (1..45).map(|i| (format!("S{i}"), json!(0.0))).collect();
        which.insert("S45".into(), json!(0.9));
        which.insert("NONE".into(), json!(0.1));
        let typed = typed_mock(which.into(), 0.1, "CORRECT");
        let llm = llm_mock();
        let mut tel = Telemetry::default();
        let (status, _) = gated_run(&llm, &typed, gate_only(Some(JEV)), &subject, &mut tel);
        assert_eq!(status, Status::Gated, "{:?}", tel.detail);
        assert_eq!(tel.gate_calls, 2);
        let sent = typed.bodies();
        assert_eq!(sent.len(), 2);
        // The harness's `MAX_QUESTIONS_PER_CALL`, not this crate's constant.
        assert_eq!(sent[0]["questions"].as_object().map(serde_json::Map::len), Some(40));
        assert!(sent[1]["questions"].get("k45").is_some() && sent[1]["questions"].get("which").is_none());
        let raw = &typed.raw()[0];
        let at = |needle: &str| raw.find(needle).unwrap_or_else(|| panic!("{needle} not sent"));
        assert!(at(r#"{"state":"AUTHOR'S TEXT:\nSentence number 1"#) == 0, "{raw}");
        assert!(at(r#","model":"jev-latest","questions":{"which":"#) < at(r#""k1":"#));
        assert!(at(r#""S9":"#) < at(r#""S10":"#) && at(r#""S10":"#) < at(r#""NONE":"#));
        assert!(llm.bodies().is_empty());
    }

    #[test]
    fn a_subject_scored_below_its_threshold_is_gated_and_nothing_else_runs() {
        // Invariant 2. Revert: return `ok` with no findings on a skip —
        // the status assertion fails, and the gated record would carry the
        // clean `findings: []` the status guard exists to withhold.
        for verb in ["fact", "claim"] {
            let subject = Subject { verb: verb.into(), ..fact_subject() };
            let typed = typed_mock(json!({"S1": 0.1, "NONE": 0.9}), 1.0, "CORRECT");
            let llm = llm_mock();
            let mut tel = Telemetry::default();
            let (status, f) = gated_run(&llm, &typed, gate_only(Some(JEV)), &subject, &mut tel);
            assert_eq!(status, Status::Gated, "{verb}: {:?}", tel.detail);
            assert!(f.is_empty());
            assert!(llm.bodies().is_empty(), "{verb}: a gated subject still called classify, check or literals");
            assert_eq!((tel.gate_calls, tel.attempts), (1, 1), "{verb}");
            assert!(tel.cost > 0.0, "{verb}: the gate's call totalled nothing");
            assert_eq!(tel.typed_versions, [MEASURED_TYPED_VERSION]);
        }
        // Contrast: the same subject scored above the thresholds is
        // checked. On `fact` the kind weighs in, so a unit picked at 0.9
        // that is judged not to describe the code still gates.
        for (verb, current, gated) in [("fact", 1.0, false), ("claim", 1.0, false), ("fact", 0.1, true)] {
            let subject = Subject { verb: verb.into(), ..fact_subject() };
            let typed = typed_mock(json!({"S1": 0.9, "NONE": 0.1}), current, "CORRECT");
            let llm = llm_mock();
            let mut tel = Telemetry::default();
            let (status, _) = gated_run(&llm, &typed, gate_only(Some(JEV)), &subject, &mut tel);
            assert_eq!(status == Status::Gated, gated, "{verb} at CURRENT {current}: {status:?}");
            assert_eq!(llm.bodies().is_empty(), gated, "{verb} at CURRENT {current}");
        }
    }

    #[test]
    fn the_gate_never_runs_where_the_row_has_none_or_nothing_is_set() {
        // Reverts: gate every verb; gate without `verify.typed_model`.
        let prose = Subject { verb: "prose".into(), ..fact_subject() };
        for (subject, tm) in [(&prose, Some(JEV)), (&fact_subject(), None)] {
            let typed = typed_mock(json!({"S1": 0.0, "NONE": 1.0}), 1.0, "CORRECT");
            let llm = llm_mock();
            let mut tel = Telemetry::default();
            let (status, _) = gated_run(&llm, &typed, gate_only(tm), subject, &mut tel);
            assert_ne!(status, Status::Gated, "{} {tm:?}", subject.verb);
            assert!(typed.bodies().is_empty(), "{} {tm:?}: asked TypeSafe", subject.verb);
            assert_eq!(tel.gate_calls, 0);
        }
        // Text too short to offer a unit is checked without asking.
        let short = Subject { verb: "fact".into(), ..subject_fixture("it is fast", &[("F1", &["x"])]) };
        let typed = typed_mock(json!({"NONE": 1.0}), 1.0, "CORRECT");
        let llm = llm_mock();
        let mut tel = Telemetry::default();
        gated_run(&llm, &typed, gate_only(Some(JEV)), &short, &mut tel);
        assert!(typed.bodies().is_empty() && !llm.bodies().is_empty());
    }

    #[test]
    fn a_gate_that_fails_runs_the_check_and_says_so() {
        // Invariant 3. Revert: treat a failed gate as a skip (`Err(_) =>
        // return (Status::Gated, …)`) — every case below is then gated.
        // The partial answers each score zero as the harness read them —
        // a skip. Revert: default a missing probability, as the harness did.
        let unanswered = || Mock::start(|_| (200, json!({"model": MEASURED_TYPED_VERSION, "answers": {}})));
        for (verb, typed, want) in [
            ("fact", Mock::start(|_| (500, json!({}))), Status::Unavailable),
            ("fact", Mock::start(|_| (429, json!({}))), Status::Unavailable),
            ("fact", unanswered(), Status::Unparsable),
            ("fact", typed_mock(json!({}), 1.0, "CORRECT"), Status::Unparsable),
            ("fact", typed_mock(json!({"NONE": 0.1}), 1.0, "CORRECT"), Status::Unparsable),
            ("claim", typed_mock(json!({"S1": 0.9}), 1.0, "CORRECT"), Status::Unparsable),
        ] {
            let llm = llm_mock();
            let mut tel = Telemetry::default();
            let subject = Subject { verb: verb.into(), ..fact_subject() };
            let (status, f) = gated_run(&llm, &typed, gate_only(Some(JEV)), &subject, &mut tel);
            assert_eq!(status, Status::Ok, "{verb} {want:?}: {:?}", tel.detail);
            assert_eq!(f.len(), 1, "{want:?}");
            assert_eq!(tel.gate_status, Some(want));
            assert_eq!(tel.gate_calls, 1);
            assert!(!llm.bodies().is_empty());
        }
        let r = Record { findings: vec![finding_fixture()], gate_calls: 1, gate_status: Some("unavailable".into()), ..record_fixture() };
        let b = block(&settings_fixture(), "claim", Some(&r), Trigger::NotAttempted, None);
        assert_eq!(b["gate_incomplete"], "unavailable", "{b}");
        let clean = Record { gate_status: None, ..r };
        assert!(block(&settings_fixture(), "claim", Some(&clean), Trigger::NotAttempted, None).get("gate_incomplete").is_none());
    }

    #[test]
    fn one_typesafe_model_on_both_keys_still_refutes() {
        // Invariant 5. The gate and the refuter are the same model on
        // `fact`, and every finding the refuter is handed came from the
        // LLM check, so the self-refutation guard has nothing to guard.
        // Revert: extend the guard to the typed model (`refuter.filter(|r|
        // Some(*r) != typed_model)` in `run`) — the WRONG below then stands.
        let typed = typed_mock(json!({"S1": 0.9, "NONE": 0.1}), 1.0, "WRONG");
        let llm = llm_mock();
        let mut tel = Telemetry::default();
        let legs = Legs { literals: false, refuter: Some(JEV), typed_model: Some(JEV) };
        let (status, f) = gated_run(&llm, &typed, legs, &fact_subject(), &mut tel);
        assert_eq!(status, Status::Ok, "{:?}", tel.detail);
        assert!(f.is_empty(), "the refuter did not run: {f:?}");
        assert_eq!(tel.refuted, 1);
        let asked: Vec<bool> = typed.bodies().iter().map(|b| b["questions"].get("which").is_some()).collect();
        assert_eq!(asked, [true, false], "gate first, then one refutation");
    }

    #[test]
    fn a_verification_that_ran_the_gate_is_not_counted_as_retried() {
        // Invariant 10. A `split` check behind a gate makes three calls
        // unretried. Revert: compare attempts against the LLM count alone
        // (or drop `gate_calls` from the comparison) — the first record
        // counts as a retry. A gated record makes one call, fewer than
        // either count, so it is the ungated one this guards.
        let ran = Record { attempts: 3, gate_calls: 1, ..record_fixture() };
        let gated = Record { status: "gated".into(), attempts: 1, gate_calls: 1, ..record_fixture() };
        assert_eq!(retried(&[ran.clone(), gated]), 0);
        // Contrast: one more attempt than that is a retry.
        assert_eq!(retried(&[Record { attempts: 4, ..ran }]), 1);
        assert_eq!(expected_calls("split", false, RefuterLeg::Off, TypedCalls { gate: 1, ..TypedCalls::default() }), Calls { llm: 2, typed: 1 });
    }

    #[test]
    fn a_gated_record_carries_no_findings_but_keeps_the_version_flag() {
        // The status guard withholds `findings`; the version keys are
        // outside it (invariant 7). Revert: emit `findings` for `gated`.
        let r = Record { status: "gated".into(), typed_versions: vec!["jev-1.14.0".into()], ..record_fixture() };
        let s = Settings { typed_model: Some(JEV.into()), ..settings_fixture() };
        let b = block(&s, "claim", Some(&r), Trigger::NotAttempted, None);
        assert!(b.get("findings").is_none(), "{b}");
        assert_eq!(b["typed_model_unmeasured"], true, "{b}");
        assert_eq!(b["typed_model"], JEV, "{b}");
        assert_eq!(Status::Gated.as_str(), "gated");
    }

    #[test]
    fn a_typed_model_demands_its_credential_where_its_gate_runs() {
        // Invariant 6, for the gate. Reverts: demand the key on every verb
        // (prose has no gate); demand it for the refuter alone.
        let s = Settings { typed_model: Some(JEV.into()), ..settings_fixture() };
        let key = || Some("k".to_string());
        for verb in ["fact", "claim"] {
            assert!(providers_for(&s, verb, key(), None).is_none(), "{verb} gated without its key");
            assert!(providers_for(&s, verb, key(), key()).is_some_and(|p| p.typed.is_some()), "{verb}");
            let d = unauthorized_detail(&s, verb, true, false).expect("a gap");
            assert!(d.contains("verify.typed_model") && d.contains(TYPED_KEY_VAR), "{d}");
        }
        assert!(providers_for(&s, "prose", key(), None).is_some_and(|p| p.typed.is_none()));
        assert_eq!(unauthorized_detail(&s, "prose", true, false), None);
    }

    #[test]
    fn the_budget_pays_for_each_typed_leg_where_it_runs() {
        // Reverts: leave the gate out of `planned_calls`, which `settings`
        // budgets from; price a typed classify or literal leg on top of the
        // LLM call it replaces rather than in its place; price either off
        // `split` or with the literal leg off.
        let m = Some("openai/gpt-5.6-luna");
        let calls = |llm, typed| Calls { llm, typed };
        for (verb, approach, literals, untyped, typed) in [
            ("fact", "split", true, calls(3, 0), calls(3, 1)),
            ("claim", "split", false, calls(2, 0), calls(1, 2)),
            ("claim", "split", true, calls(3, 0), calls(1, 3)),
            ("claim", "direct", false, calls(1, 0), calls(1, 1)),
            ("claim", "direct", true, calls(2, 0), calls(1, 2)),
            ("prose", "split", true, calls(3, 0), calls(3, 0)),
        ] {
            let plan = |tm| planned_calls(verb, approach, literals, None, m, tm);
            assert_eq!((plan(None), plan(Some(JEV))), (untyped, typed), "{verb} {approach} literals={literals}");
        }
    }

    #[test]
    fn jev_classify_and_literals_go_out_as_the_harness_sent_them() {
        // `wire_fixtures.py` captures both from `classify_jev.py` and
        // `literals_jev.py` on the same claim. Reverts: reorder a question's
        // keys; cut units inside brackets; key units from 1; drop a
        // candidate's noun; reword a criterion.
        let claim = include_str!("../tests/fixtures/typed_wire/claim.txt");
        let classify = json_ordered(&classify_questions(&classify_units(claim)));
        assert_eq!(classify, include_str!("../tests/fixtures/typed_wire/classify_claim.json").trim_end());
        let judged: Vec<&str> = literal_candidates(claim).into_iter().filter(|l| is_checkable(l)).collect();
        let literals = json_ordered(&literal_questions(claim, &judged));
        assert_eq!(literals, include_str!("../tests/fixtures/typed_wire/literals_claim.json").trim_end());
    }

    #[test]
    fn the_literal_and_clause_ports_find_what_the_harness_found() {
        // The regexes and splitters are hand-ported, so each is compared on
        // texts built to reach their backtracking and their edge cases.
        // Reverts: let NOUN take a `(` word; try `%` after no `%`; drop the
        // lookbehind on figures; cut clauses inside a code span; read the
        // sentence's position from the slice rather than `find`.
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/fixtures/typed_wire/splits_claim.json")).expect("fixture");
        for r in &rows {
            let text = r["text"].as_str().expect("text");
            let want = |k: &str| -> Vec<&str> { r[k].as_array().expect(k).iter().filter_map(|v| v.as_str()).collect() };
            let candidates = literal_candidates(text);
            let clauses: Vec<&str> = candidates.iter().map(|c| literal_clause(text, c)).collect();
            assert_eq!(candidates, want("candidates"), "{text}");
            assert_eq!(clauses, want("clauses"), "{text}");
            assert_eq!(classify_units(text), want("units"), "{text}");
        }
    }

    #[test]
    fn jev_labels_a_part_current_from_its_threshold_and_needs_a_whole_answer() {
        // Reverts: take the plain choice; label at > rather than >=; score
        // a missing probability as zero, as the harness did.
        let a = |current: f64, choice: &str| {
            json!({"choice": choice, "probabilities": {"current": current, "proposed": 1.0 - current, "argument": 0.0}})
        };
        assert_eq!(classify_label(&a(0.4, "proposed")), Some("current"));
        assert_eq!(classify_label(&a(0.39, "proposed")), Some("proposed"));
        assert_eq!(classify_label(&a(0.39, "argument")), Some("argument"));
        assert_eq!(classify_label(&json!({"choice": "current", "probabilities": {"current": 0.9, "proposed": 0.1}})), None);
        assert_eq!(classify_label(&json!({"probabilities": {"current": 0.1, "proposed": 0.9, "argument": 0.0}})), None);
        assert_eq!(classify_label(&a(0.1, "PROPOSED")), None);
    }

    #[test]
    fn jev_classifies_a_claim_in_the_llms_place_and_the_check_reads_its_labels() {
        // Reverts: keep the LLM classify call beside Jev's; run Jev
        // classify under `direct`; label from the choice alone.
        let subject = Subject { verb: "claim".into(), ..fact_subject() };
        let typed = typed_mock(json!({"S1": 0.9, "NONE": 0.1}), 0.4, "CORRECT");
        let llm = llm_mock();
        let mut tel = Telemetry::default();
        let legs = Legs { literals: false, refuter: None, typed_model: Some(JEV) };
        let (status, f) = gated_run(&llm, &typed, legs, &subject, &mut tel);
        assert_eq!(status, Status::Ok, "{:?}", tel.detail);
        assert_eq!(f.len(), 1);
        let systems: Vec<String> = llm.bodies().iter().map(|b| b["messages"][0]["content"].as_str().unwrap_or("").to_string()).collect();
        assert_eq!(systems, [CHECK_SYSTEM], "the LLM classified too, or the check did not run");
        let check = llm.bodies()[0]["messages"][1]["content"].as_str().unwrap_or("").to_string();
        assert!(check.contains(r#"ASSERTIONS:
{"assertions":[{"label":"current","text":"the function returns early"}]}"#), "{check}");
        assert_eq!((tel.gate_calls, tel.typed_classify_calls, tel.attempts), (1, Some(1), 3));
        let classify = &typed.bodies()[1];
        assert_eq!(classify["state"], "CLAIM:\nthe function returns early");
        assert!(classify["questions"].get("u0").is_some());
        // Contrast: `direct` asks no one to classify.
        let typed = typed_mock(json!({"S1": 0.9, "NONE": 0.1}), 0.4, "CORRECT");
        let llm = llm_mock();
        let mut tel = Telemetry::default();
        run(&providers_fixture(&llm.url, &typed.url), "openai/gpt-5.6-luna", "direct", legs, &subject, Instant::now(), Duration::from_secs(20), &mut tel);
        assert_eq!((typed.bodies().len(), llm.bodies().len(), tel.typed_classify_calls), (1, 1, None));
    }

    #[test]
    fn a_claim_jev_cannot_label_fails_and_one_with_no_part_is_checked_unlabelled() {
        // A missing answer ends the verification as a failed LLM classify
        // does. Reverts: skip the part; default its label.
        let subject = Subject { verb: "claim".into(), ..fact_subject() };
        let typed = Mock::start(|b| {
            let which = b["questions"].get("which").is_some();
            let answers = if which { json!({"which": {"probabilities": {"S1": 0.9, "NONE": 0.1}}}) } else { json!({}) };
            (200, json!({"model": MEASURED_TYPED_VERSION, "answers": answers}))
        });
        let llm = llm_mock();
        let mut tel = Telemetry::default();
        let legs = Legs { literals: false, refuter: None, typed_model: Some(JEV) };
        let (status, _) = gated_run(&llm, &typed, legs, &subject, &mut tel);
        assert_eq!(status, Status::Unparsable, "{:?}", tel.detail);
        assert!(llm.bodies().is_empty());
        // Too short to offer a part: no typed call, and the check runs in
        // `direct`'s shape. Revert: send an empty question map.
        let short = subject_fixture("it is fast", &[("F1", &["x"])]);
        let typed = typed_mock(json!({"NONE": 1.0}), 1.0, "CORRECT");
        let llm = llm_mock();
        let mut tel = Telemetry::default();
        let (status, _) = gated_run(&llm, &typed, legs, &short, &mut tel);
        assert_eq!(status, Status::Ok, "{:?}", tel.detail);
        assert!(typed.bodies().is_empty());
        assert_eq!(tel.typed_classify_calls, Some(0));
        let check = llm.bodies()[0]["messages"][1]["content"].as_str().unwrap_or("").to_string();
        assert_eq!(check, check_prompt(&short, None));
    }

    /// A claim with two literals: `40 entries`, which nothing captured
    /// carries, and `12 lines`, which the capture does.
    fn literal_subject() -> Subject {
        subject_fixture("The cache holds 40 entries and the log is 12 lines long.", &[("F1", &["log: 12 lines long"])])
    }

    #[test]
    fn jev_judges_a_claims_literals_at_the_measured_thresholds() {
        // Reverts: keep at P(quantity) > 0.7 rather than >=; keep at
        // P(carried) <= 0.5; ask about a literal the capture carries.
        for (quantity, carried, kept) in [(0.7, 0.49, true), (0.69, 0.0, false), (1.0, 0.5, false)] {
            let typed = typed_mock_literals(json!({"S1": 0.9, "NONE": 0.1}), 1.0, "CORRECT", Some(quantity), Some(carried));
            let llm = llm_mock();
            let mut tel = Telemetry::default();
            let (status, f) = gated_run(&llm, &typed, gate_only(Some(JEV)), &literal_subject(), &mut tel);
            assert_eq!(status, Status::Ok, "{:?}", tel.detail);
            let lits: Vec<_> = f.iter().filter_map(|f| f.literal.as_deref()).collect();
            assert_eq!(lits, if kept { vec!["40 entries"] } else { vec![] }, "q {quantity} c {carried}");
            assert_eq!((tel.literals_refuted, tel.typed_literal_calls), (1, Some(1)));
            let asked = &typed.bodies()[2];
            assert!(asked["state"].as_str().is_some_and(|s| s.starts_with("TEXT:\nThe cache holds")));
            let keys: Vec<&String> = asked["questions"].as_object().expect("questions").keys().collect();
            assert_eq!(keys, ["c0", "q0"]);
            if kept {
                let finding = f.iter().find(|f| f.literal.is_some()).expect("finding");
                assert_eq!((finding.kind.as_str(), finding.why.as_str()), (KIND_UNEVIDENCED, TYPED_LITERAL_WHY));
                assert!(finding.clause_quoted && !finding.quoted);
            }
            let systems: Vec<_> = llm.bodies().iter().map(|b| b["messages"][0]["content"].as_str().unwrap_or("").to_string()).collect();
            assert!(!systems.iter().any(|s| s == LITERALS_SYSTEM), "the LLM literal leg ran too");
        }
    }

    #[test]
    fn a_literal_answer_jev_left_out_fails_the_leg_and_keeps_the_check() {
        // Reverts: read a missing P(carried) as not carried, which keeps
        // the finding; fail the whole verification.
        let typed = typed_mock_literals(json!({"S1": 0.9, "NONE": 0.1}), 1.0, "CORRECT", Some(1.0), None);
        let llm = llm_mock();
        let (status, f, lit) = run(
            &providers_fixture(&llm.url, &typed.url),
            "openai/gpt-5.6-luna",
            "split",
            gate_only(Some(JEV)),
            &literal_subject(),
            Instant::now(),
            Duration::from_secs(20),
            &mut Telemetry::default(),
        );
        assert_eq!((status, lit), (Status::Ok, Some(Status::Unparsable)));
        assert_eq!(f.len(), 1, "the check's finding went with the leg");
    }

    #[test]
    fn fact_runs_no_jev_classify_or_literals_and_no_row_refutes_its_own_findings() {
        // Invariant 8. Reverts: set either cell on `fact`; set `refuter`
        // on `claim`.
        // The LLM classify stand-in quotes "the function returns early".
        let text = "the function returns early after 40 entries.";
        let subject = Subject { verb: "fact".into(), ..subject_fixture(text, &[("F1", &["return"])]) };
        let typed = typed_mock_literals(json!({"S1": 0.9, "NONE": 0.1}), 1.0, "CORRECT", Some(1.0), Some(0.0));
        let llm = llm_mock();
        let mut tel = Telemetry::default();
        let (status, _) = gated_run(&llm, &typed, gate_only(Some(JEV)), &subject, &mut tel);
        assert_eq!(status, Status::Ok, "{:?}", tel.detail);
        let asked: Vec<String> = typed.bodies().iter().flat_map(|b| b["questions"].as_object().map(|q| q.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()).collect();
        assert!(asked.iter().all(|k| k == "which" || k.starts_with('k')), "{asked:?}");
        let systems: Vec<_> = llm.bodies().iter().map(|b| b["messages"][0]["content"].as_str().unwrap_or("").to_string()).collect();
        assert!(systems.iter().any(|s| s == CLASSIFY_SYSTEM) && systems.iter().any(|s| s == LITERALS_SYSTEM));
        assert_eq!((tel.typed_classify_calls, tel.typed_literal_calls), (None, None));
        for (verb, row) in TYPED_LEGS {
            assert!(!(row.literals && row.refuter), "{verb}: Jev would refute its own findings");
        }
        assert!(typed_legs("claim").literals && !typed_legs("claim").refuter);
    }

    #[test]
    fn a_verification_whose_typed_legs_stood_in_is_not_counted_as_retried() {
        // Invariant 10. The totals only part where a typed leg made other
        // than one call: two each, or none at all. Revert: count the
        // classify and literal legs as the LLM calls they replaced.
        let ran = |attempts, n| Record {
            approach: "split".into(),
            literals: true,
            attempts,
            gate_calls: 1,
            typed_classify_calls: Some(n),
            typed_literal_calls: Some(n),
            ..record_fixture()
        };
        assert_eq!(retried(&[ran(6, 2), ran(2, 0)]), 0);
        // Contrast: one more attempt than either is a retry.
        assert_eq!(retried(&[ran(7, 2), ran(3, 0)]), 2);
    }

    #[test]
    fn a_typed_leg_without_its_endpoint_fails_rather_than_falling_back() {
        // A wiring fault: `spawn` builds the endpoint whenever the row runs
        // a typed leg. Revert: fall back to the LLM leg.
        let llm = llm_mock();
        let subject = Subject { verb: "claim".into(), ..literal_subject() };
        let providers = Providers { llm: Endpoint { url: llm.url.clone(), key: "k".into() }, typed: None };
        let mut tel = Telemetry::default();
        let run_with = |approach, tel: &mut Telemetry| {
            run(&providers, "openai/gpt-5.6-luna", approach, gate_only(Some(JEV)), &subject, Instant::now(), Duration::from_secs(20), tel)
        };
        assert_eq!(run_with("split", &mut tel).0, Status::Unauthorized);
        let (status, _, lit) = run_with("direct", &mut tel);
        assert_eq!((status, lit), (Status::Ok, Some(Status::Unauthorized)));
    }

    #[test]
    fn the_report_rates_the_llm_literal_leg_without_jevs_candidates() {
        // Jev's filter counts are over code-proposed candidates. Revert:
        // sum every record's counters into the rates.
        let literal = Finding { kind: KIND_UNEVIDENCED.into(), literal: Some("40 entries".into()), ..finding_fixture() };
        let llm = Record { findings: vec![literal.clone()], literals_refuted: 1, ..record_fixture() };
        let jev = Record {
            findings: vec![literal],
            literals_refuted: 30,
            not_a_quantity: 30,
            typed_literal_calls: Some(1),
            ..record_fixture()
        };
        let out = fidelity_text(&[llm, jev], false);
        assert!(out.contains("machine-refuted  1 "), "{out}");
        assert!(out.contains("not a quantity   0 "), "{out}");
        assert!(out.contains("50% of what it raised"), "{out}");
        assert!(out.contains("LITERALS, judged by Jev\n  unevidenced      1   over 1 verification(s)"), "{out}");
    }

    #[test]
    fn a_refused_typed_model_is_stated_where_a_typed_leg_would_run() {
        // Reverts: never state it (the typed legs stop and the reply is
        // silent, TET-99); state it on every verb (prose has no typed leg
        // to lose); make it a gap (a refused value is off, not missing a
        // credential, so the status must not change).
        let why = "`verify.typed_model` takes a `typesafe/` model (sentinel 7f3e)".to_string();
        let s = Settings {
            typed_model_refusal: Some(why.clone()),
            approach: "split".into(),
            verbs: vec!["claim".into(), "fact".into(), "prose".into()],
            ..settings_fixture()
        };
        for verb in ["claim", "fact"] {
            let b = block(&s, verb, None, Trigger::NotAttempted, None);
            assert_eq!(b["typed_model_refused"], why, "{verb}: {b}");
            assert!(b.get("typed_model").is_none() && b.get("typed_model_not_run").is_none(), "{verb}: {b}");
            assert_eq!(unauthorized_detail(&s, verb, true, false), None, "{verb}");
        }
        let b = block(&s, "prose", None, Trigger::NotAttempted, None);
        assert!(b.get("typed_model_refused").is_none(), "{b}");
        let off = Settings { enabled: false, ..s.clone() };
        assert!(block(&off, "fact", None, Trigger::NotAttempted, None).get("typed_model_refused").is_none());
        let unlisted = Settings { verbs: vec!["fact".into()], ..s };
        assert!(block(&unlisted, "claim", None, Trigger::NotAttempted, None).get("typed_model_refused").is_none());
    }

    #[test]
    fn a_refused_model_is_named_rather_than_reported_unset() {
        // Invariant 4, at the printer: what `settings()` resolved from the
        // file reaches `detail`. The file end is `tests/mcp_cli.rs`.
        let why = crate::config::typed_model_refusal(crate::config::KEY_VERIFY_MODEL, "typesafe/jev-1.13.0");
        let s = Settings { model: None, model_refusal: Some(why.clone()), ..settings_fixture() };
        let d = unauthorized_detail(&s, "claim", true, true).expect("a gap");
        assert!(d.contains("typesafe/jev-1.13.0"), "{d}");
        assert!(!d.contains("is not set"), "told the key is unset: {d}");
    }

    #[test]
    fn prose_is_announced_as_a_paragraph_and_split_by_its_own_prompt() {
        // Measured together: the header and the prose classify prompt took
        // the proposal cluster from 11 findings to 0. `fact` keeps `CLAIM:`
        // because every fact number on record was measured with it.
        let mut prose = subject_fixture("some text", &[("F1", &["alpha"])]);
        prose.verb = "prose".into();
        assert!(classify_prompt(&prose).starts_with("PARAGRAPH:\n"));
        assert!(check_prompt(&prose, None).starts_with("PARAGRAPH:\n"));
        assert_eq!(classify_system_for("prose"), PROSE_CLASSIFY_SYSTEM);
        assert!(PROSE_CLASSIFY_SYSTEM.starts_with("You are given one PARAGRAPH"));

        let mut fact = subject_fixture("some text", &[("F1", &["alpha"])]);
        fact.verb = "fact".into();
        assert!(classify_prompt(&fact).starts_with("CLAIM:\n"));
        assert_eq!(classify_system_for("fact"), CLASSIFY_SYSTEM);
        assert_eq!(classify_system_for("claim"), CLASSIFY_SYSTEM);
    }

    #[test]
    fn the_fact_prompt_still_asks_for_the_kind_the_code_drops() {
        // This looks like an inconsistency and is the measured
        // configuration: `fact_v1.json` was drawn from a prompt that names
        // both kinds, and `kind_reported_for` removes one afterwards.
        // Writing the kind out of the prompt has never been scored, and the
        // twice-measured result of instructing a model away from a move is
        // that it relocates rather than stops. Delete these paragraphs and
        // the 63% describes a run nobody made.
        assert!(FACT_SYSTEM.contains(KIND_OVERREACHES), "the bound is mechanical, not prompted");
        assert!(!kind_reported_for("fact", KIND_OVERREACHES));
    }

    // -----------------------------------------------------------------
    // TET-84: a verification that did not run says so.
    // -----------------------------------------------------------------

    /// What each of the four reply-text sites is driven with. Short, and
    /// inside the 200 characters every excerpt keeps.
    const SENTINEL: &str = "SENTINEL-7f3a-reply-text";

    #[test]
    fn a_delivered_failure_says_why() {
        // C8 (1). Revert: remove the `detail` insert in `block`.
        let r = Record {
            status: "timeout".into(),
            revision: Some(0),
            detail: Some("provider did not answer within the remaining budget".into()),
            ..record_fixture()
        };
        let b = block(&settings_fixture(), "claim", Some(&r), Trigger::NotAttempted, None);
        assert_eq!(b["detail"], "provider did not answer within the remaining budget", "{b}");
        // And under the other two failure statuses.
        for status in ["unavailable", "unparsable"] {
            let r = Record { status: status.into(), detail: Some("provider replied 503".into()), revision: Some(0), ..record_fixture() };
            let b = block(&settings_fixture(), "claim", Some(&r), Trigger::NotAttempted, None);
            assert_eq!(b["detail"], "provider replied 503", "{status}: {b}");
        }
    }

    #[test]
    fn a_failure_written_before_the_split_delivers_no_detail() {
        // A record from before this change has no revision, and its detail
        // may quote the reply it could not read. Revert: drop the
        // `revision.is_some()` conjunct.
        let r = Record {
            status: "unparsable".into(),
            detail: Some(format!("reply was not a usable answer; 40 bytes beginning: {SENTINEL}")),
            ..record_fixture()
        };
        assert_eq!(r.revision, None, "premise: a legacy record");
        let b = block(&settings_fixture(), "claim", Some(&r), Trigger::NotAttempted, None);
        assert!(b.get("detail").is_none(), "{b}");
        assert_eq!(b["guidance"], UNCHECKED_GUIDANCE, "still told the mint went unchecked: {b}");
    }

    #[test]
    fn a_clean_status_delivers_no_detail_even_when_its_record_has_one() {
        // C8 (2). Revert: widen the insert to every record with a detail.
        let r = Record {
            refuter_status: Some("timeout".into()),
            detail: Some("provider did not answer within the remaining budget".into()),
            ..record_fixture()
        };
        let b = block(&settings_fixture(), "claim", Some(&r), Trigger::NotAttempted, None);
        assert_eq!(b["refuter_incomplete"], "timeout", "premise: the qualifier names the leg: {b}");
        assert!(b.get("detail").is_none(), "{b}");
        let gated = Record { status: "gated".into(), ..r };
        let b = block(&settings_fixture(), "claim", Some(&gated), Trigger::NotAttempted, None);
        assert!(b.get("detail").is_none(), "{b}");
    }

    /// `spawn`'s path from a finished `run` to the logged record, without
    /// the thread or the credential.
    fn record_after(subject: &Subject, approach: &str, legs: Legs<'_>, providers: &Providers, budget: Duration) -> Record {
        let mut tel = Telemetry::default();
        let started = Instant::now();
        let out = run(providers, "openai/gpt-5.6-luna", approach, legs, subject, started, budget, &mut tel);
        let ran = Ran { model: "openai/gpt-5.6-luna".into(), approach: approach.into(), literals: legs.literals, refuter: legs.refuter };
        record_of(1, subject, ran, out, tel, started.elapsed())
    }

    #[test]
    fn a_failure_after_a_failed_gate_delivers_its_own_cause_not_the_gates() {
        // C8 (3). The gate is the one leg that fails without ending the
        // verification, so its detail is written first and must not be
        // what is delivered. Revert: have the gate's failure arm write the
        // detail after the check call has returned.
        let typed = Mock::start(|_| (503, json!({})));
        let providers = Providers {
            llm: headers_then(Some(Duration::from_secs(3))),
            typed: Some(Endpoint { url: typed.url.clone(), key: "typed-key".into() }),
        };
        let subject = fact_subject();
        let legs = Legs { literals: false, refuter: None, typed_model: Some(JEV) };
        let r = record_after(&subject, "direct", legs, &providers, Duration::from_millis(600));
        assert_eq!(r.status, "timeout", "{r:?}");
        assert_eq!(r.gate_status.as_deref(), Some("unavailable"), "premise: the gate failed first: {r:?}");
        let b = block(&settings_fixture(), "fact", Some(&r), Trigger::NotAttempted, None);
        assert_eq!(b["detail"], "provider did not finish its reply within the remaining budget", "{b}");
        assert!(!b.to_string().contains("provider replied 503"), "{b}");
    }

    /// The record's `detail` and everything delivered from it are free of
    /// the sentinel, and its log-only `reply` carries it.
    fn assert_reply_split_off(site: &str, r: &Record) {
        let detail = r.detail.as_deref().unwrap_or_else(|| panic!("{site}: no detail at all: {r:?}"));
        assert!(!detail.contains(SENTINEL), "{site}: reply text in detail: {detail}");
        let reply = r.reply.as_deref().unwrap_or_default();
        assert!(reply.contains(SENTINEL), "{site}: reply text not kept for the log: {r:?}");
        let b = block(&settings_fixture(), &r.verb, Some(r), Trigger::NotAttempted, None);
        assert!(!b.to_string().contains(SENTINEL), "{site}: reply text reached the author: {b}");
        // The record answers for the revision it was dispatched at.
        assert_eq!(r.revision, Some(7), "{site}");
    }

    fn sentinel_subject() -> Subject {
        Subject { revision: 7, ..subject_fixture("the function returns early", &[("F1", &["return"])]) }
    }

    #[test]
    fn the_check_legs_unreadable_reply_stays_out_of_detail() {
        // C8 (4), check leg. Revert: format the excerpt back into `detail`.
        let llm = Mock::start(|_| (200, llm_reply(&format!("not an answer: {SENTINEL}"))));
        let legs = Legs { literals: false, refuter: None, typed_model: None };
        let r = record_after(&sentinel_subject(), "direct", legs, &providers_fixture(&llm.url, DEAD), Duration::from_secs(20));
        assert_eq!(r.status, "unparsable", "{r:?}");
        assert_reply_split_off("check", &r);
    }

    #[test]
    fn a_typed_reply_without_answers_stays_out_of_detail() {
        // C8 (4), the typed call. Jev classify ends the verification here;
        // the gate before it fails the same way and is overwritten.
        // Revert: format the excerpt back into `detail`.
        let typed = Mock::start(|_| (200, json!({"model": MEASURED_TYPED_VERSION, "error": SENTINEL})));
        let legs = Legs { literals: false, refuter: None, typed_model: Some(JEV) };
        let r = record_after(&sentinel_subject(), "split", legs, &providers_fixture(DEAD, &typed.url), Duration::from_secs(20));
        assert_eq!(r.status, "unparsable", "{r:?}");
        assert_eq!(r.typed_classify_calls, Some(1), "premise: Jev classify was the leg that failed: {r:?}");
        assert_reply_split_off("typed", &r);
    }

    #[test]
    fn the_literal_legs_unreadable_reply_stays_out_of_detail() {
        // C8 (4), the literal check. The record ends `ok`, and its detail is
        // kept because the leg did not complete. Revert: format the excerpt
        // back into `detail`.
        let llm = Mock::start(|body| {
            let system = body["messages"][0]["content"].as_str().unwrap_or("");
            (200, llm_reply(&if system == LITERALS_SYSTEM {
                format!("no object here: {SENTINEL}")
            } else {
                r#"{"disagreements":[]}"#.to_string()
            }))
        });
        let legs = Legs { literals: true, refuter: None, typed_model: None };
        let r = record_after(&sentinel_subject(), "direct", legs, &providers_fixture(&llm.url, DEAD), Duration::from_secs(20));
        assert_eq!((r.status.as_str(), r.literals_status.as_deref()), ("ok", Some("unparsable")), "{r:?}");
        assert_reply_split_off("literal", &r);
    }

    #[test]
    fn a_classify_label_outside_the_vocabulary_stays_out_of_detail() {
        // C8 (4), parse_assertions. Revert: format the label back into its
        // explanation.
        let llm = Mock::start(|_| {
            (200, llm_reply(&format!(r#"{{"assertions":[{{"text":"the function returns early","label":"{SENTINEL}"}}]}}"#)))
        });
        let legs = Legs { literals: false, refuter: None, typed_model: None };
        let r = record_after(&sentinel_subject(), "split", legs, &providers_fixture(&llm.url, DEAD), Duration::from_secs(20));
        assert_eq!(r.status, "unparsable", "{r:?}");
        assert!(r.detail.as_deref().unwrap_or_default().contains("not one of"), "{r:?}");
        assert_reply_split_off("classify", &r);
    }

    #[test]
    fn a_failure_is_guided_as_unchecked_and_only_ok_as_findings() {
        // C8 (5). Revert: send GUIDANCE unconditionally.
        for status in ["timeout", "unavailable", "unparsable"] {
            let r = Record { status: status.into(), ..record_fixture() };
            let b = block(&settings_fixture(), "claim", Some(&r), Trigger::NotAttempted, None);
            assert_ne!(b["guidance"], GUIDANCE, "{status}");
            assert_eq!(b["guidance"], UNCHECKED_GUIDANCE, "{status}");
        }
        let b = block(&settings_fixture(), "claim", Some(&record_fixture()), Trigger::NotAttempted, None);
        assert_eq!(b["guidance"], GUIDANCE);
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tetel-unverified-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn plant(dir: &Path, records: &[Record]) -> Vec<Record> {
        let lines: Vec<String> = records.iter().map(|r| serde_json::to_string(r).unwrap()).collect();
        std::fs::write(log_path(dir), format!("{}\n", lines.join("\n"))).unwrap();
        peek_delivered(dir).log
    }

    fn at(verb: &str, mint: &str, revision: Option<u64>, status: &str) -> Record {
        Record { verb: verb.into(), mint: mint.into(), revision, status: status.into(), ..record_fixture() }
    }

    fn verbs(settings: Settings, verbs: &[&str]) -> Settings {
        Settings { verbs: verbs.iter().map(|v| (*v).to_string()).collect(), ..settings }
    }

    #[test]
    fn unverified_names_the_mints_whose_latest_verification_failed() {
        // C8 (6).
        let dir = scratch("tally");
        let events = [
            crate::claims::ClaimEvent::Create { id: "E".into(), prop: "p".into(), from: vec!["F1".into()], timestamp: 0 },
            crate::claims::ClaimEvent::Withdraw { id: "E".into(), why: "w".into(), timestamp: 0 },
        ];
        let ledger: Vec<String> = events.iter().map(|e| serde_json::to_string(e).unwrap()).collect();
        std::fs::write(dir.join("claims.jsonl"), format!("{}\n", ledger.join("\n"))).unwrap();
        let log = plant(&dir, &[
            at("claim", "A", Some(0), "timeout"),
            at("claim", "A", Some(1), "ok"),
            at("claim", "B", Some(0), "unavailable"),
            // G's revision finished first. Red if "latest" is log
            // position, `seq` or `at` — the out-of-order case.
            at("claim", "G", Some(1), "ok"),
            at("claim", "G", Some(0), "timeout"),
            at("claim", "D", Some(2), "unparsable"),
            // Withdrawn. Red if the ledger is not consulted.
            at("claim", "E", Some(0), "timeout"),
            // A verb no longer verified. Red if the verb filter is dropped.
            at("prose", "H", Some(0), "timeout"),
        ]);
        let s = verbs(settings_fixture(), &["claim"]);
        let u = unverified(&dir, &s, &log);
        assert_eq!(u, Some(Unverified { count: 2, mints: vec!["D".into(), "B".into()] }));
        let b = block(&s, "claim", None, Trigger::NotAttempted, u.as_ref());
        assert_eq!(b["unverified"], json!({"count": 2, "mints": ["D", "B"]}), "{b}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unverified_counts_every_mint_and_names_ten() {
        // C8 (7). Revert: drop the `take`.
        let dir = scratch("cap");
        let records: Vec<Record> = (0..11).map(|i| at("fact", &format!("F{i}"), Some(0), "timeout")).collect();
        let log = plant(&dir, &records);
        let u = unverified(&dir, &verbs(settings_fixture(), &["fact"]), &log).expect("eleven failures");
        assert_eq!((u.count, u.mints.len()), (11, 10));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unverified_is_absent_when_verification_is_off_and_present_on_an_off_verb() {
        // C8 (8). Reverts: drop the `enabled` check; insert the key only
        // when the reply's own verb is verified.
        let dir = scratch("off");
        let log = plant(&dir, &[at("claim", "C1", Some(0), "timeout")]);
        let disabled = Settings { enabled: false, ..settings_fixture() };
        let u = unverified(&dir, &disabled, &log);
        assert!(block(&disabled, "claim", None, Trigger::NotAttempted, u.as_ref()).get("unverified").is_none());
        // Verification on for `claim`; this reply is `prose`'s, which is not.
        let on = settings_fixture();
        let u = unverified(&dir, &on, &log);
        let b = block(&on, "prose", None, Trigger::NotAttempted, u.as_ref());
        assert_eq!(b["status"], "off", "premise: this reply's verb is not verified: {b}");
        assert_eq!(b["unverified"], json!({"count": 1, "mints": ["C1"]}), "{b}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_without_a_revision_sorts_behind_one_with_a_revision() {
        // C8 (9). Revert: `revision: u64` defaulting to 0 — the second case
        // then ties at 0 and the later, unrevisioned `ok` wins.
        let dir = scratch("legacy");
        let s = verbs(settings_fixture(), &["fact"]);
        let log = plant(&dir, &[at("fact", "F1", None, "timeout"), at("fact", "F1", Some(0), "ok")]);
        assert_eq!(unverified(&dir, &s, &log), None);
        let log = plant(&dir, &[at("fact", "F1", Some(0), "timeout"), at("fact", "F1", None, "ok")]);
        assert_eq!(unverified(&dir, &s, &log), Some(Unverified { count: 1, mints: vec!["F1".into()] }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tie_at_one_revision_goes_to_the_later_record() {
        // C8 (10). Revert: break the tie toward the earlier record (`<=`
        // for `<`).
        let dir = scratch("tie");
        let s = verbs(settings_fixture(), &["fact"]);
        let log = plant(&dir, &[at("fact", "F1", Some(1), "ok"), at("fact", "F1", Some(1), "timeout")]);
        assert_eq!(unverified(&dir, &s, &log), Some(Unverified { count: 1, mints: vec!["F1".into()] }));
        let log = plant(&dir, &[at("fact", "F1", Some(1), "timeout"), at("fact", "F1", Some(1), "ok")]);
        assert_eq!(unverified(&dir, &s, &log), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

