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

/// The witnessed path must exist over MCP, not only on the CLI.
///
/// K1's first grounding pass ran over the CLI because that is what its
/// prompt gave it, and its fact notes came back with every apostrophe
/// stripped — the shell-quoting damage this server exists to prevent. An
/// agent that can reach only `record`'s ingested path over MCP is an
/// agent that cannot produce witnessed evidence without a shell, which
/// would put the two properties in opposition.
#[tokio::test]
async fn record_from_fact_is_reachable_over_mcp_with_text_a_shell_would_damage() {
    let sb = Sandbox::new("record-from-fact");
    sb.write("alpha.rs", "fn alpha() {}\n");
    let client = sb.connect().await;

    let call = |tool: &'static str, a: serde_json::Value| {
        let c = &client;
        async move {
            c.call_tool(CallToolRequestParams::new(tool).with_arguments(args(a)))
                .await
                .unwrap_or_else(|e| panic!("{tool} must succeed: {e}"))
        }
    };

    call("look", serde_json::json!({"workspace": "w", "path": "alpha.rs"})).await;
    call("fact", serde_json::json!({"workspace": "w", "note": "alpha.rs defines alpha()"})).await;
    call(
        "claim",
        serde_json::json!({"workspace": "w", "proposition": "alpha.rs defines alpha()", "cites": "F1"}),
    )
    .await;
    call("prose", serde_json::json!({"workspace": "w", "text": "Defines alpha().", "cites": "C1"})).await;

    let memo = sb.dir.join("memo.md");
    call(
        "render",
        serde_json::json!({"workspace": "w", "out": memo.to_str().unwrap()}),
    )
    .await;

    // The note carries exactly what the CLI run lost: apostrophes in
    // possessives, plus backticks and an embedded newline.
    let note = "the parcel's own tree_hash, not `known_complete`'s\nsecond line";
    let result = call(
        "record",
        serde_json::json!({
            "memo": memo.to_str().unwrap(),
            "workspace": "w",
            "from_fact": "F1",
            "claim": "C1",
            "verdict": "qualifies",
            "note": note,
        }),
    )
    .await;

    let out = result.structured_content.expect("record must carry structured_content");
    assert_eq!(out["witnessed"], true, "got: {out}");
    assert!(out["pass"].as_str().is_some_and(|p| !p.is_empty()), "got: {out}");

    // Byte-exact through the whole path: MCP -> record -> jsonl.
    let raw = std::fs::read_to_string(sb.dir.join("memo.md.evidence.jsonl")).unwrap();
    let rec: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
    assert_eq!(rec["predicate"]["note"].as_str().unwrap(), note);
    assert_eq!(rec["predicateType"], tetel::evidence::CAPTURED_PREDICATE_TYPE);
    // And the extent came from the fact, not from any field a caller sent.
    assert_eq!(rec["predicate"]["extent"][0], "alpha.rs");

    client.cancel().await.expect("clean shutdown");
}

/// A bare `qualifies` is refused over MCP too, as a structured refusal
/// rather than a protocol error.
#[tokio::test]
async fn a_qualifies_with_no_note_is_refused_over_mcp() {
    let sb = Sandbox::new("mcp-bare-qualifies");
    sb.write("alpha.rs", "fn alpha() {}\n");
    let client = sb.connect().await;

    for (tool, a) in [
        ("look", serde_json::json!({"workspace": "w", "path": "alpha.rs"})),
        ("fact", serde_json::json!({"workspace": "w", "note": "alpha.rs defines alpha()"})),
        ("claim", serde_json::json!({"workspace": "w", "proposition": "alpha.rs defines alpha()", "cites": "F1"})),
        ("prose", serde_json::json!({"workspace": "w", "text": "Defines alpha().", "cites": "C1"})),
    ] {
        client.call_tool(CallToolRequestParams::new(tool).with_arguments(args(a))).await.unwrap();
    }
    let memo = sb.dir.join("memo.md");
    client
        .call_tool(CallToolRequestParams::new("render").with_arguments(args(
            serde_json::json!({"workspace": "w", "out": memo.to_str().unwrap()}),
        )))
        .await
        .unwrap();

    let result = client
        .call_tool(CallToolRequestParams::new("record").with_arguments(args(serde_json::json!({
            "memo": memo.to_str().unwrap(),
            "workspace": "w",
            "from_fact": "F1",
            "claim": "C1",
            "verdict": "qualifies",
        }))))
        .await
        .expect("the call must succeed at the protocol level");

    assert_eq!(result.is_error, Some(true), "a bare qualifies must be a tool-level refusal");
    let s = result.structured_content.as_ref().expect("structured refusal");
    assert!(
        s["guidance"].as_str().unwrap_or("").contains("needs a `note`"),
        "guidance was: {s}"
    );

    client.cancel().await.expect("clean shutdown");
}

