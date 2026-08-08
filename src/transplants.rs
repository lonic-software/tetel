//! `tetel transplant` — declare that this design installs a mechanism
//! taken from somewhere else, list the premises the donor states for it,
//! and answer each one at the destination.
//!
//! # Why the trigger is a declaration
//!
//! The rule this implements was written as "any design or fix that
//! transplants a mechanism must carry a premise inventory". That has no
//! decidable trigger, for the reason [`crate::targets`] already records:
//! nothing in the record model types anything as a transplant, so "the
//! designs that transplant" denotes no set a check can enumerate.
//! Deciding it from English — or by comparing code across two sites — is
//! the heuristic read of content this project rules out as a refusal
//! trigger and has measured dead once already in [`crate::scope`].
//!
//! So the author declares, the refusal attaches to the declaration, and
//! a design that transplants without declaring is not detected. It is
//! made visible: the rendered section shows its own hole, and `check`
//! prints the blind spot in its standing non-coverage list.
//!
//! # Why a premise is selected, never transcribed
//!
//! The rule asked for the donor's premises "transcribed verbatim". Taken
//! literally that is an honour system: an author types text and promises
//! it is a quotation, in a tool whose entire guarantee is that evidence
//! is captured and cannot be typed. Nothing would stop a premise the
//! donor never wrote, or a paraphrase quietly weaker than the original —
//! and a paraphrased precondition is exactly the failure the inventory
//! exists to catch.
//!
//! So a premise is refused unless its text is a verbatim substring of one
//! observation inside the cited donor fact — [`crate::facts::Fact::quotes`].
//! The author still types; typing is how the selection is *expressed*,
//! not what is trusted. You cannot invent a premise, only select one from
//! what you captured, which means a premise you know about but never
//! captured is fixed by looking again rather than by typing.
//!
//! # Why the refusals do not all live here
//!
//! [`crate::targets`] moved its whole refusal to the verb, because a
//! census either exists when a target is declared or it does not. A
//! premise inventory is different in kind: it is built across time, and
//! the instrument works by transcribe-first. The author selects the
//! donor's premises *before* knowing whether each holds at the
//! destination — that confrontation is the mechanism.
//!
//! Refusing an undischarged premise here would demand the
//! destination-holds claim before the premise could be recorded,
//! inverting the order into answer-first, and pushing an author who
//! cannot yet discharge a premise toward never declaring it. That is the
//! "refusing would make the honest case unwritable to catch the
//! dishonest one" failure [`crate::scope`] names.
//!
//! What this verb enforces is therefore per-record and decidable at
//! creation: the donor exists and can be quoted from, a premise really is
//! the donor's text, the destination is a live modification target, a
//! discharge names a live claim. *Completeness* — every premise answered
//! — is a property of a finished document, and is refused by
//! `render --out`.
//!
//! # Immutable, dischargeable, withdrawable
//!
//! A premise is a selection of immutable bytes, so there is nothing about
//! it to revise honestly: it is withdrawn and reselected. A discharge is
//! append-only and supersedable — a premise whose first destination-holds
//! claim is refuted in grounding is re-discharged by citing another,
//! without losing the record that the first was tried.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::workspace::{self, AuthoringError, Kind};
use crate::{claims, facts, targets};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum TransplantEvent {
    Declare {
        id: String,
        /// The fact whose captured output holds the donor's text. One
        /// fact, for the reason a target cites one: a premise selected
        /// from "one of these several facts" would leave which one
        /// undecided, and the quotation relation needs a single answer.
        from: String,
        /// The modification target this mechanism lands on.
        into: String,
        timestamp: u64,
    },
    Premise {
        id: String,
        transplant: String,
        text: String,
        timestamp: u64,
    },
    Discharge {
        premise: String,
        cites: String,
        timestamp: u64,
    },
    Withdraw {
        id: String,
        why: String,
        timestamp: u64,
    },
}

#[derive(Debug, Clone)]
pub struct Premise {
    pub id: String,
    pub text: String,
    /// The claim asserting this premise holds at the destination — the
    /// latest one cited, since a discharge can be superseded.
    pub discharged_by: Option<String>,
    pub withdrawn: bool,
}

#[derive(Debug, Clone)]
pub struct Transplant {
    pub id: String,
    pub from: String,
    pub into: String,
    pub premises: Vec<Premise>,
    pub withdrawn: bool,
}

impl Transplant {
    pub fn live_premises(&self) -> impl Iterator<Item = &Premise> {
        self.premises.iter().filter(|p| !p.withdrawn)
    }
}

