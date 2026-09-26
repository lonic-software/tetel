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

/// Distinguishes a caller-path refusal from a genuine I/O failure on a
/// workspace-less reader (`check`, `brief`, `record`'s two memo reads —
/// every MCP handler that calls `check_file`/`brief_file`/
/// `record_from_fact_file`/`record_file`, which all read through
/// `workspace::read_caller_path`) and returns the shape each deserves.
///
/// `read_caller_path` sets `ErrorKind::InvalidInput` deliberately for a
/// refusal — see `workspace.rs`. One other case can carry the same kind:
/// `fs::metadata` itself rejects a path containing an embedded NUL byte
/// (measured on this platform) before `read_caller_path`'s own
/// classification ever runs. That is not this function's refusal, but
/// routing it the same way is still the right shape — a caller-supplied
/// path the OS itself refuses to open is exactly the kind of thing a
/// `refused` result, not an opaque `internal_error`, should report. A
/// refusal (of either origin) surfaces the way `look`'s does: a
/// structured `refused` result the caller can act on, not
/// `internal_error`, which asserts something false for the deliberate
/// case — nothing went wrong internally, the tool correctly declined a
/// path it will not read.
///
/// Not routed through [`refusal`]/`workspace::refuse`: `check` and
/// `brief` hold no workspace to log a refusal into, which is the whole
/// reason they call `read_caller_path` instead of `guard_regular_file`
/// in the first place. This changes only the shape the caller receives.
///
/// A genuine failure (missing file, permission denied, …) is unchanged:
/// `internal_error_msg` is exactly the message each call site already
/// built for that case, so only the one error kind above is affected —
/// not the wording, and not any other kind.
fn reading_error(cmd: &str, e: std::io::Error, internal_error_msg: String) -> Result<CallToolResult, ErrorData> {
    if e.kind() == std::io::ErrorKind::InvalidInput {
        Ok(CallToolResult::structured_error(json!({
            "error": "refused",
            "command": cmd,
            "guidance": e.to_string(),
        })))
    } else {
        Err(ErrorData::internal_error(internal_error_msg, None))
    }
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
    verify: serde_json::Value,
) -> serde_json::Value {
    let attention: Vec<serde_json::Value> = crate::scope::for_fact(dir, id)
        .iter()
        .map(|o| {
            // The labels `advice` names, not all of them: see its doc.
            let (extent, more) = crate::scope::extent_shown(o);
            let mut entry = json!({
                "kind": "note-outside-extent",
                "mentioned": o.mentioned,
                "extent": extent,
                "guidance": crate::scope::advice(o),
            });
            if more > 0 {
                entry["extent_more"] = json!(more);
            }
            entry
        })
        .collect();
    let cap = |s: Vec<String>| -> Vec<serde_json::Value> {
        s.iter().map(|s| json!(crate::reply::capped(s, crate::reply::ENTRY_CAP))).collect()
    };
    // `refused` answers a different question from `folded`: not what this
    // mint took, but what the author tried and could not do in the window
    // that produced it. A caller that folded one leftover observation
    // after two refused `look` calls can see both here, which is the
    // whole of the incident this was built for.
    let lists = [
        ("attention", attention),
        ("folded", cap(folded)),
        ("refused_since_previous_fact", cap(refused)),
    ];
    let mut out = json!({
        "id": id,
        "action": action,
        // Built by `verify_block` rather than here, so the status
        // vocabulary, the model name, the guidance string and the
        // non-determinism marker are one definition reaching all three
        // verbs. `fact_result` builds only `fact` results; the other two
        // are assembled inline at their own sites, and the verb this
        // design ships enabled is one of the two it never touches.
        "verify": verify,
    });
    for (k, v) in &lists {
        out[*k] = json!(v);
    }
    // `structured()` sends the JSON twice, once as text, so a reply that
    // fits in half the budget goes out whole with its structured copy. One
    // that does not is fitted to the whole budget instead: the backstop
    // then drops the structured copy, which loses nothing, and entries are
    // worth more than a duplicate of the text.
    let budget = crate::reply::REPLY_BUDGET;
    if out.to_string().len() <= budget / 2 {
        return out;
    }
    fit_lists(out, id, lists, budget)
}

/// Keep `fact`'s lists within `room` bytes of JSON, in priority order
/// (TET-93 C11): `verify` is already in `out` and is never touched here,
/// then whole `attention` entries, then `folded`, then refusals, each list
/// kept from its start while the whole stays within `room`. What a list
/// leaves out is counted under `omitted`, with where to read it.
///
/// `room` is the whole budget: `fact_result` calls this only when the reply
/// cannot go out structured, and `verify`'s allowance leaves the rest of the
/// reply `ENTRY_CAP` beside it.
fn fit_lists<const N: usize>(
    mut out: serde_json::Value,
    id: &str,
    lists: [(&str, Vec<serde_json::Value>); N],
    room: usize,
) -> serde_json::Value {
    let omitted = |counts: &[usize; N]| {
        let mut o = serde_json::Map::new();
        for ((k, _), n) in lists.iter().zip(counts) {
            o.insert((*k).to_string(), json!(n));
        }
        o.insert(
            "see".into(),
            json!(format!(
                "`query` with what: facts, id: {id} shows its note and every extent it folded; `check` \
on the rendered memo lists every refusal in its mint window"
            )),
        );
        serde_json::Value::Object(o)
    };
    let totals: [usize; N] = std::array::from_fn(|i| lists[i].1.len());
    for (k, _) in &lists {
        out[*k] = json!([]);
    }
    // Reserved at its longest: a count never has more digits than its list's
    // length.
    out["omitted"] = omitted(&totals);
    let mut used = out.to_string().len();
    let mut kept: [Vec<serde_json::Value>; N] = std::array::from_fn(|_| Vec::new());
    for (i, (_, entries)) in lists.iter().enumerate() {
        for e in entries {
            let cost = e.to_string().len() + usize::from(!kept[i].is_empty());
            if used + cost > room {
                break;
            }
            used += cost;
            kept[i].push(e.clone());
        }
    }
    let counts: [usize; N] = std::array::from_fn(|i| totals[i] - kept[i].len());
    for ((k, _), v) in lists.iter().zip(kept) {
        out[*k] = json!(v);
    }
    if counts.iter().any(|&n| n > 0) {
        out["omitted"] = omitted(&counts);
    } else {
        out.as_object_mut().expect("json object").remove("omitted");
    }
    out
}