/// A tool's published schema is a promise. `review` used to borrow
/// `render`'s parameters, so it advertised an `out` the handler silently
/// ignored — ask it to write a file and you got no file and no error.
/// Asserted against the schema the running server actually publishes,
/// not against the source, because the source is what drifted.
#[tokio::test]
async fn review_does_not_advertise_parameters_it_ignores() {
    let sb = Sandbox::new("schema-parity");
    let client = sb.connect().await;

    let tools = client.list_all_tools().await.expect("list tools");
    let review = tools.iter().find(|t| t.name == "review").expect("review tool must exist");
    let schema = serde_json::to_value(&review.input_schema).expect("schema serialises");
    let props = schema["properties"].as_object().expect("schema has properties");

    let mut names: Vec<&str> = props.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["workspace"],
        "review must advertise exactly what it reads; got: {names:?}"
    );

    client.cancel().await.expect("clean shutdown");
}

/// Every command the CLI offers is reachable over MCP. The two surfaces
/// have drifted twice: the note-vs-extent warning reached only the CLI,
/// and `record --from-fact` was CLI-only while the MCP server was being
/// recommended for the very run that needed witnessed records.
#[tokio::test]
async fn every_cli_subcommand_has_an_mcp_tool() {
    let sb = Sandbox::new("surface-parity");
    let client = sb.connect().await;

    let tools = client.list_all_tools().await.expect("list tools");
    let names: std::collections::HashSet<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    // `mcp` itself is the server, not a tool it can offer.
    for expected in [
        "look", "run", "fact", "claim", "prose", "render", "review", "query", "workspaces",
        "check", "brief", "record",
    ] {
        assert!(names.contains(expected), "no MCP tool for `{expected}`; have {names:?}");
    }

    client.cancel().await.expect("clean shutdown");
}

/// The server resolves paths against its own working directory, which
/// the caller cannot see or set — so a relative path silently means
/// something different to each side. That cost a run: an agent passed
/// relative memo paths and got "no tetel rows found", accurate about
/// what was read and useless for working out why.
///
/// Nothing here changes which file is opened. What it changes is that
/// every message naming a path names the resolved absolute one, so a
/// wrong directory diagnoses itself instead of reading as a fact about
/// the document.
#[tokio::test]
async fn a_relative_path_is_reported_back_as_the_absolute_one_it_resolved_to() {
    let sb = Sandbox::new("path-diagnostic");
    sb.write("plain.md", "# Just prose\n\nNo ledger here.\n");
    let client = sb.connect().await;

    let result = client
        .call_tool(CallToolRequestParams::new("check").with_arguments(args(
            serde_json::json!({"file": "plain.md"}),
        )))
        .await
        .expect("the call must succeed at the protocol level");

    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect();

    // The sandbox dir is the server's cwd, so the resolved path must name
    // it — that is what makes a wrong directory visible.
    let expected = sb.dir.join("plain.md");
    assert!(
        text.contains(&expected.display().to_string()),
        "message must name the resolved absolute path.\nexpected to contain: {}\ngot: {text}",
        expected.display()
    );
    assert!(!text.contains("in plain.md —"), "must not echo the bare relative path: {text}");

    client.cancel().await.expect("clean shutdown");
}