fn log_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("transplants.jsonl")
}

pub fn load_all(workspace_dir: &Path) -> io::Result<Vec<Transplant>> {
    let events: Vec<TransplantEvent> = workspace::read_jsonl(&log_path(workspace_dir))?;
    let mut by_id: std::collections::BTreeMap<String, Transplant> = std::collections::BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for ev in events {
        match ev {
            TransplantEvent::Declare { id, from, into, .. } => {
                order.push(id.clone());
                by_id.insert(id.clone(), Transplant { id, from, into, premises: Vec::new(), withdrawn: false });
            }
            TransplantEvent::Premise { id, transplant, text, .. } => {
                if let Some(t) = by_id.get_mut(&transplant) {
                    t.premises.push(Premise { id, text, discharged_by: None, withdrawn: false });
                }
            }
            TransplantEvent::Discharge { premise, cites, .. } => {
                // Last discharge wins; the earlier ones stay in the log.
                if let Some(p) = by_id.values_mut().flat_map(|t| t.premises.iter_mut()).find(|p| p.id == premise) {
                    p.discharged_by = Some(cites);
                }
            }
            TransplantEvent::Withdraw { id, .. } => {
                if let Some(t) = by_id.get_mut(&id) {
                    t.withdrawn = true;
                } else if let Some(p) = by_id.values_mut().flat_map(|t| t.premises.iter_mut()).find(|p| p.id == id) {
                    p.withdrawn = true;
                }
            }
        }
    }
    Ok(order.into_iter().filter_map(|id| by_id.remove(&id)).collect())
}

/// `tetel transplant --from F1 --into T1`.
pub fn declare(workspace_dir: &Path, from: &str, into: &str) -> Result<Transplant, AuthoringError> {
    let from = from.trim();
    let into = into.trim();
    if from.is_empty() {
        return Err(workspace::refuse(
            workspace_dir,
            "transplant",
            "a transplant must cite the fact that captured the donor (--from was missing); \
             capture the donor site with `tetel look` then `tetel fact`",
        ));
    }
    if into.is_empty() {
        return Err(workspace::refuse(
            workspace_dir,
            "transplant",
            "a transplant must name where the mechanism lands (--into was missing); \
             declare the destination with `tetel target <symbol> --cites F#` first",
        ));
    }

    let all_facts = facts::load_all(workspace_dir)?;
    let Some(fact) = all_facts.iter().find(|f| f.id == from) else {
        return Err(workspace::refuse(
            workspace_dir,
            "transplant",
            format!("cited donor fact does not exist: {from}; try `tetel query facts` to see what exists"),
        ));
    };
    // Refused now rather than at the first premise: a fact whose
    // observation boundaries were never recorded can never be quoted
    // from, so declaring against it would only defer the same refusal to
    // a point where the author has already written the premise out.
    if fact.observation_outputs().is_none() {
        return Err(workspace::refuse(
            workspace_dir,
            "transplant",
            format!(
                "{from} was minted before observation boundaries were recorded, so nothing can be \
                 quoted from it — look at the donor again and mint a new fact"
            ),
        ));
    }

    let all_targets = targets::load_all(workspace_dir)?;
    match all_targets.iter().find(|t| t.id == into) {
        None => {
            return Err(workspace::refuse(
                workspace_dir,
                "transplant",
                format!(
                    "no modification target `{into}` in this workspace. A transplant installs a \
                     mechanism somewhere, and that somewhere is a symbol this design tells an \
                     implementer to modify — declare it with `tetel target <symbol> --cites F#`"
                ),
            ));
        }
        Some(t) if t.withdrawn => {
            return Err(workspace::refuse(
                workspace_dir,
                "transplant",
                format!("modification target {into} is withdrawn; a transplant must land on a live target"),
            ));
        }
        Some(_) => {}
    }

    let id = workspace::next_id(workspace_dir, Kind::Transplant)?;
    let event = TransplantEvent::Declare {
        id: id.clone(),
        from: from.to_string(),
        into: into.to_string(),
        timestamp: workspace::now_unix(),
    };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(Transplant { id, from: from.to_string(), into: into.to_string(), premises: Vec::new(), withdrawn: false })
}

