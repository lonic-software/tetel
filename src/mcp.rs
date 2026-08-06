//! `tetel mcp` — an MCP server over stdio, exposing every CLI subcommand
//! (`look`, `run`, `fact`, `claim`, `prose`, `render`, `review`,
//! `query`, `workspaces`, `check`, `brief`, `record`) as a tool. Both halves — authoring and
//! verification — live in this one server: a document `render` just
//! produced is checkable by `check` in the same session, and splitting
//! them across two installs would obscure that connection (see
//! `compose.rs`'s doc comment on fix 1).
//!
//! # Why this exists
//!
//! Shell quoting has corrupted content in three separate runs of this
//! tool and its prototype — most recently, backticks in `--note`/
//! `--proposition` broke inline CLI use on every attempt, forcing all
//! substantial text through `@file` (see `workspace::resolve_text_value`).
//! MCP arguments arrive as JSON values, decoded straight into Rust
//! strings with no shell, and therefore no command-substitution step, in
//! the path at all. That is the property this module exists to protect —
//! not convenience, byte-exact text transport.
//!
//! # Why `rmcp`
//!
//! Built on [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk),
//! the official Rust SDK for the Model Context Protocol: at the time
//! this was written it had ~19M all-time crates.io downloads and a
//! release cut the day before, versus no comparably maintained
//! alternative in the Rust MCP crate landscape. No non-standard protocol
//! was invented here.
//!
//! # `workspace` is required on every authoring tool call — no default
//!
//! The CLI's `--workspace` flag defaults to `"default"`. This server
//! gives every authoring tool (`look`/`run`/`fact`/`claim`/`prose`/
//! `render`/`query`) a *required* `workspace` argument instead, with no
//! fallback value anywhere in this module. Two MCP connections both
//! authoring against a shared default would interleave their `look`/
//! `run` observations in one pending buffer; `facts::mint` folds the
//! *entire* buffer into one fact's extent/output/pin and clears it, and
//! that extent/output/pin is permanently unrevisable once minted (see
//! `facts.rs`) — so one connection's observation would silently become
//! part of another's immutable fact, with nothing anywhere to notice it
//! happened. A required-per-call argument was chosen over deriving a
//! per-connection default because it needs no session/connection
//! identity to lean on at all: there is simply no silent default left to
//! collide on, by construction, rather than a collision made merely
//! unlikely.
//!
//! # Ids are workspace-relative
//!
//! `F1`/`C1`/`P1` name nothing outside the workspace that minted them
//! (see `workspace.rs`'s module doc comment). Every tool description
//! below that returns or accepts one of these ids repeats this — a tool
//! description is what a model re-reads at every invocation, where a
//! one-time brief has already been read once and compacted away.
//!
//! # Refusals are structured
//!
//! Every refusal surfaced from an [`AuthoringError`] comes back as
//! [`CallToolResult::structured_error`]: a JSON object carrying the same
//! guidance text the CLI prints to stderr in a `guidance` field, plus
//! `command`/`workspace` fields — data an agent can act on, not a string
//! it has to pattern-match out of an error message.

use std::path::Path;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{ErrorData, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::workspace::{self, AuthoringError};
use crate::{claims, compose, facts, observe, prose, query};

/// Resolve `name` to a workspace directory, mapping the one failure mode
/// ([`workspace::open`]'s I/O error) to a protocol-level error: creating
/// the workspace's state directory failing is an infrastructure problem
/// the caller can't act on, not a refusal it should see as tool output —
/// see `rmcp::model::CallToolResult::error`'s own doc comment on that
/// distinction.
fn open_workspace(name: &str) -> Result<std::path::PathBuf, ErrorData> {
    workspace::open(name).map_err(|e| ErrorData::internal_error(format!("could not create workspace state: {e}"), None))
}

/// Turn an [`AuthoringError`] into a structured tool-level error result —
/// the one place every authoring tool below converts a refusal, so the
/// shape (`error`/`command`/`workspace`/`guidance`) can never drift
/// between tools. `guidance` is `err.to_string()`, byte-identical to what
/// the CLI writes after its `tetel: ` prefix.
fn refusal(command: &str, workspace_name: &str, err: AuthoringError) -> CallToolResult {
    let kind = match &err {
        AuthoringError::Refused(_) => "refused",
        AuthoringError::Io(_) => "io",
    };
    CallToolResult::structured_error(json!({
        "error": kind,
        "command": command,
        "workspace": workspace_name,
        "guidance": err.to_string(),
    }))
}

fn text_result(s: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(s.into())]))
}