/// Start a verification for a mint that has just been committed, if this
/// verb is one the settings turn on, and return the mint id it was started
/// for.
///
/// The subject is built lazily: assembling the captured side means loading
/// every fact in the workspace, and a disabled verb should not pay for
/// that. Nothing here can fail the mint — a subject that cannot be
/// assembled simply starts nothing, because the record is already written
/// and the reply is already owed.
fn start_verification(
    dir: &Path,
    settings: &crate::verify::Settings,
    verb: &str,
    subject: impl FnOnce() -> Option<crate::verify::Subject>,
) -> Start {
    if !crate::verify::verb_enabled(settings, verb) {
        return Start::NotAttempted;
    }
    let Some(s) = subject() else {
        // The verb is on; this call simply wrote nothing a captured
        // record can be compared against — a heading, a block citing no
        // claim, a withdrawal. Reporting that as `off` would tell an
        // author who has just enabled the feature that it is disabled.
        return Start::NothingToCompare;
    };
    let mint = s.mint.clone();
    // Only report a mint as queued if a thread actually started for it.
    // Saying `queued` for a verification that never began promises a
    // finding that can never arrive, and the author polls for it forever.
    if crate::verify::spawn(dir, settings, s) {
        Start::Queued(mint)
    } else {
        Start::NotAttempted
    }
}

/// What `start_verification` did, in the caller's owned form.
enum Start {
    Queued(String),
    NothingToCompare,
    NotAttempted,
}

impl Start {
    fn trigger(&self) -> crate::verify::Trigger<'_> {
        match self {
            Start::Queued(m) => crate::verify::Trigger::Queued(m),
            Start::NothingToCompare => crate::verify::Trigger::NothingToCompare,
            Start::NotAttempted => crate::verify::Trigger::NotAttempted,
        }
    }
}

/// Build the `verify` object for a reply that is about to go back, and
/// only then mark a delivered verification as delivered.
///
/// The commit belongs here and not beside the peek. A refusal is an
/// ordinary outcome — an empty pending buffer, an unknown id — and a
/// refusal reply carries no `verify` object, so consuming the finding
/// when the log is read would throw it away on any call that happened to
/// be refused. Committing where the payload is built makes the worst case
/// showing a finding twice rather than losing one.
fn verify_block(
    dir: &Path,
    settings: &crate::verify::Settings,
    verb: &str,
    peeked: crate::verify::Peeked,
    started: &Start,
) -> serde_json::Value {
    let crate::verify::Peeked { delivered, log } = peeked;
    if let Some((at, _)) = &delivered {
        crate::verify::commit_delivered(dir, *at);
    }
    // After the dispatch, so a claim withdrawn by this very call is already
    // left out.
    let unverified = crate::verify::unverified(dir, settings, &log);
    fit_findings(
        crate::verify::block(settings, verb, delivered.as_ref().map(|(_, r)| r), started.trigger(), unverified.as_ref()),
        crate::reply::VERIFY_ALLOWANCE,
    )
}

/// The most of one finding's `clause`, `evidence` or `why` a reply quotes,
/// when the allowance has room for more.
const FINDING_TEXT_CAP: usize = crate::reply::ENTRY_CAP / 2;

/// A finding's fields that quote text, and so are cut; the rest (`kind`,
/// `facts`, the fidelity marks, and an `unevidenced` finding's `literal`,
/// which is that finding's whole content) are shown whole.
const FINDING_TEXT: [&str; 3] = ["clause", "evidence", "why"];