/// `tetel transplant --premise X1 --text "<the donor's own words>"`.
///
/// Refused unless the text is a verbatim substring of one observation
/// inside the transplant's donor fact.
pub fn add_premise(workspace_dir: &Path, transplant: &str, text: &str) -> Result<Premise, AuthoringError> {
    if text.is_empty() {
        return Err(workspace::refuse(workspace_dir, "transplant", "--text is empty; a premise is the donor's own words"));
    }
    let all = load_all(workspace_dir)?;
    let Some(t) = all.iter().find(|t| t.id == transplant) else {
        return Err(workspace::refuse(
            workspace_dir,
            "transplant",
            format!("no transplant `{transplant}` in this workspace"),
        ));
    };
    if t.withdrawn {
        return Err(workspace::refuse(workspace_dir, "transplant", format!("{transplant} is withdrawn")));
    }

    let all_facts = facts::load_all(workspace_dir)?;
    let Some(fact) = all_facts.iter().find(|f| f.id == t.from) else {
        return Err(workspace::refuse(
            workspace_dir,
            "transplant",
            format!("{transplant} cites donor fact {}, which is not in this workspace", t.from),
        ));
    };
    if !fact.quotes(text) {
        return Err(workspace::refuse(
            workspace_dir,
            "transplant",
            format!(
                "that text is not in {}'s captured output, so it is not the donor's words.\n\
                 A premise is selected from what was captured, never typed from memory: copy it \
                 byte for byte — comment markers, indentation and line breaks included — from one \
                 observation in {}.\n\
                 If the donor states it somewhere {} never opened, look there and mint a fact that \
                 covers it. The refusal is about provenance, never about whether the premise is true.",
                t.from, t.from, t.from
            ),
        ));
    }

    // Ordinal over every premise ever declared on this transplant,
    // withdrawn ones included, so an id is never reused.
    let id = format!("{transplant}.{}", t.premises.len() + 1);
    let event = TransplantEvent::Premise {
        id: id.clone(),
        transplant: transplant.to_string(),
        text: text.to_string(),
        timestamp: workspace::now_unix(),
    };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(Premise { id, text: text.to_string(), discharged_by: None, withdrawn: false })
}

/// `tetel transplant --discharge X1.1 --cites C4`.
pub fn discharge(workspace_dir: &Path, premise_id: &str, cites: &str) -> Result<Premise, AuthoringError> {
    let cites = cites.trim();
    if cites.is_empty() {
        return Err(workspace::refuse(
            workspace_dir,
            "transplant",
            "--discharge requires --cites: a premise is answered by a claim that it holds at the destination",
        ));
    }
    let all = load_all(workspace_dir)?;
    let Some(p) = all.iter().flat_map(|t| t.premises.iter()).find(|p| p.id == premise_id) else {
        return Err(workspace::refuse(
            workspace_dir,
            "transplant",
            format!("no premise `{premise_id}` in this workspace"),
        ));
    };
    if p.withdrawn {
        return Err(workspace::refuse(workspace_dir, "transplant", format!("{premise_id} is withdrawn")));
    }

    let all_claims = claims::load_all(workspace_dir)?;
    match all_claims.iter().find(|c| c.id == cites) {
        None => {
            return Err(workspace::refuse(
                workspace_dir,
                "transplant",
                format!("no claim `{cites}` in this workspace; state the destination-holds assertion with `tetel claim` first"),
            ));
        }
        Some(c) if c.withdrawn => {
            return Err(workspace::refuse(workspace_dir, "transplant", format!("claim {cites} is withdrawn")));
        }
        Some(_) => {}
    }

    let event = TransplantEvent::Discharge {
        premise: premise_id.to_string(),
        cites: cites.to_string(),
        timestamp: workspace::now_unix(),
    };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(Premise { id: p.id.clone(), text: p.text.clone(), discharged_by: Some(cites.to_string()), withdrawn: false })
}

/// `tetel transplant --withdraw <X1|X1.1> --why <text>`.
pub fn withdraw(workspace_dir: &Path, id: &str, why: &str) -> Result<(), AuthoringError> {
    if why.trim().is_empty() {
        return Err(workspace::refuse(workspace_dir, "transplant", "--withdraw requires --why"));
    }
    let all = load_all(workspace_dir)?;
    let already = if let Some(t) = all.iter().find(|t| t.id == id) {
        t.withdrawn
    } else if let Some(p) = all.iter().flat_map(|t| t.premises.iter()).find(|p| p.id == id) {
        p.withdrawn
    } else {
        return Err(workspace::refuse(
            workspace_dir,
            "transplant",
            format!("no transplant or premise `{id}` in this workspace"),
        ));
    };
    if already {
        return Err(workspace::refuse(workspace_dir, "transplant", format!("{id} is already withdrawn")));
    }
    let event =
        TransplantEvent::Withdraw { id: id.to_string(), why: why.to_string(), timestamp: workspace::now_unix() };
    workspace::append_jsonl(&log_path(workspace_dir), &event)?;
    Ok(())
}

