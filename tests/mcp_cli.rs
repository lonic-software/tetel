//! End-to-end tests for `tetel mcp`. Each test spawns the real `tetel
//! mcp` binary as a child process (never an in-process transport, and
//! never a bare call into `tetel::mcp::TetelServer`'s methods) and talks
//! to it exactly the way a real MCP client would: JSON-RPC over the
//! child's stdio. This is deliberate — the whole point of an MCP server
//! here is that arguments arrive as JSON with no shell in the path, and
//! only a real subprocess + stdio transport actually exercises that
//! path; a same-process function call would prove nothing about it.
//!
//! Mirrors `authoring_cli.rs`'s `Sandbox` pattern: each test gets a
//! private directory used as the child's working directory and, via
//! `TETEL_STATE_HOME`, as the root its workspace state lives under, so
//! tests never share state and never touch a real user's
//! `~/.local/state/tetel`.

use std::path::PathBuf;

use rmcp::model::{CallToolRequestParams, ClientInfo};
use rmcp::service::RunningService;
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientHandler, RoleClient, ServiceExt};

#[derive(Debug, Clone, Default)]
struct DummyClientHandler;

impl ClientHandler for DummyClientHandler {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tetel-mcp-cli-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Sandbox { dir }
    }

    fn state_home(&self) -> PathBuf {
        self.dir.join("state-home")
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
        path
    }

    fn facts_jsonl(&self, workspace: &str) -> String {
        std::fs::read_to_string(self.state_home().join("workspaces").join(workspace).join("facts.jsonl")).unwrap_or_default()
    }

    /// Spawn `tetel mcp` as a child process and complete the MCP
    /// initialise handshake against it — `ServiceExt::serve` performs
    /// the full `initialize`/`initialized` exchange before returning, so
    /// a successful `connect()` here *is* the handshake test.
    async fn connect(&self) -> RunningService<RoleClient, DummyClientHandler> {
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_tetel"));
        cmd.arg("mcp");
        cmd.current_dir(&self.dir);
        cmd.env("TETEL_STATE_HOME", self.state_home());
        let transport = TokioChildProcess::new(cmd).expect("failed to spawn `tetel mcp`");
        DummyClientHandler.serve(transport).await.expect("mcp initialise handshake failed")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn args(json: serde_json::Value) -> rmcp::model::JsonObject {
    json.as_object().expect("test arguments must be a JSON object").clone()
}

#[tokio::test]
async fn mcp_server_completes_the_initialise_handshake() {
    let sb = Sandbox::new("handshake");
    let client = sb.connect().await;

    // `connect()` already completed initialize/initialized (see its doc
    // comment); assert on what the server actually said about itself,
    // so this test fails loudly if the handshake ever starts responding
    // with the wrong identity instead of just "didn't crash".
    let peer_info = client.peer_info().expect("server must report its info after a successful handshake");
    let server_info = peer_info.server_info.as_ref().expect("server must identify itself during initialize");
    assert_eq!(server_info.name, "tetel");
    assert!(peer_info.capabilities.tools.is_some(), "server must advertise the tools capability");

    client.cancel().await.expect("clean shutdown");
}

#[tokio::test]
async fn fact_call_on_an_empty_buffer_returns_a_structured_error_not_a_crash() {
    let sb = Sandbox::new("empty-buffer");
    let client = sb.connect().await;

    let result = client
        .call_tool(CallToolRequestParams::new("fact").with_arguments(args(serde_json::json!({
            "workspace": "ws-empty",
            "note": "nothing was looked at",
        }))))
        .await
        .expect("the call itself must succeed at the protocol level");

    assert_eq!(result.is_error, Some(true), "minting with no prior look/run must be reported as a tool-level error");
    let structured = result.structured_content.as_ref().expect("refusal must carry structured_content, not just prose");
    assert_eq!(structured["error"], "refused");
    assert_eq!(structured["command"], "fact");
    assert_eq!(structured["workspace"], "ws-empty");
    let guidance = structured["guidance"].as_str().expect("guidance must be a string an agent can read directly");
    assert!(guidance.contains("pending observation buffer is empty"), "guidance was: {guidance}");

    assert!(sb.facts_jsonl("ws-empty").is_empty(), "a refused fact must not be logged");

    client.cancel().await.expect("clean shutdown");
}

#[tokio::test]
async fn note_with_backticks_quotes_newlines_and_a_trailing_space_round_trips_byte_exact() {
    // The entire reason this server exists: shell quoting has corrupted
    // exactly this shape of text on every inline CLI attempt across
    // three separate runs of this tool and its prototype. A tool call's
    // JSON arguments must carry it through completely untouched.
    let sb = Sandbox::new("byte-exact");
    sb.write("src/lib.rs", "content\n");
    let client = sb.connect().await;

    let look_result = client
        .call_tool(CallToolRequestParams::new("look").with_arguments(args(serde_json::json!({
            "workspace": "ws-byte-exact",
            "path": "src/lib.rs",
        }))))
        .await
        .expect("look must succeed");
    assert_ne!(look_result.is_error, Some(true), "look must not be refused: {look_result:?}");

    let note = "line one\nline two with `backticks`, a 'single-quoted' phrase\nline three ";
    assert!(note.ends_with(' '), "test setup: the note must end in a trailing space");

    let fact_result = client
        .call_tool(CallToolRequestParams::new("fact").with_arguments(args(serde_json::json!({
            "workspace": "ws-byte-exact",
            "note": note,
        }))))
        .await
        .expect("fact must succeed");
    assert_ne!(fact_result.is_error, Some(true), "fact must not be refused: {fact_result:?}");
    let structured = fact_result.structured_content.expect("a minted fact returns structured_content");
    let id = structured["id"].as_str().expect("minted fact must report its id").to_string();
    assert_eq!(structured["action"], "minted");

    let log = sb.facts_jsonl("ws-byte-exact");
    let first_line = log.lines().next().expect("facts.jsonl must have exactly the one minted event");
    let parsed: serde_json::Value = serde_json::from_str(first_line).expect("facts.jsonl line must be valid JSON");
    assert_eq!(parsed["id"], id);
    assert_eq!(
        parsed["note"].as_str().expect("note must be a JSON string"),
        note,
        "the note stored on disk must be byte-identical to what the tool call sent — \
         no shell, no re-escaping, nothing dropped or altered in transit"
    );

    client.cancel().await.expect("clean shutdown");
}

#[tokio::test]
async fn two_workspaces_do_not_share_a_pending_buffer() {
    let sb = Sandbox::new("workspace-isolation");
    sb.write("src/lib.rs", "content\n");
    let client = sb.connect().await;

    // `look` into workspace A only.
    let look_a = client
        .call_tool(CallToolRequestParams::new("look").with_arguments(args(serde_json::json!({
            "workspace": "workspace-a",
            "path": "src/lib.rs",
        }))))
        .await
        .expect("look must succeed");
    assert_ne!(look_a.is_error, Some(true), "look in workspace A must not be refused: {look_a:?}");

    // Minting into workspace A must succeed — it has an observation.
    let fact_a = client
        .call_tool(CallToolRequestParams::new("fact").with_arguments(args(serde_json::json!({
            "workspace": "workspace-a",
            "note": "a fact from workspace a's own look",
        }))))
        .await
        .expect("fact must succeed");
    assert_ne!(fact_a.is_error, Some(true), "workspace A must mint from its own pending buffer: {fact_a:?}");

    // Minting into workspace B, which never looked at anything, must be
    // refused — if it silently succeeded, it must have reached across
    // and consumed workspace A's pending buffer instead of its own.
    let fact_b = client
        .call_tool(CallToolRequestParams::new("fact").with_arguments(args(serde_json::json!({
            "workspace": "workspace-b",
            "note": "must not be able to see workspace a's observation",
        }))))
        .await
        .expect("the call itself must succeed at the protocol level");
    assert_eq!(fact_b.is_error, Some(true), "workspace B has no observations of its own and must refuse to mint");
    let structured = fact_b.structured_content.expect("refusal must be structured");
    assert_eq!(structured["workspace"], "workspace-b");
    assert!(
        structured["guidance"].as_str().unwrap_or_default().contains("pending observation buffer is empty"),
        "workspace B must not have inherited workspace A's buffer: {structured:?}"
    );

    assert!(!sb.facts_jsonl("workspace-a").is_empty(), "workspace A's fact must exist");
    assert!(sb.facts_jsonl("workspace-b").is_empty(), "workspace B must have minted nothing");

    client.cancel().await.expect("clean shutdown");
}

/// Helper: `look` at `path`, then `fact` with `note`, returning the
/// fact call's structured content.
async fn look_then_fact(
    client: &RunningService<RoleClient, DummyClientHandler>,
    ws: &str,
    path: &str,
    extra: serde_json::Value,
) -> serde_json::Value {
    client
        .call_tool(CallToolRequestParams::new("look").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "path": path,
        }))))
        .await
        .expect("look must succeed");

    let mut fact_args = serde_json::json!({ "workspace": ws });
    for (k, v) in extra.as_object().unwrap() {
        fact_args[k] = v.clone();
    }
    let result = client
        .call_tool(CallToolRequestParams::new("fact").with_arguments(args(fact_args)))
        .await
        .expect("fact must succeed");
    assert_ne!(result.is_error, Some(true), "fact must not be refused: {result:?}");
    result.structured_content.expect("fact must carry structured_content").clone()
}

