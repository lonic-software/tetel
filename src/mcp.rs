//! `tetel mcp` — an MCP server over stdio, exposing every CLI subcommand
//! (`look`, `run`, `fact`, `claim`, `target`, `prose`, `render`, `review`,
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
//! # Filesystem paths must be absolute
//!
//! Every path a tool takes (`look`'s `path`, `render`'s `out`,
//! `check`/`brief`/`record`'s memo) is resolved against **this server's**
//! working directory, which is wherever the client happened to start it —
//! not the caller's, and not anything the caller can see or set. A
//! relative path therefore resolves somewhere the caller cannot predict.
//!
//! This has already cost a run: an agent passed relative paths to
//! `check` and `brief`, the server resolved them against its own
//! directory, and the tools reported "no tetel rows found" and "no
//! evidence ledger found" — accurate about what they read, and useless
//! for working out why. Every path-taking parameter now says to pass an
//! absolute path, and every message that names a path names the resolved
//! absolute one, so a wrong directory is visible in the answer instead of
//! having to be deduced.
//!
//! # Rebuilding requires restarting the client
//!
//! `cargo install` replaces the binary by rename, so a server process
//! already running keeps the file it opened: **reinstalling does not
//! reach it, and neither does reloading plugins — only restarting the
//! client does.** Every agent that reaches tetel through this server is
//! otherwise grading with the old build and cannot tell.
//!
//! That step is no longer only documentation. A server whose binary has
//! been replaced underneath it refuses every tool call with the reason
//! and the remedy (see `call_tool` below and `buildid.rs`), and `check`
//! names the build that graded it on its last line, so two disagreeing
//! verdicts can be attributed rather than argued about.
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
use crate::{claims, compose, facts, observe, prose, query, targets, transplants};

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