/// Live premises with no live discharge, as `(transplant, premise, text)`.
///
/// The completeness question, asked in one place so `render --out` and
/// `check` cannot answer it differently. A discharge whose claim has
/// since been withdrawn counts as no discharge: the premise is once again
/// unanswered, which is the state the inventory exists to make visible.
pub fn undischarged(transplants: &[Transplant], claim_list: &[claims::Claim]) -> Vec<(String, String, String)> {
    let live_claim = |id: &str| claim_list.iter().any(|c| c.id == id && !c.withdrawn);
    let mut out = Vec::new();
    for t in transplants.iter().filter(|t| !t.withdrawn) {
        for p in t.live_premises() {
            let answered = p.discharged_by.as_deref().is_some_and(live_claim);
            if !answered {
                out.push((t.id.clone(), p.id.clone(), p.text.clone()));
            }
        }
    }
    out
}

/// The completeness refusal, applied by `render --out` on both front
/// ends — never by a preview render.
///
/// # Why here and not at the verb
///
/// See the module doc comment: an undischarged premise is a legal
/// workspace state, because the author has to be able to write the
/// donor's condition down before knowing the answer to it.
///
/// # Why here and not left to `check`
///
/// `--out` is the act that writes the reviewable artifact and its
/// snapshot, under the pinned policy that a rendered memo is never
/// hand-edited. A memo carrying a premise nobody answered should fail to
/// come into existence rather than exist and fail a check the author may
/// never run — and the remedy is singular, which is this project's own
/// test for refusing rather than reporting: answer the premise, or
/// withdraw it.
///
/// A preview render (no `--out`) still assembles, marking the unanswered
/// rows loudly, so the half-built state stays visible while it is being
/// worked on.
pub fn refuse_incomplete(workspace_dir: &Path) -> Result<(), AuthoringError> {
    let all = load_all(workspace_dir)?;
    let claim_list = claims::load_all(workspace_dir)?;
    let owed = undischarged(&all, &claim_list);
    if owed.is_empty() {
        return Ok(());
    }
    let mut msg = String::from(
        "this document has premises nobody answered at the destination, so it is not written.\n\
         A transplant's premise is a condition the donor states for the mechanism to be sound; \
         carrying one across without answering it is the defect the inventory exists to catch.\n",
    );
    for (t, p, text) in &owed {
        let first = text.lines().next().unwrap_or("").trim();
        let shown = if text.lines().count() > 1 { format!("{first} …") } else { first.to_string() };
        msg.push_str(&format!("\n  {p} (on {t}): {shown}"));
    }
    msg.push_str(
        "\n\nFor each: state whether it holds at the destination with `tetel claim`, then \
         `tetel transplant --discharge <premise> --cites C#`. If it does not apply, \
         `tetel transplant --withdraw <premise> --why \"...\"`.",
    );
    Err(workspace::refuse(workspace_dir, "render", msg))
}

/// What `tetel transplant` was asked to do — the single shape both the
/// CLI and the MCP server build, so the two front ends cannot drift on
/// which act they performed.
pub enum TransplantRequest {
    Declare { from: Option<String>, into: Option<String> },
    Premise { transplant: String, text: Option<String> },
    Discharge { premise: String, cites: Option<String> },
    Withdraw { id: String, why: Option<String> },
}

pub enum TransplantOutcome {
    Declared(Transplant),
    PremiseAdded(Premise),
    Discharged(Premise),
    Withdrawn { id: String },
}

pub fn dispatch(workspace_dir: &Path, req: TransplantRequest) -> Result<TransplantOutcome, AuthoringError> {
    match req {
        TransplantRequest::Declare { from, into } => {
            declare(workspace_dir, &from.unwrap_or_default(), &into.unwrap_or_default())
                .map(TransplantOutcome::Declared)
        }
        TransplantRequest::Premise { transplant, text } => {
            add_premise(workspace_dir, &transplant, &text.unwrap_or_default()).map(TransplantOutcome::PremiseAdded)
        }
        TransplantRequest::Discharge { premise, cites } => {
            discharge(workspace_dir, &premise, &cites.unwrap_or_default()).map(TransplantOutcome::Discharged)
        }
        TransplantRequest::Withdraw { id, why } => withdraw(workspace_dir, &id, &why.unwrap_or_default())
            .map(|()| TransplantOutcome::Withdrawn { id }),
    }
}