/// The result of a successful `fact` mint or revision, carrying any
/// note-vs-extent findings back to whoever wrote the note.
///
/// This is the authoring surface agents actually use, so a finding that
/// only reached `check` would only ever reach the human reviewing the
/// finished memo — after the note, the claim resting on it, and the
/// prose are all written. The author is the one who can still cheaply
/// tell context from conclusion, and they get it here.
///
/// Carried as `attention`, a top-level array, rather than folded into a
/// prose sentence: the caller is a program, and a field it can branch on
/// beats a string it has to notice. `advice` is shared verbatim with the
/// CLI so the two surfaces cannot drift into saying different things
/// about the same finding.
fn fact_result(dir: &Path, id: &str, action: &str) -> serde_json::Value {
    let attention: Vec<serde_json::Value> = crate::scope::for_fact(dir, id)
        .iter()
        .map(|o| {
            json!({
                "kind": "note-outside-extent",
                "mentioned": o.mentioned,
                "extent": o.extent_labels,
                "guidance": crate::scope::advice(o),
            })
        })
        .collect();
    json!({ "id": id, "action": action, "attention": attention })
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LineRange {
    /// 1-based inclusive start line.
    start: usize,
    /// 1-based inclusive end line.
    end: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LookParams {
    /// The authoring workspace this observation is recorded into.
    workspace: String,
    /// The file to open (plain mode), or the file/directory to search
    /// when `grep` is given.
    path: String,
    /// Restrict the open to this 1-based inclusive line range. Only
    /// valid without `grep`.
    #[serde(default)]
    lines: Option<LineRange>,
    /// Search `path` for this pattern instead of opening it.
    #[serde(default)]
    grep: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RunParams {
    /// The authoring workspace this observation is recorded into.
    workspace: String,
    /// The command and its arguments — executed directly, never through
    /// a shell. `command[0]` is the program name.
    command: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FactParams {
    /// The authoring workspace this fact belongs to.
    workspace: String,
    /// The fact's note. Plain text — no shell involved, so backticks,
    /// quotes and embedded newlines all pass through byte-exact.
    /// Required to mint a new fact; with `revise`, this is the fact's
    /// new note.
    #[serde(default)]
    note: Option<String>,
    /// Revise this existing fact's note instead of minting a new one.
    /// Extent, output and pin were set once at mint time and are never
    /// revised — only the note can change.
    #[serde(default)]
    revise: Option<String>,
    /// Required with `revise`: why the note is changing.
    #[serde(default)]
    why: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ClaimParams {
    /// The authoring workspace this claim belongs to.
    workspace: String,
    /// The claim's proposition. Plain text — no shell involved, so
    /// backticks, quotes and embedded newlines all pass through
    /// byte-exact. Required to create a new claim; with `revise`, an
    /// omitted `proposition` leaves the proposition unchanged.
    #[serde(default)]
    proposition: Option<String>,
    /// Comma-separated fact ids the claim rests on (e.g. `"F1,F3"`). The
    /// same field `prose` takes, because it is the same relation — this
    /// rests on that — and `render` prints it as `*cites: …*`. Required
    /// to create a new claim; with `revise`, an omitted `cites` leaves
    /// the citations unchanged.
    #[serde(default)]
    cites: Option<String>,
    /// Revise this existing claim instead of creating a new one.
    #[serde(default)]
    revise: Option<String>,
    /// Withdraw this existing claim instead of creating a new one.
    #[serde(default)]
    withdraw: Option<String>,
    /// Required with `revise`/`withdraw`: why.
    #[serde(default)]
    why: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ProseParams {
    /// The authoring workspace this prose block belongs to.
    workspace: String,
    /// The block's text. Plain text — no shell involved, so backticks,
    /// quotes and embedded newlines all pass through byte-exact.
    text: String,
    /// Mint a heading instead of a paragraph, at this markdown depth
    /// (1..=6).
    #[serde(default)]
    heading_level: Option<u8>,
    /// Comma-separated claim ids this paragraph cites (e.g. `"C1,C4"`).
    /// The same field `claim` takes, because it is the same relation —
    /// this rests on that — and `render` prints it as `*cites: …*`.
    /// Ignored for a heading.
    #[serde(default)]
    cites: Option<String>,
    /// Revise this existing block's text instead of creating a new one.
    #[serde(default)]
    revise: Option<String>,
    /// Required with `revise`: why.
    #[serde(default)]
    why: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RenderParams {
    /// The authoring workspace to assemble into markdown.
    workspace: String,
    /// Write the document to this path, and the workspace snapshot its
    /// citations point into to `<path>.tetel/`, in one act. Omit to get
    /// the markdown back as text without writing anything.
    ///
    /// Use this for a document you intend to keep: the citation ids in a
    /// rendered document are workspace-relative, so a document saved
    /// without its snapshot cites evidence nobody else can resolve.
    #[serde(default)]
    out: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum QueryWhat {
    Facts,
    Claims,
    Prose,
    Deps,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct QueryParams {
    /// The authoring workspace to inspect.
    workspace: String,
    /// Which read-only view to return.
    what: QueryWhat,
    /// Required when `what` is `"deps"`: the fact or claim id to look up
    /// (must start with `F` or `C`).
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CheckParams {
    /// Path to the markdown memo to check.
    file: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BriefParams {
    /// Path to the memo to brief. Omit only when `authoring` is true.
    #[serde(default)]
    memo: Option<String>,
    /// Emit machine-readable JSON instead of the human-readable form.
    /// Ignored when `authoring` is true.
    #[serde(default)]
    json: bool,
    /// Emit the authoring rhythm brief instead of a grounding brief for
    /// a memo — the exact text handed to whoever is about to write a
    /// document with `look`/`run`/`fact`/`claim`/`prose`/`render`. Takes
    /// no memo.
    #[serde(default)]
    authoring: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecordParams {
    /// Path to the memo the claim id must be defined in.
    memo: String,
    /// One grounding result, shaped as `evidence::RecordInput`: `claim`,
    /// `pass`, `verdict` (`supports`|`refutes`|`qualifies`),
    /// `reported_kind` (`run`|`reading`|`observed`|`attested`), `source`
    /// (a file path, or `proc:<session-or-agent>`), and optional
    /// `extent`/`note`/`pin`. Given as a JSON object directly — MCP is
    /// JSON end to end, so there is no reason to make this a
    /// JSON-encoded string a caller has to escape into.
    ///
    /// This is the **ingested** path: `extent` and `source` are typed by
    /// you, so the tool witnessed the report and not the act, and the
    /// record caps at attested standing. Prefer `from_fact` below for
    /// anything you observed yourself through this server.
    #[serde(default)]
    input: Option<serde_json::Value>,
    /// Ground `claim` on a fact **this workspace captured** — the
    /// witnessed path. The extent is copied from the fact, where
    /// `look`/`run` captured it, and there is no field here by which you
    /// could supply one; that absence is what separates this from
    /// `input`. The record carries the workspace's identity, so `check`
    /// can recompute whether a grounding pass rested on its own
    /// observations or inherited someone else's.
    ///
    /// Requires `workspace`, `claim` and `verdict`.
    #[serde(default)]
    from_fact: Option<String>,
    /// The workspace whose fact is being cited. Required with `from_fact`.
    #[serde(default)]
    workspace: Option<String>,
    /// Which claim is being grounded. Required with `from_fact`.
    #[serde(default)]
    claim: Option<String>,
    /// `supports` | `refutes` | `qualifies`. Required with `from_fact`.
    ///
    /// `qualifies` means the proposition holds only under a condition it
    /// does not state, or that you could not establish it from what you
    /// were given — and it requires `note` saying which. Where you cannot
    /// establish something, that is the correct answer, not a charitable
    /// `supports`.
    #[serde(default)]
    verdict: Option<String>,
    /// Why, in your words. Required when `verdict` is `qualifies`.
    #[serde(default)]
    note: Option<String>,
}

/// The MCP server: authoring (`look`/`run`/`fact`/`claim`/`prose`/
/// `render`/`query`) and verification (`check`/`brief`/`record`) in one
/// process. Holds no state of its own — every tool resolves its
/// workspace directory (or memo path) fresh from its own arguments, the
/// same way each separate CLI invocation does.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // `tool_router` is read by the `#[tool_handler]`-generated dispatch, invisible to dead-code analysis (mirrors rmcp's own test pattern)
pub struct TetelServer {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl TetelServer {
    pub fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }

    #[tool(description = "Open a path into the pending observation buffer, or search it with `grep` — the evidence a `fact` is later minted from. `workspace` is required (never defaulted); ids elsewhere are workspace-relative only.")]
    async fn look(&self, Parameters(p): Parameters<LookParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        let req = if let Some(pattern) = p.grep {
            observe::LookRequest::Grep { pattern, root: Some(p.path) }
        } else {
            observe::LookRequest::Open { path: Some(p.path), lines: p.lines.map(|l| (l.start, l.end)) }
        };
        match observe::dispatch(&dir, req) {
            Ok(outcome) => text_result(outcome.printed),
            Err(e) => Ok(refusal("look", &p.workspace, e)),
        }
    }

    #[tool(description = "Execute a command directly (no shell) and record its combined stdout/stderr into the pending buffer. `workspace` is required (never defaulted); ids elsewhere are workspace-relative only.")]
    async fn run(&self, Parameters(p): Parameters<RunParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        match observe::run_command(&dir, &p.command) {
            Ok(outcome) => Ok(CallToolResult::structured(json!({
                "exit_code": outcome.exit_code,
                "output": outcome.printed,
            }))),
            Err(e) => Ok(refusal("run", &p.workspace, e)),
        }
    }

    #[tool(description = "Mint a fact from the pending buffer (refuses on an empty buffer — run `look`/`run` first), or `revise` an existing fact's note (extent/output/pin were set once at mint time and are never revised). Check the `attention` array in the result: a non-empty entry means your note names a location this fact's extent does not cover — read that location and mint a fact for it, or revise the note, rather than leaving a conclusion about code you did not open. `workspace` is required (never defaulted); minted ids (F#) are workspace-relative only.")]
    async fn fact(&self, Parameters(p): Parameters<FactParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        let req = match p.revise {
            Some(id) => facts::FactRequest::Revise { id, note: p.note, why: p.why },
            None => facts::FactRequest::Mint { note: p.note },
        };
        match facts::dispatch(&dir, req) {
            Ok(facts::FactOutcome::Minted(f)) => {
                Ok(CallToolResult::structured(fact_result(&dir, &f.id, "minted")))
            }
            Ok(facts::FactOutcome::Revised { id }) => {
                Ok(CallToolResult::structured(fact_result(&dir, &id, "revised")))
            }
            Err(e) => Ok(refusal("fact", &p.workspace, e)),
        }
    }

    #[tool(description = "Assert a claim resting on one or more fact ids, or `revise`/`withdraw` an existing one. Expect to `revise` a claim when writing its prose exposes it as imprecise or needing a qualification — that's the normal rhythm, not a mistake. `workspace` is required (never defaulted); ids (C#) are workspace-relative only.")]
    async fn claim(&self, Parameters(p): Parameters<ClaimParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        let req = if let Some(id) = p.withdraw {
            claims::ClaimRequest::Withdraw { id, why: p.why }
        } else if let Some(id) = p.revise {
            claims::ClaimRequest::Revise { id, prop: p.proposition, from: p.cites, why: p.why }
        } else {
            claims::ClaimRequest::Create { prop: p.proposition, from: p.cites }
        };
        match claims::dispatch(&dir, req) {
            Ok(claims::ClaimOutcome::Created(outcome)) => {
                let overlap: Vec<_> =
                    outcome.overlap.iter().map(|(id, note)| json!({"id": id, "note": note})).collect();
                Ok(CallToolResult::structured(json!({
                    "id": outcome.claim.id,
                    "action": "created",
                    "overlap": overlap,
                })))
            }
            Ok(claims::ClaimOutcome::Revised { id }) => Ok(CallToolResult::structured(json!({"id": id, "action": "revised"}))),
            Ok(claims::ClaimOutcome::Withdrawn { id }) => {
                Ok(CallToolResult::structured(json!({"id": id, "action": "withdrawn"})))
            }
            Err(e) => Ok(refusal("claim", &p.workspace, e)),
        }
    }

    #[tool(description = "Append a paragraph or heading to the document's prose, or `revise` an existing block. Write this as soon as a claim exists to say something about — don't defer prose to a writing phase at the end. `workspace` is required (never defaulted); ids (P#) are workspace-relative only.")]
    async fn prose(&self, Parameters(p): Parameters<ProseParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        let req = if let Some(id) = p.revise {
            prose::ProseRequest::Revise { id, text: p.text, why: p.why }
        } else if let Some(level) = p.heading_level {
            prose::ProseRequest::Heading { text: p.text, level: Some(level) }
        } else {
            prose::ProseRequest::Paragraph { text: p.text, cite: p.cites }
        };
        match prose::dispatch(&dir, req) {
            Ok(prose::ProseOutcome::Created(b)) => Ok(CallToolResult::structured(json!({"id": b.id, "action": "appended"}))),
            Ok(prose::ProseOutcome::Revised { id }) => Ok(CallToolResult::structured(json!({"id": id, "action": "revised"}))),
            Err(e) => Ok(refusal("prose", &p.workspace, e)),
        }
    }

    #[tool(description = "Assemble the workspace's current prose into the finished markdown document, plus a checkable evidence ledger. `workspace` is required (never defaulted).")]
    async fn render(&self, Parameters(p): Parameters<RenderParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        let rendered = match compose::render(&dir) {
            Ok(r) => r,
            Err(e) => {
                return Err(ErrorData::internal_error(format!("error rendering: {e}"), None))
            }
        };
        let Some(out) = p.out else {
            return text_result(rendered);
        };

        // Same ordering as the CLI: document first, then snapshot, so a
        // failed snapshot leaves a recoverable state rather than a record
        // describing a document that was never written.
        let path = Path::new(&out);
        if let Err(e) = std::fs::write(path, &rendered) {
            return Err(ErrorData::internal_error(
                format!("could not write {out}: {e}"),
                None,
            ));
        }
        if let Err(e) = crate::snapshot::write(path, &dir) {
            return Err(ErrorData::internal_error(
                format!("wrote {out} but could not write its snapshot: {e}"),
                None,
            ));
        }
        let pending = crate::snapshot::pending_count(&dir);
        let warning = if pending > 0 {
            format!(
                "\nwarning: {pending} observation(s) still pending, never minted into a fact — \
they are in the snapshot but nothing in the document rests on them"
            )
        } else {
            String::new()
        };
        text_result(format!(
            "{out} written, snapshot in {}{warning}",
            crate::snapshot::snapshot_path(path).display()
        ))
    }

    #[tool(description = "Plain, greppable, read-only inspection of facts, claims, prose, or an id's dependencies. Never refuses. `workspace` is required (never defaulted); ids are workspace-relative only.")]
    async fn query(&self, Parameters(p): Parameters<QueryParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        let out = match p.what {
            QueryWhat::Facts => query::facts_text(&dir),
            QueryWhat::Claims => query::claims_text(&dir),
            QueryWhat::Prose => query::prose_text(&dir),
            QueryWhat::Deps => {
                let Some(id) = p.id else {
                    return Err(ErrorData::invalid_params("query `deps` requires `id`", None));
                };
                query::deps_text(&dir, &id)
            }
        };
        match out {
            Ok(s) => text_result(s),
            Err(e) => Err(ErrorData::internal_error(format!("error querying: {e}"), None)),
        }
    }

    #[tool(description = "Every paragraph beside the claims it cites, assembled for reading. Use this before `render --out`: read each paragraph against its propositions and ask whether the paragraph says what its claims say, no more. A paragraph asserting something none of its claims carries is the failure this is for — nothing detects it mechanically, and seeing the two together will. `workspace` is required (never defaulted).")]
    async fn review(&self, Parameters(p): Parameters<RenderParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        match crate::review::render(&dir) {
            Ok(out) => text_result(out),
            Err(e) => Err(ErrorData::internal_error(format!("error building review: {e}"), None)),
        }
    }

    #[tool(description = "List every authoring workspace on this machine with its fact/claim/prose counts. Takes no `workspace` — this is the one question that cannot be answered from inside one. Read-only; never creates anything.")]
    async fn workspaces(&self) -> Result<CallToolResult, ErrorData> {
        match crate::workspace::list() {
            Ok(list) if list.is_empty() => text_result(format!(
                "no workspaces yet under {}",
                crate::workspace::state_home().join("workspaces").display()
            )),
            Ok(list) => text_result(
                list.into_iter()
                    .map(|w| {
                        format!(
                            "{}\t{} facts\t{} claims\t{} prose",
                            w.name, w.facts, w.claims, w.prose
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Err(e) => Err(ErrorData::internal_error(
                format!("could not list workspaces: {e}"),
                None,
            )),
        }
    }

    #[tool(description = "Check a markdown memo's `tetel` evidence rows and evidence ledger. Never writes any file, executes any command from the document, or makes a network call.")]
    async fn check(&self, Parameters(p): Parameters<CheckParams>) -> Result<CallToolResult, ErrorData> {
        match crate::check_file(Path::new(&p.file)) {
            Ok((code, report)) => {
                let block = vec![ContentBlock::text(report)];
                Ok(if code == crate::EXIT_CLEAN { CallToolResult::success(block) } else { CallToolResult::error(block) })
            }
            Err(e) => Err(ErrorData::internal_error(format!("error reading {}: {e}", p.file), None)),
        }
    }

    // NOTE: `authoring: true` returns `brief::AUTHORING_BRIEF` byte-for-byte
    // unchanged — that text is the tested artifact (a matched-pair
    // experiment established it produces interleaved composition where
    // its absence produces transcription; see brief.rs). This tool's own
    // `description` string below is a *separate*, additional channel —
    // it must never be merged into or substituted for AUTHORING_BRIEF, so
    // a future run can still tell brief-driven behavior apart from
    // description-driven behavior.
    #[tool(description = "Emit the grounding brief for a memo's evidence ledger (id + proposition only, scope withheld), or with `authoring: true`, the authoring rhythm brief for whoever is about to write a document with look/run/fact/claim/prose/render.")]
    async fn brief(&self, Parameters(p): Parameters<BriefParams>) -> Result<CallToolResult, ErrorData> {
        if p.authoring {
            return text_result(crate::brief::AUTHORING_BRIEF);
        }
        let Some(memo) = p.memo else {
            return Ok(CallToolResult::structured_error(json!({
                "error": "refused",
                "command": "brief",
                "guidance": "tetel: `brief` requires a memo, or `authoring: true`",
            })));
        };
        match crate::brief_file(Path::new(&memo), p.json) {
            Ok((code, out)) => {
                let block = vec![ContentBlock::text(out)];
                Ok(if code == crate::EXIT_CLEAN { CallToolResult::success(block) } else { CallToolResult::error(block) })
            }
            Err(e) => Err(ErrorData::internal_error(format!("error reading {memo}: {e}"), None)),
        }
    }

    #[tool(description = "Append one grounding result to the memo's evidence log. Two paths: `from_fact` (witnessed — the extent is copied from a fact this workspace captured and cannot be typed, and the record carries the workspace identity so `check` can recompute whether a pass grounded its own observations) or `input` (ingested — extent and source typed by you, capped at attested standing). Prefer `from_fact` for anything you observed through this server. Refuses an unknown claim id, an invalid verdict, a `qualifies` with no note, or malformed input, and never performs a partial write.")]
    async fn record(&self, Parameters(p): Parameters<RecordParams>) -> Result<CallToolResult, ErrorData> {
        let refused = |e: crate::evidence::RecordError| {
            Ok(CallToolResult::structured_error(json!({
                "error": "refused",
                "command": "record",
                "guidance": e.to_string(),
            })))
        };

        if let Some(fact_id) = p.from_fact {
            let (Some(ws), Some(claim), Some(verdict_raw)) = (p.workspace, p.claim, p.verdict)
            else {
                return Ok(CallToolResult::structured_error(json!({
                    "error": "refused",
                    "command": "record",
                    "guidance": "`from_fact` needs `workspace`, `claim` and `verdict`",
                })));
            };
            let Some(verdict) = crate::evidence::Verdict::parse(verdict_raw.trim()) else {
                return Ok(CallToolResult::structured_error(json!({
                    "error": "refused",
                    "command": "record",
                    "guidance": format!(
                        "invalid `verdict` {verdict_raw:?}; expected supports, refutes or qualifies"
                    ),
                })));
            };
            let dir = open_workspace(&ws)?;
            return match crate::record_from_fact_file(
                Path::new(&p.memo),
                &dir,
                &claim,
                verdict,
                &fact_id,
                p.note,
            ) {
                Ok(Ok(identity)) => Ok(CallToolResult::structured(json!({
                    "recorded": true,
                    "witnessed": true,
                    "claim": claim,
                    "from_fact": fact_id,
                    "pass": identity,
                }))),
                Ok(Err(e)) => refused(e),
                Err(e) => {
                    Err(ErrorData::internal_error(format!("error reading {}: {e}", p.memo), None))
                }
            };
        }

        let Some(input) = p.input else {
            return Ok(CallToolResult::structured_error(json!({
                "error": "refused",
                "command": "record",
                "guidance": "give either `from_fact` (witnessed: extent copied from a fact this \
workspace captured) or `input` (ingested: extent typed by you)",
            })));
        };
        match crate::record_file(Path::new(&p.memo), &input.to_string()) {
            Ok(Ok(())) => Ok(CallToolResult::structured(json!({
                "recorded": true,
                "witnessed": false,
            }))),
            Ok(Err(e)) => refused(e),
            Err(e) => Err(ErrorData::internal_error(format!("error reading {}: {e}", p.memo), None)),
        }
    }
}

// `name`/`version` are set via a manual `get_info` below (not the
// `tool_handler(name = ..., version = ...)` attribute form) because
// those attributes take string literals only, and the version should
// track `Cargo.toml` via `env!("CARGO_PKG_VERSION")` rather than be
// hand-duplicated and left to drift.
#[tool_handler]
impl ServerHandler for TetelServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tetel", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Authoring (look/run/fact/claim/prose/render/query) and verification \
                 (check/brief/record) in one server, since a rendered memo is checkable by the \
                 same tool that wrote it. Every authoring tool requires an explicit `workspace` \
                 argument — there is no shared default, and ids it returns are workspace-relative \
                 only. See each tool's own description for details; those persist across calls \
                 where this one-time text may not.",
            )
    }
}

/// Serve `TetelServer` over stdio until the peer disconnects.
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = TetelServer::new();
    let transport = stdio();
    server.serve(transport).await?.waiting().await?;
    Ok(())
}