/// The authoring surface agents actually use must carry the
/// note-vs-extent finding back to the author. A finding that only
/// reaches `check` only ever reaches the human reviewing the finished
/// memo — long after the note, the claim resting on it, and the prose
/// are all written, and long after the cheapest moment to fix it.
#[tokio::test]
async fn a_note_naming_an_unopened_file_comes_back_on_the_mint_result() {
    let sb = Sandbox::new("attention-mint");
    sb.write("read_me.rs", "fn a() {}\n");
    let client = sb.connect().await;

    let out = look_then_fact(
        &client,
        "ws-a",
        "read_me.rs",
        serde_json::json!({"note": "read_me.rs defines a(), and other_file.rs calls it exactly once"}),
    )
    .await;

    assert_eq!(out["action"], "minted");
    let attention = out["attention"].as_array().expect("result must carry an attention array");
    assert_eq!(attention.len(), 1, "expected one finding, got: {out}");
    assert_eq!(attention[0]["kind"], "note-outside-extent");
    assert_eq!(attention[0]["mentioned"], "other_file.rs");

    let guidance = attention[0]["guidance"].as_str().expect("guidance must be a string");
    assert!(guidance.contains("other_file.rs"), "guidance names the file: {guidance}");
    assert!(
        guidance.contains("look") && guidance.contains("revise"),
        "guidance must name both corrections an author can take: {guidance}"
    );

    client.cancel().await.expect("clean shutdown");
}