/// Hold a `verify` object's findings to `allowance` bytes, serialized
/// (TET-93 C11).
///
/// The finding is already committed as delivered by the time this runs,
/// and no surface prints a delivered finding afterwards, so the one thing
/// shaping must not do is drop one. Every finding is shown; what shrinks is
/// the text each one quotes, to the largest cap (at most
/// [`FINDING_TEXT_CAP`]) at which all of them fit, so more findings means
/// shorter text rather than fewer findings. Only when the findings'
/// uncuttable fields alone cannot fit, on the order of a hundred findings
/// in one verification, are the ones past the last that fits withheld,
/// and `findings_withheld` says how many. That loss is conceded rather
/// than avoided by leaving the delivery uncommitted: the delivery cursor is
/// one position, and holding it back would block every later verification
/// in the workspace.
fn fit_findings(verify: serde_json::Value, allowance: usize) -> serde_json::Value {
    let Some(findings) = verify.get("findings").and_then(serde_json::Value::as_array).cloned() else {
        return verify;
    };
    let with = |findings: &[serde_json::Value], cap: usize, withheld: usize| {
        let mut v = verify.clone();
        let cut: Vec<serde_json::Value> = findings
            .iter()
            .map(|f| {
                let mut f = f.clone();
                for k in FINDING_TEXT {
                    if let Some(serde_json::Value::String(s)) = f.get(k) {
                        f[k] = json!(crate::reply::capped(s, cap));
                    }
                }
                f
            })
            .collect();
        v["findings"] = json!(cut);
        if withheld > 0 {
            v["findings_withheld"] = json!(withheld);
        }
        v
    };
    let fits = |v: &serde_json::Value| v.to_string().len() <= allowance;
    // The largest `cap` in MIN_CAP..=FINDING_TEXT_CAP at which `shown`
    // fits, if any does. From the trailer's length up, a cut value's size
    // never shrinks as its cap grows; below it, `capped` turns a field
    // shorter than the trailer into the longer trailer, so the smallest cap
    // would not be the smallest reply, and findings that fit whole would be
    // withheld.
    const MIN_CAP: usize = crate::reply::ELLIPSIS.len();
    let best_cap = |shown: &[serde_json::Value], withheld: usize| {
        if !fits(&with(shown, MIN_CAP, withheld)) {
            return None;
        }
        let (mut lo, mut hi) = (MIN_CAP, FINDING_TEXT_CAP);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if fits(&with(shown, mid, withheld)) { lo = mid } else { hi = mid - 1 }
        }
        Some(lo)
    };
    if let Some(cap) = best_cap(&findings, 0) {
        return with(&findings, cap, 0);
    }
    // The floor: the longest prefix whose uncuttable fields fit.
    let (mut lo, mut hi) = (0, findings.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if best_cap(&findings[..mid], findings.len() - mid).is_some() { lo = mid } else { hi = mid - 1 }
    }
    let cap = best_cap(&findings[..lo], findings.len() - lo).unwrap_or(MIN_CAP);
    with(&findings[..lo], cap, findings.len() - lo)
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
    /// The workspace whose pending buffer receives the output.
    workspace: String,
    /// The file to open (plain mode) — must be a regular file, or a
    /// symlink to one; a directory is refused there — or the
    /// file/directory to search when `grep` is given, where a directory
    /// is fine (grep recurses into it) and so is a single regular file.
    /// A FIFO, socket or device named directly as `path` is refused in
    /// either mode (TET-79) — one reached by recursing into a searched
    /// directory is not covered, and still blocks the search.
    /// **Pass an absolute path** — see this module's doc comment on why
    /// a relative one resolves somewhere you cannot predict.
    path: String,
    /// Restrict the open to this 1-based inclusive line range. Only
    /// valid without `grep`.
    #[serde(default)]
    lines: Option<LineRange>,
    /// Search `path` for this pattern instead of opening it.
    ///
    /// POSIX extended regular expressions — the `grep -E` dialect — never
    /// a literal string. Parentheses group, so a literal `(` or `)` (and
    /// likewise `+ ? { } |`) needs its own backslash. ERE cannot spell
    /// "match this whole string of metacharacters literally"; for that,
    /// use `run` with `["grep", "-P", …]`, whose own argv records the
    /// dialect it ran under.
    #[serde(default)]
    grep: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RunParams {
    /// The workspace whose pending buffer receives the output.
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
    /// Required with `revise`. Free text, stored with the edit.
    #[serde(default)]
    why: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TargetParams {
    /// The authoring workspace this target belongs to.
    workspace: String,
    /// The symbol to be modified, exactly as searched: it is compared
    /// byte-for-byte with the census pattern, so give the symbol itself,
    /// not a longer phrase containing it.
    #[serde(default)]
    symbol: Option<String>,
    /// Id of the fact holding the census: a `look` with `grep` set to
    /// exactly this symbol and `path` set to the worktree root. One fact
    /// id, not a list.
    #[serde(default)]
    cites: Option<String>,
    /// Withdraw this existing target instead of declaring one. There is
    /// no `revise`: a changed symbol is a different census.
    #[serde(default)]
    withdraw: Option<String>,
    /// Required with `withdraw`. Free text, stored with the withdrawal.
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
    /// Required with `withdraw`. Free text, stored with the withdrawal.
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
    /// Required with `revise`/`withdraw`. Free text, stored with the change.
    #[serde(default)]
    why: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ProseParams {
    /// The authoring workspace this prose block belongs to.
    workspace: String,
    /// The block's text. Plain text — no shell involved, so backticks,
    /// quotes and embedded newlines all pass through byte-exact.
    /// Required for a create or a revise; omit only with `ack`, which
    /// carries none — the server refuses in code, rather than at the
    /// schema, when it is missing and the call is not an `ack`.
    #[serde(default)]
    text: Option<String>,
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
    /// Required with `revise` or `ack`. Free text, stored with the change.
    #[serde(default)]
    why: Option<String>,
    /// Acknowledge this existing block's *current* text and citations
    /// against the claims they cite, instead of creating or revising —
    /// see the `prose --ack` design memo. Requires `why`; refused if
    /// combined with `text`, `revise`, `heading_level`, `cites` or
    /// `before`.
    #[serde(default)]
    ack: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RenderParams {
    /// Name of the workspace to render.
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
    /// The workspace whose prose and claims to list.
    workspace: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct QueryParams {
    /// The authoring workspace to inspect.
    workspace: String,
    /// Which read-only view to return.
    what: QueryWhat,
    /// Required when `what` is `"deps"`: the fact or claim id to look up
    /// (must start with `F` or `C`). With `"facts"` or `"claims"`: return
    /// that one record uncut instead of the listing.
    #[serde(default)]
    id: Option<String>,
    /// The id to start at: the record a `facts`, `claims` or `prose`
    /// listing starts at, or the dependent `deps` starts at. A reply that
    /// leaves records out leads with the `from` to continue with.
    #[serde(default)]
    from: Option<String>,
    /// With `id` on a fact only: the 1-based extent to start that fact's
    /// extents at. A reply that leaves extents out names the next one.
    #[serde(default)]
    extent_from: Option<usize>,
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
    /// Print the writing guide for the `look`/`run`/`fact`/`claim`/
    /// `prose`/`render` sequence instead of a memo's claims for grading.
    /// Takes no memo.
    #[serde(default)]
    authoring: bool,
    /// How many distinct non-author workspaces must already have graded a
    /// claim's **current** wording before it drops off the owed list.
    /// Omit for the default. Must be at least 1 — a floor of 0 leaves
    /// nothing ever owed, which is the empty schedule a switched-off flag
    /// would produce, and there is deliberately no such flag.
    #[serde(default)]
    confirm: Option<u32>,
}

// The published shape of `record`'s `input`.
//
// `input` deserialises as a bare [`serde_json::Value`] so that both an
// object and a JSON-encoded string are accepted — but `Value`'s own
// generated schema carries **no `type` at all**, just a description. A
// client with nothing telling it this is an object reasonably
// serialises one into a string, and then `record` refuses with
// "invalid type: string, expected struct RecordInput". That happened
// three times to one agent before this existed.
//
// So the runtime type stays permissive and the *schema* is declared
// here, field by field, mirroring [`crate::evidence::RecordInput`]. A
// caller now sees what to send instead of guessing from prose.
//
// Plain comments, not doc comments: schemars serves a struct's doc
// comment as the schema's `description`, so this history reached every
// model that loaded `record` (TET-91).
#[derive(Debug, Deserialize, JsonSchema)]
struct IngestedRecord {
    /// The claim id this result grades — must exist in the memo's ledger.
    claim: String,
    /// Which grading pass this is. Free text on this path, and validated
    /// only for being non-empty: nothing can check it. Use `from_fact`
    /// instead for independence that can be derived.
    pass: String,
    /// `supports` | `refutes` | `qualifies`. A `qualifies` requires
    /// `note`.
    verdict: String,
    /// The kind of act being reported: `run` | `reading` | `observed` |
    /// `attested`. Stored as given and never consulted when standing is
    /// derived — this path records a report of an act, not the act, so
    /// every ingested record caps at attested regardless of this value.
    reported_kind: String,
    /// Where the act is preserved: a file path, or
    /// `proc:<session-or-agent>` when the only record of it is the session
    /// or agent that performed it. Required — a record naming nothing
    /// preserved anywhere is rejected.
    source: String,
    /// What the act examined. Supplied by hand on this path, which is what
    /// marks the record as reported rather than witnessed.
    #[serde(default)]
    extent: Vec<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    pin: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecordParams {
    /// Absolute path of the memo file whose ledger defines the claim id (a
    /// relative path resolves against the server's working directory).
    memo: String,
    /// One grading result, shaped as `evidence::RecordInput`: `claim`,
    /// `pass`, `verdict` (`supports`|`refutes`|`qualifies`),
    /// `reported_kind` (`run`|`reading`|`observed`|`attested`), `source`
    /// (a file path, or `proc:<session-or-agent>`), and optional
    /// `extent`/`note`/`pin`. Given as a JSON object directly — MCP is
    /// JSON end to end, so there is no reason to make this a
    /// JSON-encoded string a caller has to escape into.
    ///
    /// This is the **ingested** path: `extent` and `source` are supplied
    /// by hand, so the server holds a report of the act and not the act,
    /// and the record caps at attested standing. Prefer `from_fact` below
    /// for anything captured through this server.
    #[serde(default)]
    #[schemars(with = "Option<IngestedRecord>")]
    input: Option<serde_json::Value>,
    /// Grade `claim` against a fact **this workspace captured** — the
    /// witnessed path. The extent is copied from the fact, as `look`/`run`
    /// captured it, and this path has no field for supplying one; that is
    /// what separates it from `input`. The record carries the workspace's
    /// identity, so `check` can recompute whether a grading pass relied on
    /// output it captured itself or on another workspace's.
    ///
    /// Requires `workspace`, `claim` and `verdict`.
    #[serde(default)]
    from_fact: Option<String>,
    /// The workspace whose fact is being cited. Required with `from_fact`.
    #[serde(default)]
    workspace: Option<String>,
    /// Which claim is being graded. Required with `from_fact`.
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
    /// Explanation. Required when `verdict` is `qualifies`: name the
    /// condition the proposition omits, or what could not be established.
    #[serde(default)]
    note: Option<String>,
}

/// Builds the `check` tool's real MCP description from the same category
/// arrays `report.rs` uses for its own scope strings —
/// [`crate::report::MACHINE_CHECKED_CATEGORIES`] and
/// [`crate::report::HUMAN_OWED_CATEGORIES`] — rather than hand-typing a
/// second copy of either list here. `TetelServer::new` installs this
/// text over the `#[tool(description = ...)]` placeholder on `check`
/// immediately after construction; see that attribute's comment for why
/// the macro cannot call this function directly.
fn check_description() -> String {
    format!(
        "Check a rendered memo. Output is two partitions and never a single verdict. \
MACHINE-CHECKED (exit 1 if any fail): {}. \
HUMAN-OWED, never failing but never settled by a passing check: {}. \
Exit {} means no tetel rows were found at all — out of scope, nothing checked, which is NOT a \
clean run. Read-only: never writes a file, runs a command from the document, or makes a network \
call.",
        crate::report::join_categories(crate::report::MACHINE_CHECKED_CATEGORIES),
        crate::report::join_categories(crate::report::HUMAN_OWED_CATEGORIES),
        crate::report::EXIT_NO_ROWS,
    )
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
        let mut tool_router = Self::tool_router();
        // `check`'s `#[tool(description = ...)]` above carries only a
        // placeholder — see its comment for why the macro cannot compute
        // this text itself. Patched once here, before the router is ever
        // handed to a transport, so every real caller (including
        // `list_tools`/`get_tool`) sees the generated text and nothing
        // else ever does.
        if let Some(route) = tool_router.map.get_mut("check") {
            route.attr.description = Some(std::borrow::Cow::Owned(check_description()));
        }
        // Every tool declares its spill threshold, set here over the whole
        // router rather than per `#[tool]` so a tool added later declares
        // it without anyone remembering to. See `reply` on why the
        // declared figure is headroom over the budget, not the budget.
        for route in tool_router.map.values_mut() {
            route.attr.meta.get_or_insert_with(rmcp::model::MetaObject::new).insert(
                crate::reply::MAX_RESULT_SIZE_KEY.to_string(),
                json!(crate::reply::DECLARED_MAX_RESULT_SIZE_CHARS),
            );
        }
        Self { tool_router }
    }

    #[tool(description = "Read a file, or search a file or directory with `grep`, and hold the result in the workspace's pending buffer until `fact` mints it. `path` must be a regular file (or a symlink to one), or, with `grep`, a directory to search recursively. `grep` is POSIX extended regular expressions (the `grep -E` dialect), never a literal string — parentheses group, so a literal `(`, `)`, and likewise `+ ? { } |`, needs its own backslash; for a whole-string literal search, use `run` with `[\"grep\", \"-P\", …]` instead, whose own argv records the dialect it ran under. A malformed pattern is rejected before anything is searched. A FIFO, socket or device named directly as `path` is rejected up front in either mode, because it does not behave like a file (a FIFO with no writer blocks forever; a device like `/dev/zero` never reaches EOF) — but one reached by recursing into a searched directory is not covered, and still blocks the search. `workspace` is required (never defaulted); ids elsewhere are workspace-relative only.")]
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

    #[tool(description = "Execute a command directly (no shell) and record its combined stdout/stderr into the pending buffer. The whole output is captured verbatim and, once minted into a fact, becomes permanently unrevisable and ships into the snapshot beside the memo — so a command whose output is enormous, or contains a credential, puts that in a repository forever. A command exiting 0 also establishes nothing by exiting 0: read the output and ask whether it states your proposition or merely fails to deny it. There is a wall-clock bound (`run.timeout_ms`, 5 minutes unset): past it the command and everything it started are killed, the call is refused, and NOTHING is captured — so a command you expect to take longer needs the bound raised first, not a retry. `workspace` is required (never defaulted); ids elsewhere are workspace-relative only.")]
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

    #[tool(description = "Mint a fact from the pending buffer (rejected when the buffer is empty — call `look`/`run` first), or `revise` an existing fact's note (extent, output and pin are set once at mint time and never revised). The result's `attention` array lists every location the note names that the fact's captured extent does not cover; each entry needs either a `look` at that location and a fact for it, or a narrower note. The result also carries `folded` (what this mint took from the pending buffer, with ages) and `refused_since_previous_fact` (every rejection logged in this workspace since the previous mint, whatever the tool — each `error: refused`, plus `render --out` and CLI rejections — as the first log line of each) — a refused `look` leaves the buffer untouched, so a file missing from `folded` for that reason shows up there instead; a read that failed with `error: io` does not. A reply is held to a size budget: whole entries are kept in the order `attention`, `folded`, refusals, and whatever is left out is counted under `omitted`, which says where to read it. Each `folded` and refusal entry is cut to 1024 bytes; an `attention` entry's `extent` and `guidance` name at most four labels, each cut to 1024 bytes, and `extent_more` counts the rest. The result also carries `verify`, an object with a mandatory `status`: `off`/`unauthorized`/`queued`/`skipped` mean no finding is being reported, and `ok`/`gated`/`unavailable`/`timeout`/`unparsable` report a verification started by an earlier call, which `for_mint` names; `findings` is meaningful only under `ok`; the `claim` tool's description says what each status and finding carries. On `fact` it is on by default: 88% of what it reports about a note is correct, and it reports something about roughly one note in fourteen. `workspace` is required (never defaulted); minted ids (F#) are workspace-relative only.")]
    async fn fact(&self, Parameters(p): Parameters<FactParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        // Captured before the request consumes `p.revise`.
        let revise_target = p.revise.clone();
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
        // Every authoring call in the workspace delivers whatever finished
        // since the last one — a verification outlives the reply that
        // triggered it, so this is where it lands.
        let settings = crate::verify::settings(&dir, "fact");
        let delivered = crate::verify::peek_delivered(&dir);
        // The note as it stood before this call. `facts::revise` refuses
        // only a missing id, an empty `--why` and an empty note — it
        // appends happily for a note byte-identical to the current one —
        // so the no-op skip has to be made here, as it is for the other
        // two verbs.
        let previous_note = match (&revise_target, crate::verify::verb_enabled(&settings, "fact")) {
            (Some(id), true) => facts::load_all(&dir)
                .ok()
                .and_then(|fs| fs.into_iter().find(|f| &f.id == id))
                .map(|f| f.note),
            _ => None,
        };
        match facts::dispatch(&dir, req) {
            Ok(facts::FactOutcome::Minted(f)) => {
                let queued = start_verification(&dir, &settings, "fact", || {
                    crate::verify::fact_subject(&dir, &f.id).ok()
                });
                let verify = verify_block(&dir, &settings, "fact", delivered, &queued);
                Ok(CallToolResult::structured(fact_result(&dir, &f.id, "minted", folded, refused, verify)))
            }
            Ok(facts::FactOutcome::Revised { id }) => {
                // A revision changes the note, which is the text being
                // compared, so it is normally a new comparison. A note
                // revised to byte-identical text is not, and nothing
                // below this stops it — `facts::revise` appends such an
                // event without complaint — so the check is made here.
                let queued = start_verification(&dir, &settings, "fact", || {
                    let subject = crate::verify::fact_subject(&dir, &id).ok()?;
                    if previous_note.as_deref() == Some(subject.text.as_str()) {
                        return None;
                    }
                    Some(subject)
                });
                let verify = verify_block(&dir, &settings, "fact", delivered, &queued);
                Ok(CallToolResult::structured(fact_result(
                    &dir,
                    &id,
                    "revised",
                    Vec::new(),
                    Vec::new(),
                    verify,
                )))
            }
            Err(e) => Ok(refusal("fact", &p.workspace, e)),
        }
    }

    #[tool(description = "Assert a claim resting on one or more fact ids, or `revise`/`withdraw` an existing one. Expect to `revise` a claim when writing its prose exposes it as imprecise or needing a qualification — that's the normal rhythm, not a mistake. Creating a claim returns an OVERLAP REPORT: the id and shared designator(s) (extent key, e.g. a resolved file path) of every other fact whose extent touches the same file or command as the facts you cited, and which you did NOT cite — not that fact's note. It is not an error — read it and decide whether one of them belongs in this claim, or whether citing only some of what you looked at is deliberate. Want the note of an overlapping fact? Get it from `query facts` with that fact's `id`, which returns it uncut. Every result also carries `verify`, an object with a mandatory `status`: `off`/`unauthorized`/`queued`/`skipped` mean no finding is being reported to you, and `ok`/`gated`/`unavailable`/`timeout`/`unparsable` report a verification started by an EARLIER call — `for_mint` says which one, because it is no longer the id beside it. `gated` means a TypeSafe gate (`verify.typed_model`) judged the text to have nothing to find and nothing was compared. A delivered `timeout`/`unavailable`/`unparsable` carries `detail` saying why that mint went unchecked, and `unverified` names every mint whose latest verification failed so. `findings` is meaningful only under `ok`, and a finding is not an error: a model thought your wording and the captured evidence disagree, it is wrong a meaningful fraction of the time, and `deterministic: false` is there because two identical mints can answer differently. Each finding's `kind` is `contradicts` or `overreaches` — or, when `literals` is on, `unevidenced`, meaning your text states a number, path or name as current fact that appears in no capture you cited; that one names a `literal` rather than quoting evidence, because the finding IS the absence. Two fidelity marks travel with every finding and are worth reading before you act on it: `facts` lists every cited fact whose captured output contains the quoted span (empty means none did, which is what `quoted: false` says), and `clause_quoted: false` means the clause shown is the model's paraphrase rather than your words. A finding's quoted text (`clause`, `evidence`, `why`) is cut to at most 512 bytes, ending ` …`, and shorter when there are many findings; every finding is shown, unless a verification has so many that not even their other fields fit, when `findings_withheld` counts the rest. Read the quoted evidence and decide. `workspace` is required (never defaulted); ids (C#) are workspace-relative only.")]
    async fn claim(&self, Parameters(p): Parameters<ClaimParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        // Captured before the request consumes `p.revise`.
        let revise_target = p.revise.clone();
        let req = if let Some(id) = p.withdraw {
            claims::ClaimRequest::Withdraw { id, why: p.why }
        } else if let Some(id) = p.revise {
            claims::ClaimRequest::Revise { id, prop: p.proposition, from: p.cites, why: p.why }
        } else {
            claims::ClaimRequest::Create { prop: p.proposition, from: p.cites }
        };
        let settings = crate::verify::settings(&dir, "claim");
        let delivered = crate::verify::peek_delivered(&dir);
        // The wording as it stood before this call, so a revision that
        // leaves the compared text alone can make no call — which is what
        // this design promised, and where most of the volume lives: two
        // thirds of claim traffic in the largest memo on disk is
        // revision. Read only when the verb is actually on, so a disabled
        // feature pays nothing for it.
        let previous_prop = match (&revise_target, crate::verify::verb_enabled(&settings, "claim")) {
            (Some(id), true) => claims::load_all(&dir)
                .ok()
                .and_then(|cs| cs.into_iter().find(|c| &c.id == id))
                // Both halves of the comparison, not just the author's:
                // a revision that keeps the proposition and changes the
                // cited facts is a different comparison.
                .map(|c| (c.prop, c.from)),
            _ => None,
        };
        match claims::dispatch(&dir, req) {
            Ok(claims::ClaimOutcome::Created(outcome)) => {
                let overlap: Vec<_> =
                    outcome.overlap.iter().map(|(id, keys)| json!({"id": id, "keys": keys})).collect();
                // The captured side is the cited facts TOGETHER WITH the
                // overlap set. Cited alone would be author-selected, and
                // an overreaching proposition could then be made to agree
                // with its evidence by citing only the facts that agree
                // with it — the author's diligence checking the author's
                // diligence, which is the failure the construction exists
                // to avoid. Both are already on disk at this point and
                // neither costs a new mechanism.
                let queued = start_verification(&dir, &settings, "claim", || {
                    crate::verify::claim_subject(
                        &dir,
                        &outcome.claim.id,
                        &outcome.claim.prop,
                        &outcome.claim.from,
                        &outcome.overlap,
                        outcome.claim.revisions,
                    )
                    .ok()
                });
                Ok(CallToolResult::structured(json!({
                    "id": outcome.claim.id,
                    "action": "created",
                    "overlap": overlap,
                    "verify": verify_block(&dir, &settings, "claim", delivered, &queued),
                })))
            }
            Ok(claims::ClaimOutcome::Revised { id }) => {
                let queued = start_verification(&dir, &settings, "claim", || {
                    // Inside the closure, not before it: replaying the
                    // whole claim log is what the laziness is for, and
                    // verification is off by default.
                    let c = claims::load_all(&dir).ok()?.into_iter().find(|c| c.id == id)?;
                    if previous_prop.as_ref() == Some(&(c.prop.clone(), c.from.clone())) {
                        // Same text, same evidence, same answer as the
                        // call that already paid for it.
                        return None;
                    }
                    let overlap = claims::overlap_for(&dir, &c.from).unwrap_or_default();
                    crate::verify::claim_subject(&dir, &c.id, &c.prop, &c.from, &overlap, c.revisions).ok()
                });
                Ok(CallToolResult::structured(json!({
                    "id": id,
                    "action": "revised",
                    "verify": verify_block(&dir, &settings, "claim", delivered, &queued),
                })))
            }
            // A withdrawal leaves no text to compare, so it starts
            // nothing — but it is still an authoring call, and still
            // delivers whatever finished before it. Routed through the
            // same helper so that with the verb on it reports `skipped`
            // rather than `off`.
            Ok(claims::ClaimOutcome::Withdrawn { id }) => {
                let queued = start_verification(&dir, &settings, "claim", || None);
                Ok(CallToolResult::structured(json!({
                    "id": id,
                    "action": "withdrawn",
                    "verify": verify_block(&dir, &settings, "claim", delivered, &queued),
                })))
            }
            Err(e) => Ok(refusal("claim", &p.workspace, e)),
        }
    }

    #[tool(description = "Declare a symbol this design tells an implementer to modify, citing the fact that censuses it. Rejected unless that fact's captured extent contains a search of the WHOLE WORKTREE for exactly this symbol \u{2014} so capture it with `look` using `grep: \"<symbol>\"` and `path: \"<worktree root>\"`, then `fact`, then cite that fact here. The check is on whether the search exists and where it was rooted, never on what it found; a symbol with no occurrences is censused by a search showing it has none. Declaring is not required by anything \u{2014} nothing can detect a recommendation that was not declared \u{2014} so an undeclared target is invisible, and the rendered section says so. `workspace` is required (never defaulted); ids (T#) are workspace-relative only.")]
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

    #[tool(description = "Append a paragraph or heading to the document's prose, `revise` an existing block, or `ack` a block: record that its current text and citations were compared with the claims they cite and need no change (requires `why`; clears a `prose-revised-since-proof` finding for it, and is rejected if combined with `text`, `revise`, `heading_level`, `cites` or `before`). Write prose as soon as a claim exists to say something about — don't defer to a writing phase at the end. The result also carries `verify`, an object with a mandatory `status` — see the `claim` tool's description for the vocabulary; on `prose` it is `off` unless you have turned the verb on, this being the highest-volume verb of the three and the only one whose precision sits at the floor of what is worth printing rather than above it. `workspace` is required (never defaulted); ids (P#) are workspace-relative only.")]
    async fn prose(&self, Parameters(p): Parameters<ProseParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        // Captured before the request consumes `p.revise`.
        let revise_target = p.revise.clone();
        let req = if let Some(id) = p.ack {
            // `heading` has no MCP-side equivalent to refuse independently
            // of `text`: unlike the CLI, this schema has no separate
            // heading-text field, so a heading create/revise's text
            // already lands in `p.text` and is caught by that check
            // alone. `heading_level` stands in for the CLI's `--level`
            // here, which is exactly the flag an enumeration written
            // against this shape alone would miss (see `dispatch`'s doc
            // comment).
            prose::ProseRequest::Ack {
                id,
                why: p.why,
                text: p.text,
                revise: p.revise,
                heading: None,
                level: p.heading_level,
                cites: p.cites,
                before: p.before,
            }
        } else {
            // The schema no longer requires `text` (an `ack` carries
            // none), so its absence for every other mode is refused here,
            // in code, rather than by deserialisation — see `ProseParams`
            // and the design memo on why that move is load-bearing.
            let Some(text) = p.text else {
                return Ok(refusal(
                    "prose",
                    &p.workspace,
                    workspace::refuse(&dir, "prose", "prose requires text (omit it only with `ack`)"),
                ));
            };
            if let Some(id) = p.revise {
                prose::ProseRequest::Revise { id, text, why: p.why, cite: p.cites }
            } else if let Some(level) = p.heading_level {
                prose::ProseRequest::Heading { text, level: Some(level), before: p.before }
            } else {
                prose::ProseRequest::Paragraph { text, cite: p.cites, before: p.before }
            }
        };
        let settings = crate::verify::settings(&dir, "prose");
        let delivered = crate::verify::peek_delivered(&dir);
        // Same reason as `claim`: an unchanged text is a comparison
        // already paid for.
        let previous_text = match (&revise_target, crate::verify::verb_enabled(&settings, "prose")) {
            (Some(id), true) => prose::load_all(&dir)
                .ok()
                .and_then(|bs| bs.into_iter().find(|b| &b.id == id))
                .map(|b| (b.text, b.cite)),
            _ => None,
        };
        match prose::dispatch(&dir, req) {
            Ok(prose::ProseOutcome::Created(b)) => {
                // A heading cites nothing and asserts nothing about
                // captured evidence, so there is no comparison to make.
                let queued = start_verification(&dir, &settings, "prose", || {
                    if b.heading || b.cite.is_empty() {
                        return None;
                    }
                    crate::verify::prose_subject(&dir, &b.id, &b.text, &b.cite, b.revisions).ok()
                });
                Ok(CallToolResult::structured(json!({
                    "id": b.id,
                    "action": "appended",
                    "verify": verify_block(&dir, &settings, "prose", delivered, &queued),
                })))
            }
            Ok(prose::ProseOutcome::Revised { id }) => {
                let queued = start_verification(&dir, &settings, "prose", || {
                    // Inside the closure, for the same reason.
                    let b = prose::load_all(&dir).ok()?.into_iter().find(|b| b.id == id)?;
                    if b.heading || b.cite.is_empty() {
                        return None;
                    }
                    if previous_text.as_ref() == Some(&(b.text.clone(), b.cite.clone())) {
                        return None;
                    }
                    crate::verify::prose_subject(&dir, &b.id, &b.text, &b.cite, b.revisions).ok()
                });
                Ok(CallToolResult::structured(json!({
                    "id": id,
                    "action": "revised",
                    "verify": verify_block(&dir, &settings, "prose", delivered, &queued),
                })))
            }
            // An acknowledgement changes no text and cites nothing new, so
            // the comparison would be the one the previous call already
            // made. It starts nothing and delivers as any authoring call
            // does.
            Ok(prose::ProseOutcome::Acked { id }) => {
                let queued = start_verification(&dir, &settings, "prose", || None);
                Ok(CallToolResult::structured(json!({
                    "id": id,
                    "action": "acknowledged",
                    "verify": verify_block(&dir, &settings, "prose", delivered, &queued),
                })))
            }
            Err(e) => Ok(refusal("prose", &p.workspace, e)),
        }
    }

    #[tool(description = "Builds the finished markdown document from the workspace: prose in order, then an evidence ledger of every non-withdrawn claim, then a Facts table with each fact's note and its CAPTURED extent, so a reader can compare a note with what was actually opened without having the workspace. Call `review` first and compare each paragraph with the claims it cites. With `out`, also writes the workspace snapshot to `<out>.tetel/` in the same step, and assigns the workspace identity if it has none; without a snapshot beside it a memo's citation ids resolve to nothing and `check` cannot tell self-grounding from independent grounding. Warns if captured output is still pending, never minted into a fact. `workspace` is required (never defaulted).")]
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

    #[tool(description = "Plain, greppable, read-only inspection of facts, claims, prose, or an id's dependencies. Never refuses. `workspace` is required (never defaulted); ids are workspace-relative only. A listing pages: it shows whole records up to the reply bound, each label and text cut to 1024 bytes, and when it leaves records out it leads with the `from` id to continue with. `id` with `facts` or `claims` returns that one record uncut, a fact's extents paged by `extent_from`; only a note, label or claim longer than a page by itself is cut, and the cut is stated.")]
    async fn query(&self, Parameters(p): Parameters<QueryParams>) -> Result<CallToolResult, ErrorData> {
        let dir = open_workspace(&p.workspace)?;
        let (id, from) = (p.id.as_deref(), p.from.as_deref());
        let refuse = |msg: &str| Err(ErrorData::invalid_params(msg.to_string(), None));
        if p.extent_from.is_some() && !(matches!(p.what, QueryWhat::Facts) && id.is_some()) {
            return refuse("query `extent_from` applies only with `id` on `facts`");
        }
        let q = match (p.what, id) {
            (QueryWhat::Facts | QueryWhat::Claims, Some(_)) if from.is_some() => {
                return refuse("query `from` pages a listing; it cannot be combined with `id`");
            }
            (QueryWhat::Facts, Some(id)) => query::Query::Fact { id, extent_from: p.extent_from },
            (QueryWhat::Facts, None) => query::Query::Facts { from },
            (QueryWhat::Claims, Some(id)) => query::Query::Claim { id },
            (QueryWhat::Claims, None) => query::Query::Claims { from },
            (QueryWhat::Prose, Some(_)) => return refuse("query `id` applies to `facts`, `claims` and `deps`"),
            (QueryWhat::Prose, None) => query::Query::Prose { from },
            (QueryWhat::Deps, Some(id)) => query::Query::Deps { id, from },
            (QueryWhat::Deps, None) => return refuse("query `deps` requires `id`"),
        };
        match query::text(&dir, q) {
            Ok(s) => text_result(s),
            Err(e) => Err(ErrorData::internal_error(format!("error querying: {e}"), None)),
        }
    }

    #[tool(description = "Lists each paragraph of the workspace's prose next to the claims it cites. Use it before `render --out` to compare each paragraph with its claims: a paragraph stating something none of its cited claims states is the error this view exists to expose, and no automatic check detects it. `workspace` is required (never defaulted).")]
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

    // The literal below is never served: `rmcp-macros` parses a `#[tool]`
    // attribute's `description` as a source-literal `String`
    // (`ToolAttribute::description: Option<String>`), never an expression,
    // so nothing computed can be spliced in at this call site. `Self::new`
    // overwrites this route's baked-in description with
    // `check_description()`'s output — built from the same
    // `report::MACHINE_CHECKED_CATEGORIES`/`HUMAN_OWED_CATEGORIES` this
    // file's own scope strings draw from — immediately after construction,
    // before the server is ever served. See `check_description` below.
    #[tool(description = "placeholder — see `check_description` in this file; `Self::new` replaces this text before the server is ever served")]
    async fn check(&self, Parameters(p): Parameters<CheckParams>) -> Result<CallToolResult, ErrorData> {
        match crate::check_file(&resolved(&p.file)) {
            Ok((code, report)) => {
                let block = vec![ContentBlock::text(report)];
                Ok(if code == crate::EXIT_CLEAN { CallToolResult::success(block) } else { CallToolResult::error(block) })
            }
            Err(e) => {
                let msg = format!("error reading {}: {e}", resolved(&p.file).display());
                reading_error("check", e, msg)
            }
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
    #[tool(description = "Print a memo's claims for grading (id and proposition only, without the memo's other text), or with `authoring: true`, the writing guide for the `look`/`run`/`fact`/`claim`/`prose`/`render` sequence.")]
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
        // The settings file is consulted here exactly as the CLI consults
        // it. The grounding floor is the setting the whole `config`
        // module was justified by, and grounding passes run through this
        // server — a floor that applied only to the CLI would be a
        // setting silently ignored on the surface it was written for.
        //
        // Global scope only: `brief` takes no workspace (it reads a memo
        // on disk), so there is no workspace file to resolve against.
        let floor = match p.confirm {
            Some(n) => n,
            None => crate::config::grounding_floor(None).0.unwrap_or(crate::brief::DEFAULT_FLOOR),
        };
        if floor == 0 {
            return Ok(CallToolResult::structured_error(json!({
                "error": "refused",
                "command": "brief",
                "guidance": "`confirm: 0` would leave nothing ever owed, which is the empty \
schedule a switched-off flag produces. The floor is at least 1.",
            })));
        }
        match crate::brief_file(&resolved(&memo), p.json, floor) {
            Ok((code, out)) => {
                let block = vec![ContentBlock::text(out)];
                Ok(if code == crate::EXIT_CLEAN { CallToolResult::success(block) } else { CallToolResult::error(block) })
            }
            Err(e) => {
                let msg = format!("error reading {memo}: {e}");
                reading_error("brief", e, msg)
            }
        }
    }

    #[tool(description = "Append one grading result to the memo's evidence log. Two paths: `from_fact` (witnessed — the extent is copied from a fact this workspace captured and cannot be supplied by hand, and the record carries the workspace identity so `check` can recompute whether a pass relied on output it captured itself) or `input` (ingested — extent and source supplied by hand, capped at attested standing). Prefer `from_fact` for anything captured through this server. Rejects an unknown claim id, an invalid verdict, a `qualifies` with no note, or malformed input, and never performs a partial write.")]
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
                Err(e) => {
                    let msg = format!("error reading {}: {e}", resolved(&p.memo).display());
                    reading_error("record", e, msg)
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
            Err(e) => {
                let msg = format!("error reading {}: {e}", resolved(&p.memo).display());
                reading_error("record", e, msg)
            }
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
    ///
    /// **Every outcome leaves through [`crate::reply::bound`]**, the
    /// refusal above included, for the same one-place reason: a reply over
    /// the budget is spilled to a file the caller cannot open, and a bound
    /// each verb had to remember would be broken by the next verb.
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, ErrorData> {
        let outcome = if let crate::buildid::Freshness::Stale { running, installed, path } =
            crate::buildid::freshness()
        {
            Ok(CallToolResult::structured_error(json!({
                "error": "refused",
                "command": request.name,
                "guidance": crate::buildid::stale_guidance(&running, &installed, &path),
                "running_build": running,
                "installed_build": installed,
                "binary": path,
            }))
            .into())
        } else {
            let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
            self.tool_router.call(tcc).await
        };
        crate::reply::bound(outcome)
    }

    /// Written out by hand for the same reason `call_tool` is: `#[tool_handler]`'s
    /// default `list_tools` calls the associated function `Self::tool_router()`
    /// fresh on every request, rather than reading the `self.tool_router`
    /// field — so a patch applied to *this instance's* router after
    /// construction (see `TetelServer::new`, which overwrites `check`'s
    /// placeholder description with `check_description()`'s output) would
    /// never reach a caller that lists tools, only one that calls them.
    /// Mirrors the generated body (`rmcp-macros` 3.1.1's `tool_handler.rs`)
    /// with that one substitution.
    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
        let supports_cache_hints = context
            .protocol_version()
            .is_some_and(|version| version >= rmcp::model::ProtocolVersion::V_2026_07_28);
        Ok(rmcp::model::ListToolsResult {
            result_type: Some(rmcp::model::ResultType::COMPLETE),
            tools: self.tool_router.list_all(),
            meta: None,
            next_cursor: None,
            ttl_ms: supports_cache_hints.then_some(0),
            cache_scope: supports_cache_hints.then_some(rmcp::model::CacheScope::Public),
        })
    }

    /// Same reason and same substitution as `list_tools` above: reads
    /// `self.tool_router` (this instance's, possibly patched) rather than
    /// reconstructing a fresh one via `Self::tool_router()`.
    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        self.tool_router.get(name).cloned()
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