/// Every path-taking parameter must say so, since the rule cannot be
/// enforced — the server has no way to reject a relative path that
/// happens to resolve to a real file.
#[tokio::test]
async fn every_path_parameter_documents_that_it_wants_an_absolute_path() {
    let sb = Sandbox::new("path-docs");
    let client = sb.connect().await;
    let tools = client.list_all_tools().await.expect("list tools");

    for (tool, param) in [
        ("look", "path"),
        ("render", "out"),
        ("check", "file"),
        ("brief", "memo"),
        ("record", "memo"),
    ] {
        let t = tools.iter().find(|t| t.name == tool).unwrap_or_else(|| panic!("{tool} missing"));
        let schema = serde_json::to_value(&t.input_schema).expect("schema serialises");
        let desc = schema["properties"][param]["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool}.{param} has no description"))
            .to_lowercase();
        assert!(
            desc.contains("absolute"),
            "{tool}.{param} must tell the caller to pass an absolute path; got: {desc}"
        );
    }

    client.cancel().await.expect("clean shutdown");
}

/// A tool description is the only thing a model re-reads at every call,
/// and three of them had drifted from the behaviour by the time this was
/// written: `review` advertised a parameter it ignored, `record` rejected
/// a shape it should accept, and no path parameter said it wanted an
/// absolute path. This pins the load-bearing claims each description
/// makes to the code that has to keep them true — a weaker guarantee
/// than checking prose for accuracy, but it fails when the constant or
/// the returned field moves.
#[tokio::test]
async fn tool_descriptions_stay_tied_to_the_behaviour_they_promise() {
    let sb = Sandbox::new("desc-pins");
    let client = sb.connect().await;
    let tools = client.list_all_tools().await.expect("list tools");
    let desc = |name: &str| -> String {
        tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .description
            .clone()
            .unwrap_or_default()
            .to_string()
    };

    // `check` promises exit 2 for the no-rows state — and that it is not
    // a clean run, which is the distinction the code exists to keep.
    let c = desc("check");
    assert!(
        c.contains(&format!("Exit {}", tetel::EXIT_NO_ROWS)),
        "check must name the no-rows exit code, which is {}: {c}",
        tetel::EXIT_NO_ROWS
    );
    assert!(c.contains("NOT a clean run"), "check must say exit 2 is not clean: {c}");
    // And it must name both partitions, since the two-partition contract
    // is the whole output shape.
    assert!(c.contains("MACHINE-CHECKED") && c.contains("HUMAN-OWED"), "got: {c}");

    // `render` promises the snapshot suffix that `snapshot_path` decides.
    let r = desc("render");
    let suffix = tetel::snapshot::snapshot_path(std::path::Path::new("m.md"))
        .extension()
        .and_then(|e| e.to_str())
        .expect("snapshot path has an extension")
        .to_string();
    assert!(
        r.contains(&format!(".{suffix}/")),
        "render must name the snapshot suffix `.{suffix}/`: {r}"
    );

    // `claim` promises an overlap report; the handler returns it as a
    // field, so the promise and the payload move together.
    assert!(desc("claim").contains("OVERLAP REPORT"), "claim must explain its overlap output");

    // `record` promises the witnessed/ingested split.
    let rec = desc("record");
    assert!(rec.contains("from_fact") && rec.contains("input"), "record must name both paths: {rec}");
    assert!(rec.contains("witnessed"), "record must name the witnessed path: {rec}");

    // `run` must warn that captured output is permanent and ships.
    assert!(
        desc("run").contains("unrevisable") && desc("run").contains("snapshot"),
        "run must warn that its capture is permanent and ships with the memo"
    );

    client.cancel().await.expect("clean shutdown");
}

/// The CLI refuses `--lines` with `--grep` through clap; MCP silently
/// ignored `lines` and ran the grep, so one call meant different things
/// on the two surfaces.
#[tokio::test]
async fn look_refuses_lines_combined_with_grep_as_the_cli_does() {
    let sb = Sandbox::new("lines-grep");
    sb.write("a.rs", "fn a() {}\n");
    let client = sb.connect().await;

    let result = client
        .call_tool(CallToolRequestParams::new("look").with_arguments(args(serde_json::json!({
            "workspace": "w",
            "path": sb.dir.join("a.rs").to_str().unwrap(),
            "grep": "fn",
            "lines": {"start": 1, "end": 1},
        }))))
        .await
        .expect("the call must succeed at the protocol level");

    assert_eq!(result.is_error, Some(true), "the combination must be refused");
    let s = result.structured_content.as_ref().expect("structured refusal");
    assert!(s["guidance"].as_str().unwrap_or("").contains("cannot be combined"), "got: {s}");

    client.cancel().await.expect("clean shutdown");
}