/// A caller-supplied path, resolved to absolute for every message that
/// names it.
///
/// The server cannot know the caller's working directory, so a relative
/// path silently means something different to each side. Nothing here
/// changes *which* file is opened — that was always resolved against this
/// process's directory — but it makes the resolution visible in the
/// answer, so "no tetel rows found in /somewhere/unexpected/memo.md"
/// diagnoses itself instead of reading as a fact about the document.
fn resolved(path: &str) -> std::path::PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf())
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
fn fact_result(
    dir: &Path,
    id: &str,
    action: &str,
    folded: Vec<String>,
    refused: Vec<String>,
) -> serde_json::Value {
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
    // `refused` answers a different question from `folded`: not what this
    // mint took, but what the author tried and could not do in the window
    // that produced it. A caller that folded one leftover observation
    // after two refused `look` calls can see both here, which is the
    // whole of the incident this was built for.
    json!({
        "id": id,
        "action": action,
        "attention": attention,
        "folded": folded,
        "refused_since_previous_fact": refused,
    })
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
    /// when `grep` is given. **Pass an absolute path** — see this
    /// module's doc comment on why a relative one resolves somewhere you
    /// cannot predict.
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
struct TargetParams {
    /// The authoring workspace this target belongs to.
    workspace: String,
    /// The symbol this design tells an implementer to modify. Compared
    /// byte-for-byte against the pattern of the census behind it, so it
    /// must be the symbol itself and not a longer phrase containing it.
    #[serde(default)]
    symbol: Option<String>,
    /// The fact whose captured extent censuses the symbol: a `look`
    /// with `grep` set to exactly this symbol and `path` set to the
    /// worktree root. One fact id, not a list.
    #[serde(default)]
    cites: Option<String>,
    /// Withdraw this existing target instead of declaring one. There is
    /// no `revise`: a changed symbol is a different census.
    #[serde(default)]
    withdraw: Option<String>,
    /// Required with `withdraw`: why.
    #[serde(default)]
    why: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TransplantParams {
    /// The authoring workspace this transplant belongs to.
    workspace: String,
    /// The fact that captured the donor site — the code this design is
    /// taking a mechanism from. One fact id, not a list: a premise is
    /// quoted from it, and that needs a single answer.
    #[serde(default)]
    from: Option<String>,
    /// The modification target (T#) this mechanism lands on. Must already
    /// be declared with `target`, which is what forces a census at the
    /// landing site.
    #[serde(default)]
    into: Option<String>,
    /// Select a premise from this transplant's (X#) donor fact. Pair with
    /// `text`.
    #[serde(default)]
    premise: Option<String>,
    /// The donor's own words, byte for byte — comment markers,
    /// indentation and line breaks included. REFUSED unless it is a
    /// verbatim substring of one observation in the donor fact, so copy
    /// it from that fact's captured output rather than retyping it.
    #[serde(default)]
    text: Option<String>,
    /// Answer this premise (X#.#) with the claim asserting it holds at
    /// the destination. Pair with `cites`.
    #[serde(default)]
    discharge: Option<String>,
    /// The claim that answers the premise.
    #[serde(default)]
    cites: Option<String>,
    /// Withdraw a transplant (X#) or a single premise (X#.#). There is no
    /// `revise`: a premise is a selection of immutable bytes.
    #[serde(default)]
    withdraw: Option<String>,
    /// Required with `withdraw`: why.
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
    /// Insert this new block immediately before an existing one, instead
    /// of appending. Document order is otherwise authoring order, which
    /// means writing prose as discoveries happen — what the brief asks —
    /// produces a document in discovery order. Without this, the only way
    /// to get a well-ordered document is to defer all prose to the end,
    /// which is the pattern the brief exists to prevent.
    #[serde(default)]
    before: Option<String>,
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
    /// Write the document to this path (**absolute** — a relative path
    /// resolves against this server's working directory, not yours), and
    /// the workspace snapshot its citations point into to
    /// `<path>.tetel/`, in one act. Omit to get
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

/// `review` takes a workspace and nothing else.
///
/// It used to borrow [`RenderParams`], which meant its published schema
/// advertised an `out` parameter the handler silently ignored — a caller
/// could reasonably ask `review` to write a file and get no file and no
/// error. A tool's parameters are a promise; sharing a struct for
/// convenience made this one lie.
#[derive(Debug, Deserialize, JsonSchema)]
struct ReviewParams {
    /// The authoring workspace whose prose and claims to pair up.
    workspace: String,
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
    /// Path to the markdown memo to check. **Absolute** — a relative
     /// path resolves against this server's working directory, not
     /// yours.
    file: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BriefParams {
    /// Path to the memo to brief, **absolute** — a relative path
    /// resolves against this server's working directory, not yours. Omit
    /// only when `authoring` is true.
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

/// The published shape of `record`'s `input`.
///
/// `input` deserialises as a bare [`serde_json::Value`] so that both an
/// object and a JSON-encoded string are accepted — but `Value`'s own
/// generated schema carries **no `type` at all**, just a description. A
/// client with nothing telling it this is an object reasonably
/// serialises one into a string, and then `record` refuses with
/// "invalid type: string, expected struct RecordInput". That happened
/// three times to one agent before this existed.
///
/// So the runtime type stays permissive and the *schema* is declared
/// here, field by field, mirroring [`crate::evidence::RecordInput`]. A
/// caller now sees what to send instead of guessing from prose.
#[derive(Debug, Deserialize, JsonSchema)]
struct IngestedRecord {
    /// The claim id this result grades — must exist in the memo's ledger.
    claim: String,
    /// Which grounding pass this is. Free text on this path, and
    /// validated only for being non-empty: nothing can check it. Use
    /// `from_fact` instead if you want independence to be derivable.
    pass: String,
    /// `supports` | `refutes` | `qualifies`. A `qualifies` requires
    /// `note`.
    verdict: String,
    /// What kind of act you say this was: `run` | `reading` | `observed`
    /// | `attested`. Recorded verbatim and never consulted when standing
    /// is derived — the tool witnessed the saying, not the act, so every
    /// ingested record caps at attested regardless of what this says.
    reported_kind: String,
    /// Where the act is recorded: a file path, or
    /// `proc:<session-or-agent>` for a transcript-only act. Required — a
    /// record naming nothing preserved anywhere is refused.
    source: String,
    /// What the act examined. Typed by you on this path, which is exactly
    /// what marks the record as reported rather than witnessed.
    #[serde(default)]
    extent: Vec<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    pin: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecordParams {
    /// Path to the memo the claim id must be defined in. **Absolute** —
    /// a relative path resolves against this server's working directory,
    /// not yours.
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
    #[schemars(with = "Option<IngestedRecord>")]
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
        // Refused after the workspace is open, not before, so it reaches
        // `workspace::refuse` and lands in the shipped record. The CLI
        // used to refuse this in clap and this handler refused it here;
        // both were pre-choke-point, so the same refusal was invisible on
        // both surfaces. Now both take this path, with one shared reason
        // text.
        if p.lines.is_some() && p.grep.is_some() {
            return Ok(refusal(
                "look",
                &p.workspace,
                crate::workspace::refuse(&dir, "look", crate::workspace::LINES_WITH_GREP),
            ));
        }
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

    #[tool(description = "Execute a command directly (no shell) and record its combined stdout/stderr into the pending buffer. The whole output is captured verbatim and, once minted into a fact, becomes permanently unrevisable and ships into the snapshot beside the memo — so a command whose output is enormous, or contains a credential, puts that in a repository forever. A command exiting 0 also establishes nothing by exiting 0: read the output and ask whether it states your proposition or merely fails to deny it. `workspace` is required (never defaulted); ids elsewhere are workspace-relative only.")]
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

    #[tool(description = "Mint a fact from the pending buffer (refuses on an empty buffer — run `look`/`run` first), or `revise` an existing fact's note (extent/output/pin were set once at mint time and are never revised). Check the `attention` array in the result: a non-empty entry means your note names a location this fact's extent does not cover — read that location and mint a fact for it, or revise the note, rather than leaving a conclusion about code you did not open. The result also carries `folded` (what this mint took from the pending buffer, with ages) and `refused_since_previous_fact` (what was refused in the window that produced it, verbatim) — a refused `look` leaves the buffer untouched, so if you expected to fold a file and see a refusal instead, that is where it went. `workspace` is required (never defaulted); minted ids (F#) are workspace-relative only.")]
    async fn fact(&self, Parameters(p): Parameters<FactParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        let req = match p.revise {
            Some(id) => facts::FactRequest::Revise { id, note: p.note, why: p.why },
            None => facts::FactRequest::Mint { note: p.note },
        };
        // Both described before dispatch: a successful mint clears the
        // buffer and becomes the new window boundary.
        let folded = crate::pending::load(&dir)
            .map(|b| facts::describe_buffer(&b, workspace::now_unix()))
            .unwrap_or_default();
        let refused = facts::refusals_since_last_mint(&dir);
        match facts::dispatch(&dir, req) {
            Ok(facts::FactOutcome::Minted(f)) => {
                Ok(CallToolResult::structured(fact_result(&dir, &f.id, "minted", folded, refused)))
            }
            Ok(facts::FactOutcome::Revised { id }) => Ok(CallToolResult::structured(fact_result(
                &dir,
                &id,
                "revised",
                Vec::new(),
                Vec::new(),
            ))),
            Err(e) => Ok(refusal("fact", &p.workspace, e)),
        }
    }

    #[tool(description = "Assert a claim resting on one or more fact ids, or `revise`/`withdraw` an existing one. Expect to `revise` a claim when writing its prose exposes it as imprecise or needing a qualification — that's the normal rhythm, not a mistake. Creating a claim prints an OVERLAP REPORT: other facts whose extent touches the same file or command as the facts you cited, and which you did NOT cite. It is not an error — read it and decide whether one of them belongs in this claim, or whether citing only some of what you looked at is deliberate. `workspace` is required (never defaulted); ids (C#) are workspace-relative only.")]
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

    #[tool(description = "Declare a symbol this design tells an implementer to modify, citing the fact that censuses it. REFUSES unless that fact's captured extent contains a search of the WHOLE WORKTREE for exactly this symbol \u{2014} so capture it with `look` using `grep: \"<symbol>\"` and `path: \"<worktree root>\"`, then `fact`, then cite that fact here. The refusal is about the search's existence and where it was rooted, never about what it found; a symbol with no occurrences is censused by the search establishing it has none. Declaring is not required by anything \u{2014} nothing can detect a recommendation you did not declare \u{2014} so an undeclared target is invisible, and the rendered section says so. `workspace` is required (never defaulted); ids (T#) are workspace-relative only.")]
    async fn target(&self, Parameters(p): Parameters<TargetParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        let req = if let Some(id) = p.withdraw {
            targets::TargetRequest::Withdraw { id, why: p.why }
        } else {
            targets::TargetRequest::Declare { symbol: p.symbol, from: p.cites }
        };
        match targets::dispatch(&dir, req) {
            Ok(targets::TargetOutcome::Declared(t)) => Ok(CallToolResult::structured(json!({
                "id": t.id,
                "action": "declared",
                "symbol": t.symbol,
                "censused_by": t.from,
            }))),
            Ok(targets::TargetOutcome::Withdrawn { id }) => {
                Ok(CallToolResult::structured(json!({"id": id, "action": "withdrawn"})))
            }
            Err(e) => Ok(refusal("target", &p.workspace, e)),
        }
    }

    #[tool(description = "Declare that this design installs a mechanism taken from ANOTHER site, list the premises the donor states for it, and answer each at the destination \u{2014} the check for a fix that carries a mechanism across without carrying its preconditions. Four acts on one tool: declare (`from` a donor fact + `into` a live modification target); select a premise (`premise` + `text`); answer one (`discharge` + `cites` a claim); withdraw (`withdraw` + `why`). A premise is REFUSED unless `text` is a verbatim substring of ONE observation in the donor fact \u{2014} copy the donor's words out of that fact's captured output, comment markers and indentation included; you select them, you never type them. An unanswered premise is a legal state while authoring (transcribe first, answer after) but `render` with `out` refuses to write a document that still has one. Whether an answering claim is TRUE is graded by grounding, never here. `workspace` is required (never defaulted); ids (X#, X#.#) are workspace-relative only.")]
    async fn transplant(
        &self,
        Parameters(p): Parameters<TransplantParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        let req = if let Some(id) = p.withdraw {
            transplants::TransplantRequest::Withdraw { id, why: p.why }
        } else if let Some(premise) = p.discharge {
            transplants::TransplantRequest::Discharge { premise, cites: p.cites }
        } else if let Some(transplant) = p.premise {
            transplants::TransplantRequest::Premise { transplant, text: p.text }
        } else {
            transplants::TransplantRequest::Declare { from: p.from, into: p.into }
        };
        match transplants::dispatch(&dir, req) {
            Ok(transplants::TransplantOutcome::Declared(t)) => Ok(CallToolResult::structured(json!({
                "id": t.id,
                "action": "declared",
                "donor": t.from,
                "destination": t.into,
            }))),
            Ok(transplants::TransplantOutcome::PremiseAdded(pr)) => Ok(CallToolResult::structured(json!({
                "id": pr.id,
                "action": "premise_selected",
                "next": "answer it at the destination: state the claim, then discharge this premise citing it",
            }))),
            Ok(transplants::TransplantOutcome::Discharged(pr)) => Ok(CallToolResult::structured(json!({
                "id": pr.id,
                "action": "discharged",
                "answered_by": pr.discharged_by,
            }))),
            Ok(transplants::TransplantOutcome::Withdrawn { id }) => {
                Ok(CallToolResult::structured(json!({"id": id, "action": "withdrawn"})))
            }
            Err(e) => Ok(refusal("transplant", &p.workspace, e)),
        }
    }

    #[tool(description = "Append a paragraph or heading to the document's prose, or `revise` an existing block. Write this as soon as a claim exists to say something about — don't defer prose to a writing phase at the end. `workspace` is required (never defaulted); ids (P#) are workspace-relative only.")]
    async fn prose(&self, Parameters(p): Parameters<ProseParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        let req = if let Some(id) = p.revise {
            prose::ProseRequest::Revise { id, text: p.text, why: p.why, cite: p.cites }
        } else if let Some(level) = p.heading_level {
            prose::ProseRequest::Heading { text: p.text, level: Some(level), before: p.before }
        } else {
            prose::ProseRequest::Paragraph { text: p.text, cite: p.cites, before: p.before }
        };
        match prose::dispatch(&dir, req) {
            Ok(prose::ProseOutcome::Created(b)) => Ok(CallToolResult::structured(json!({"id": b.id, "action": "appended"}))),
            Ok(prose::ProseOutcome::Revised { id }) => Ok(CallToolResult::structured(json!({"id": id, "action": "revised"}))),
            Err(e) => Ok(refusal("prose", &p.workspace, e)),
        }
    }

    #[tool(description = "Assemble the workspace into the finished document: prose in order, then an evidence ledger of every non-withdrawn claim, then a Facts table carrying each fact's note and its CAPTURED extent — which is what lets a reader check a note against what was actually opened without the workspace in hand. Run `review` before this and read each paragraph against the claims it cites. With `out`, also writes the workspace snapshot to `<out>.tetel/` in the same act, and mints the workspace identity if it has none; without a snapshot beside it a memo's citation ids resolve to nothing and `check` cannot tell self-grounding from independent grounding. Warns if observations are still pending, never minted into a fact. `workspace` is required (never defaulted).")]
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
        if let Err(e) = crate::transplants::refuse_incomplete(&dir) {
            return text_result(e.to_string());
        }

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
    async fn review(&self, Parameters(p): Parameters<ReviewParams>) -> Result<CallToolResult, ErrorData> {
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

    #[tool(description = "Check a rendered memo. Output is two partitions and never a single verdict. MACHINE-CHECKED (exit 1 if any fail): grammar, domain-subset-extent on enumerated rows, abutting literals, unsettled citations, dependency cascades, ledger import, verdict contradictions (supports AND refutes on one claim), claims out of proof (every record grades a wording the claim no longer carries), and provenance drift (the document is not what its own snapshot renders). HUMAN-OWED, never failing but never settled by a passing check: ungrounded claims, qualified verdicts with the grounder's own words, whether each claim was grounded by the workspace that authored it or an independent one, facts whose note names a location outside their captured extent, refusals recorded in a fact's own mint window, which working trees the facts were taken against when one tree was seen in more than one state, and a missing snapshot. Exit 2 means no tetel rows were found at all — out of scope, nothing checked, which is NOT a clean run. Read-only: never writes a file, runs a command from the document, or makes a network call.")]
    async fn check(&self, Parameters(p): Parameters<CheckParams>) -> Result<CallToolResult, ErrorData> {
        match crate::check_file(&resolved(&p.file)) {
            Ok((code, report)) => {
                let block = vec![ContentBlock::text(report)];
                Ok(if code == crate::EXIT_CLEAN { CallToolResult::success(block) } else { CallToolResult::error(block) })
            }
            Err(e) => Err(ErrorData::internal_error(format!("error reading {}: {e}", resolved(&p.file).display()), None)),
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
        match crate::brief_file(&resolved(&memo), p.json) {
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
                &resolved(&p.memo),
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
                Err(e) => Err(ErrorData::internal_error(
                    format!("error reading {}: {e}", resolved(&p.memo).display()),
                    None,
                )),
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
        // A caller that JSON-encodes `input` into a string instead of
        // passing an object gets the same result rather than a confusing
        // "invalid type: string, expected struct RecordInput". Both
        // shapes are unambiguous — a bare string is never a valid record
        // — so accepting each costs nothing and removes a trap that has
        // already cost one agent a run.
        let input_json = match &input {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        match crate::record_file(&resolved(&p.memo), &input_json) {
            Ok(Ok(())) => Ok(CallToolResult::structured(json!({
                "recorded": true,
                "witnessed": false,
            }))),
            Ok(Err(e)) => refused(e),
            Err(e) => Err(ErrorData::internal_error(format!("error reading {}: {e}", resolved(&p.memo).display()), None)),
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
    /// Written out by hand rather than left to `#[tool_handler]` (which
    /// generates it only when absent) so there is exactly one place every
    /// tool call passes through, and the staleness gate can sit in it.
    ///
    /// Gating here rather than per-tool is the point: a guard repeated at
    /// twelve call sites is twelve places to forget it, which is the
    /// paired-artifact defect this codebase keeps finding in itself.
    ///
    /// **A stale server refuses rather than warns.** It cannot reload
    /// itself, so a warning attached to an otherwise successful result
    /// asks the caller to notice and stop — which is precisely what did
    /// not happen on 2026-08-07, when a stale `check` verdict was
    /// believed and relayed. The agents in this loop have no read path to
    /// tetel except this server, so a warning they might not act on
    /// leaves the whole authoring loop grading with an old checker. The
    /// condition is objective (the file this process was launched from no
    /// longer contains this process), the remedy is singular (restart the
    /// client), and there is deliberately no override — an override is a
    /// way to reintroduce exactly the silence this closes.
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, ErrorData> {
        if let crate::buildid::Freshness::Stale { running, installed, path } =
            crate::buildid::freshness()
        {
            return Ok(CallToolResult::structured_error(json!({
                "error": "refused",
                "command": request.name,
                "guidance": crate::buildid::stale_guidance(&running, &installed, &path),
                "running_build": running,
                "installed_build": installed,
                "binary": path,
            }))
            .into());
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tetel", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Authoring (look/run/fact/claim/prose/render/query) and verification \
                 (check/brief/record) in one server, since a rendered memo is checkable by the \
                 same tool that wrote it. Every authoring tool requires an explicit `workspace` \
                 argument — there is no shared default, and ids it returns are workspace-relative \
                 only. See each tool's own description for details; those persist across calls \
                 where this one-time text may not. `check` names the build that graded it on its \
                 last line; if that build differs from one you are comparing against, the two \
                 verdicts were not produced by the same checker.",
            )
    }
}

/// Serve `TetelServer` over stdio until the peer disconnects.
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Before anything is served: fix what this process *is*, so that
    // every later comparison has a baseline taken while the file on disk
    // was still this build. See `buildid.rs` on why this cannot wait for
    // the first call.
    crate::buildid::capture();
    let server = TetelServer::new();
    let transport = stdio();
    server.serve(transport).await?.waiting().await?;
    Ok(())
}