/// A clean note leaves the array empty rather than omitting the field,
/// so a caller can branch on it without first checking it exists.
#[tokio::test]
async fn a_note_within_its_extent_comes_back_with_an_empty_attention_array() {
    let sb = Sandbox::new("attention-clean");
    sb.write("read_me.rs", "fn a() {}\n");
    let client = sb.connect().await;

    let out = look_then_fact(
        &client,
        "ws-b",
        "read_me.rs",
        serde_json::json!({"note": "read_me.rs defines a()"}),
    )
    .await;

    assert_eq!(out["action"], "minted");
    assert_eq!(
        out["attention"].as_array().expect("field must exist even when empty").len(),
        0,
        "got: {out}"
    );

    client.cancel().await.expect("clean shutdown");
}

/// Editing a note is the obvious way to introduce this defect, so a
/// revision is checked exactly as a mint is.
#[tokio::test]
async fn revising_a_note_into_an_overreach_is_reported_too() {
    let sb = Sandbox::new("attention-revise");
    sb.write("read_me.rs", "fn a() {}\n");
    let client = sb.connect().await;

    let minted = look_then_fact(
        &client,
        "ws-c",
        "read_me.rs",
        serde_json::json!({"note": "read_me.rs defines a()"}),
    )
    .await;
    assert_eq!(minted["attention"].as_array().unwrap().len(), 0, "clean at mint: {minted}");

    let result = client
        .call_tool(CallToolRequestParams::new("fact").with_arguments(args(serde_json::json!({
            "workspace": "ws-c",
            "revise": "F1",
            "why": "adding what I concluded",
            "note": "read_me.rs defines a(), which other_file.rs never calls",
        }))))
        .await
        .expect("revise must succeed");
    let out = result.structured_content.expect("revise must carry structured_content");

    assert_eq!(out["action"], "revised");
    let attention = out["attention"].as_array().unwrap();
    assert_eq!(attention.len(), 1, "a revised note must be checked too: {out}");
    assert_eq!(attention[0]["mentioned"], "other_file.rs");

    client.cancel().await.expect("clean shutdown");
}
