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

    /// A config home inside the sandbox, so a developer's own
    /// `~/.config/tetel` cannot decide what these tests measure — most
    /// sharply `verify.enabled`, which would have the suite making
    /// provider calls on someone's key.
    fn config_home(&self) -> PathBuf {
        self.dir.join("config-home")
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
        cmd.env("TETEL_STATE_HOME", self.state_home()).env("TETEL_CONFIG_HOME", self.config_home());
        let transport = TokioChildProcess::new(cmd).expect("failed to spawn `tetel mcp`");
        DummyClientHandler.serve(transport).await.expect("mcp initialise handshake failed")
    }

    /// `connect`, with every provider credential removed from the child's
    /// environment, for a test that turns verification on and must not
    /// have a developer's key start real provider calls.
    async fn connect_keyless(&self) -> RunningService<RoleClient, DummyClientHandler> {
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_tetel"));
        cmd.arg("mcp");
        cmd.current_dir(&self.dir);
        cmd.env("TETEL_STATE_HOME", self.state_home()).env("TETEL_CONFIG_HOME", self.config_home());
        for key in ["OPENROUTER_API_KEY", "TETEL_API_KEY", "TYPESAFE_API_KEY"] {
            cmd.env_remove(key);
        }
        let transport = TokioChildProcess::new(cmd).expect("failed to spawn `tetel mcp`");
        DummyClientHandler.serve(transport).await.expect("mcp initialise handshake failed")
    }

    /// Spawn `tetel mcp` from a *copy* of the test binary placed inside
    /// this sandbox, and return the path it was launched from.
    ///
    /// The staleness tests need a binary they are allowed to replace
    /// underneath a running process; `CARGO_BIN_EXE_tetel` is cargo's own
    /// build output and must never be rewritten by a test.
    async fn connect_from_a_copy(
        &self,
    ) -> (PathBuf, RunningService<RoleClient, DummyClientHandler>) {
        let exe = self.dir.join("bin").join("tetel");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        // `fs::copy` carries the permission bits on Unix, so the copy is
        // executable without a separate chmod.
        std::fs::copy(env!("CARGO_BIN_EXE_tetel"), &exe).expect("failed to copy the test binary");

        let mut cmd = tokio::process::Command::new(&exe);
        cmd.arg("mcp");
        cmd.current_dir(&self.dir);
        cmd.env("TETEL_STATE_HOME", self.state_home()).env("TETEL_CONFIG_HOME", self.config_home());
        let transport = TokioChildProcess::new(cmd).expect("failed to spawn the copied `tetel mcp`");
        let client = DummyClientHandler
            .serve(transport)
            .await
            .expect("mcp initialise handshake failed against the copied binary");
        (exe, client)
    }
}

/// Replace `exe` the way `cargo install` does — write the new content
/// beside it and `rename` over the top — rather than by writing through
/// the existing file, which the kernel refuses for a running executable
/// (`ETXTBSY`) and which would not reproduce the defect anyway. The
/// rename is the whole mechanism: it gives the path a new inode and
/// leaves the running process holding the old one.
fn replace_by_rename(exe: &std::path::Path, content: &[u8]) {
    let staged = exe.with_extension("staged");
    std::fs::write(&staged, content).expect("failed to stage the replacement binary");
    let perms = std::fs::metadata(exe).unwrap().permissions();
    std::fs::set_permissions(&staged, perms).unwrap();
    std::fs::rename(&staged, exe).expect("failed to rename the replacement over the original");
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

    // Read the CLI's own subcommand list rather than restating it. A
    // hand-maintained list here asserted only that twelve named tools
    // existed, so a thirteenth subcommand could be added CLI-only and
    // this test would still pass — the paired-artifact drift it exists
    // to catch, in the guard itself.
    let help = std::process::Command::new(env!("CARGO_BIN_EXE_tetel"))
        .arg("--help")
        .output()
        .expect("tetel --help must run");
    let help = String::from_utf8_lossy(&help.stdout);
    let subcommands: Vec<String> = help
        .lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| !l.trim().is_empty() && l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_string)
        // `mcp` is the server, not a tool it can offer; `help` is clap's.
        //
        // `config` is withheld deliberately, and the reason is not
        // convenience. The one setting it carries is the grounding
        // floor, which decides how many claims a pass is asked to
        // grade — so an author with access to it could shorten the list
        // it is about to be graded on. Settings belong to the person, are
        // set from their terminal, and are visible in the output they
        // affect (`brief` prints the floor it used). Nothing an agent
        // needs to *read* is hidden by withholding the verb; what is
        // withheld is the ability to change the rules mid-run.
        //
        // `verify-report` is withheld for a sharper version of the same
        // reason. It joins the verifier's flags to the verdicts a
        // grading pass later reached — so it reads the graders' output,
        // which `brief` withholds from the author on purpose (scope
        // withheld, ids and propositions only). Handing an authoring
        // agent a tool that reports what the graders concluded would
        // undo that withholding, and would additionally tell it how
        // often the verifier is wrong, which is a calibration for
        // ignoring it. It is an analysis command for the person, run
        // from their terminal, over a machine that holds the workspace.
        .filter(|c| c != "mcp" && c != "help" && c != "config" && c != "verify-report")
        .collect();

    assert!(
        subcommands.len() >= 12,
        "premise: the CLI's subcommand list must have parsed, got {subcommands:?}"
    );
    for expected in &subcommands {
        assert!(
            names.contains(expected.as_str()),
            "no MCP tool for `{expected}`; CLI offers {subcommands:?}, MCP offers {names:?}"
        );
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

/// `check_file` reads its memo through `workspace::read_caller_path`
/// (TET-79), which refuses a FIFO by setting `io::ErrorKind::
/// InvalidInput` — before this fix, `check`'s MCP handler mapped every
/// `Err` from that read straight to `ErrorData::internal_error`, so the
/// same defect that gets a structured refusal on `look` surfaced here as
/// an opaque protocol-level error instead: `client.call_tool(...).await`
/// itself would come back `Err(...)`, which is what
/// `.expect("...protocol level")` below turns into a red assertion
/// rather than a silent pass. `#[cfg(unix)]`: `mkfifo` is POSIX-specific,
/// and CI (`ci.yml`) runs only macOS/Ubuntu.
#[tokio::test]
#[cfg(unix)]
async fn check_on_a_fifo_returns_a_refusal_not_a_protocol_error() {
    let sb = Sandbox::new("check-fifo-mcp");
    let client = sb.connect().await;

    let fifo = sb.dir.join("pipe.md");
    let status = std::process::Command::new("mkfifo").arg(&fifo).status().expect("failed to run mkfifo");
    assert!(status.success(), "mkfifo failed");

    let result = client
        .call_tool(CallToolRequestParams::new("check").with_arguments(args(serde_json::json!({
            "file": fifo.display().to_string(),
        }))))
        .await
        .expect("a refusal must succeed at the protocol level, not surface as an internal error");

    assert_eq!(result.is_error, Some(true), "a FIFO must be reported as a tool-level error: {result:?}");
    let structured = result.structured_content.as_ref().expect("refusal must carry structured_content, not just prose");
    assert_eq!(structured["error"], "refused");
    assert_eq!(structured["command"], "check");
    let guidance = structured["guidance"].as_str().expect("guidance must be a string an agent can read directly");
    assert!(guidance.contains("FIFO"), "guidance must name what the path is: {guidance}");

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
    // Set equality against `report.rs`'s own category constants, not just
    // the two partition headers above. Before this, a category could ship
    // into `report::MACHINE_CHECKED_CATEGORIES`/`HUMAN_OWED_CATEGORIES`
    // (or the hand-typed lists these replaced) and be missing from this
    // description forever without reddening anything — the headers stayed
    // present the whole time. This reads the category names from an
    // independent source (the constant, not a parse of `c` itself — see
    // that constant's own doc comment on why the enforcer must not share
    // the parser it is checking) and checks each is a literal substring of
    // the description the live server actually served.
    for category in tetel::report::MACHINE_CHECKED_CATEGORIES {
        assert!(
            c.contains(category),
            "check description is missing machine-checked category {category:?}: {c}"
        );
    }
    for category in tetel::report::HUMAN_OWED_CATEGORIES {
        assert!(
            c.contains(category),
            "check description is missing human-owed category {category:?}: {c}"
        );
    }

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

    // `fact` and `claim` both spell out the `verify` status vocabulary.
    // `fact` used to point at `claim`'s description instead, which the
    // grounder and attacker agents do not carry, and `claim`'s list had
    // left out `skipped`.
    for verb in ["fact", "claim"] {
        let d = desc(verb);
        for status in tetel::verify::Status::ALL {
            let word = format!("`{}`", status.as_str());
            assert!(d.contains(&word), "{verb} description is missing verify status {word}: {d}");
        }
    }

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

/// A memo authored entirely over MCP must ship an identity in its
/// snapshot, so `check` can tell self-grounding from independent
/// grounding.
///
/// This is the regression test for a real defect: the CLI's `render` arm
/// minted the workspace identity before snapshotting and the MCP handler
/// did not, so every memo produced by an agent — which reaches tetel only
/// through this surface — shipped without one. The distinction the whole
/// mechanism exists for (78% scope-equal self-grounded against 33%
/// independent) was silently unavailable for exactly the population that
/// needed it, and grounding workspaces got an identity anyway via
/// `record`, which hid the asymmetry from the side most likely to be
/// inspected.
///
/// It asserts the shipped artifact, not the call's success: the bug was
/// never a failing call.
#[tokio::test]
async fn a_memo_authored_over_mcp_ships_an_identity_in_its_snapshot() {
    let sb = Sandbox::new("mcp-identity");
    sb.write("alpha.rs", "fn alpha() {}\n");
    let client = sb.connect().await;

    for (tool, params) in [
        ("look", serde_json::json!({"workspace": "ws-id", "path": "alpha.rs"})),
        ("fact", serde_json::json!({"workspace": "ws-id", "note": "alpha.rs defines alpha()"})),
        ("claim", serde_json::json!({"workspace": "ws-id", "proposition": "alpha.rs defines alpha()", "cites": "F1"})),
        ("prose", serde_json::json!({"workspace": "ws-id", "text": "It defines alpha().", "cites": "C1"})),
    ] {
        let r = client
            .call_tool(CallToolRequestParams::new(tool).with_arguments(args(params)))
            .await
            .unwrap_or_else(|e| panic!("{tool} call failed at protocol level: {e}"));
        assert_ne!(r.is_error, Some(true), "{tool} was refused: {:?}", r.structured_content);
    }

    let memo = sb.dir.join("memo.md");
    let r = client
        .call_tool(CallToolRequestParams::new("render").with_arguments(args(serde_json::json!({
            "workspace": "ws-id",
            "out": memo.to_str().unwrap(),
        }))))
        .await
        .expect("render call failed at protocol level");
    assert_ne!(r.is_error, Some(true), "render was refused: {:?}", r.structured_content);

    // The workspace itself must have an identity...
    assert!(
        sb.state_home().join("workspaces/ws-id/identity.json").is_file(),
        "MCP authoring must mint a workspace identity"
    );
    // ...and, the part that actually shipped wrong, the snapshot must
    // carry it. A snapshot is written file-by-file and skips what the
    // workspace lacks, so the workspace having one does not imply this.
    let snapshot_identity = sb.dir.join("memo.md.tetel/identity.json");
    assert!(
        snapshot_identity.is_file(),
        "the snapshot beside an MCP-rendered memo must carry identity.json"
    );

    // And the end the user sees: `check` can now speak to independence
    // rather than declining to.
    let (_c, report) = {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_tetel"))
            .arg("check")
            .arg(&memo)
            .env("TETEL_STATE_HOME", sb.state_home()).env("TETEL_CONFIG_HOME", sb.config_home())
            .output()
            .expect("check must run");
        (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
    };
    assert!(
        !report.contains("cannot be determined from here"),
        "check must be able to determine authorship from an MCP-rendered snapshot:\n{report}"
    );

    client.cancel().await.expect("clean shutdown");
}

/// A refusal on the MCP surface must land in the shipped record, and the
/// next mint must replay it — the same two properties the CLI has.
///
/// Both halves were broken here. The `lines`+`grep` conflict was refused
/// before the workspace was opened, so it could never reach
/// `workspace::refuse`; and nothing read the log back. An agent authoring
/// over MCP is the population this matters most for, since it has no
/// terminal to have glanced at.
#[tokio::test]
async fn a_refused_look_is_recorded_and_replayed_on_the_next_mint() {
    let sb = Sandbox::new("mcp-refusal-replay");
    sb.write("a.rs", "fn a() {}\n");
    let client = sb.connect().await;

    // Seed a leftover observation, then get a look refused.
    let r = client
        .call_tool(CallToolRequestParams::new("run").with_arguments(args(serde_json::json!({
            "workspace": "ws-ref",
            "command": ["echo", "leftover"],
        }))))
        .await
        .expect("run must succeed at protocol level");
    assert_ne!(r.is_error, Some(true), "run was refused: {:?}", r.structured_content);

    let r = client
        .call_tool(CallToolRequestParams::new("look").with_arguments(args(serde_json::json!({
            "workspace": "ws-ref",
            "path": "a.rs",
            "lines": {"start": 1, "end": 5},
            "grep": "fn",
        }))))
        .await
        .expect("the call itself must succeed at the protocol level");
    assert_eq!(r.is_error, Some(true), "lines+grep must be refused");

    // It reached the choke point: the shipped record has it.
    let log = std::fs::read_to_string(
        sb.state_home().join("workspaces/ws-ref/refusals.log"),
    )
    .expect("a refused look must be recorded in refusals.log");
    assert!(log.contains("look"), "got: {log}");
    assert!(log.contains("cannot be combined"), "got: {log}");

    // And the next mint replays it beside what it folded.
    let r = client
        .call_tool(CallToolRequestParams::new("fact").with_arguments(args(serde_json::json!({
            "workspace": "ws-ref",
            "note": "a.rs defines one function",
        }))))
        .await
        .expect("fact must succeed at protocol level");
    assert_ne!(r.is_error, Some(true), "fact was refused: {:?}", r.structured_content);
    let s = r.structured_content.as_ref().expect("mint must return structured content");
    let replayed = s["refused_since_previous_fact"]
        .as_array()
        .expect("the mint result must carry the refusal replay as an array");
    assert_eq!(replayed.len(), 1, "got: {s}");
    assert!(
        replayed[0].as_str().unwrap_or_default().contains("cannot be combined"),
        "the refusal must be replayed verbatim: {s}"
    );
    // And it still says what it folded — the two are complementary.
    assert!(
        s["folded"].as_array().is_some_and(|f| !f.is_empty()),
        "the folding description must survive: {s}"
    );

    client.cancel().await.expect("clean shutdown");
}

// --- TET-31: a stale server must refuse rather than answer -------------
//
// Both directions, because a detector that never fires is the same
// failure one level up from the one it was built to fix.

#[tokio::test]
async fn a_server_whose_binary_was_replaced_refuses_every_tool() {
    let sb = Sandbox::new("stale-server");
    let (exe, client) = sb.connect_from_a_copy().await;

    // Premise first: this server answers normally *before* the swap, so
    // a refusal afterwards is attributable to the swap and not to having
    // been launched from a copy.
    let before = client
        .call_tool(CallToolRequestParams::new("workspaces").with_arguments(args(serde_json::json!({}))))
        .await
        .expect("the call itself must succeed at the protocol level");
    assert_ne!(before.is_error, Some(true), "the copied server must work before the swap: {before:?}");

    // Now do what `cargo install` does to a running server.
    let mut replacement = std::fs::read(env!("CARGO_BIN_EXE_tetel")).unwrap();
    replacement.extend_from_slice(b"\n// a different build\n");
    replace_by_rename(&exe, &replacement);

    let after = client
        .call_tool(CallToolRequestParams::new("workspaces").with_arguments(args(serde_json::json!({}))))
        .await
        .expect("a stale server must still speak the protocol — it refuses, it does not crash");

    assert_eq!(after.is_error, Some(true), "a stale server must refuse: {after:?}");
    let structured = after
        .structured_content
        .as_ref()
        .expect("the staleness refusal must be structured, not prose an agent has to pattern-match");
    assert_eq!(structured["error"], "refused");
    assert_eq!(structured["command"], "workspaces", "the refusal must name the tool that was called");
    assert_eq!(structured["binary"], exe.display().to_string());
    assert_ne!(
        structured["running_build"], structured["installed_build"],
        "the two builds must be reported separately and must differ: {structured}"
    );
    let guidance = structured["guidance"].as_str().expect("guidance must be a readable string");
    assert!(guidance.contains("no longer installed"), "guidance was: {guidance}");
    assert!(guidance.contains("Restart"), "the remedy must be named: {guidance}");

    // The gate sits in dispatch, so it covers the verdict-producing
    // tools too — which is the whole reason it exists.
    let memo = sb.write("stale.md", "# nothing\n");
    let checked = client
        .call_tool(CallToolRequestParams::new("check").with_arguments(args(serde_json::json!({
            "file": memo.display().to_string(),
        }))))
        .await
        .expect("protocol level");
    assert_eq!(checked.is_error, Some(true), "a stale server must not return a verdict: {checked:?}");
    assert_eq!(
        checked.structured_content.as_ref().expect("structured")["error"],
        "refused"
    );

    client.cancel().await.expect("clean shutdown");
}

#[tokio::test]
async fn reinstalling_the_identical_build_is_not_staleness() {
    let sb = Sandbox::new("same-build-reinstall");
    let (exe, client) = sb.connect_from_a_copy().await;

    // A rename-replace with byte-identical content: new inode, new
    // mtime, same build. A detector keyed on the file's identity rather
    // than its content would call this stale, and a refusal that fires
    // when nothing changed is a refusal that gets worked around.
    let identical = std::fs::read(env!("CARGO_BIN_EXE_tetel")).unwrap();
    replace_by_rename(&exe, &identical);

    let after = client
        .call_tool(CallToolRequestParams::new("workspaces").with_arguments(args(serde_json::json!({}))))
        .await
        .expect("protocol level");
    assert_ne!(
        after.is_error,
        Some(true),
        "re-installing the same build must not be reported as staleness: {after:?}"
    );

    client.cancel().await.expect("clean shutdown");
}

#[tokio::test]
async fn check_names_the_build_that_graded_it() {
    let sb = Sandbox::new("check-names-build");
    let client = sb.connect().await;

    let memo = sb.write("named.md", "# nothing here\n\njust prose.\n");
    let result = client
        .call_tool(CallToolRequestParams::new("check").with_arguments(args(serde_json::json!({
            "file": memo.display().to_string(),
        }))))
        .await
        .expect("protocol level");

    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("");
    // Even the no-rows state names its checker: two runs disagreeing
    // about whether a file is in scope is exactly the kind of dispute
    // that needs attributing to a build.
    assert!(text.contains("checked by tetel "), "check output must name its build: {text}");

    client.cancel().await.expect("clean shutdown");
}

/// The completeness refusal must reach the MCP surface too.
///
/// Not paranoia about a shared function: this exact render path has
/// already shipped a CLI/MCP divergence, where the CLI minted a workspace
/// identity before rendering and the MCP handler did not, so every memo
/// authored by an agent shipped a snapshot without one. The two front
/// ends now call one function; this is what proves the MCP side calls it.
#[tokio::test]
async fn render_over_mcp_refuses_a_document_with_an_unanswered_premise() {
    let sb = Sandbox::new("mcp-premise-completeness");
    // A census needs a real worktree to be rooted at.
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&sb.dir)
                .args(args)
                .output()
                .expect("git must be on PATH for this test")
                .status
                .success(),
            "git {args:?} failed"
        );
    };
    git(&["init", "-q"]);
    sb.write("donor.rs", "fn walk() {\n    // sound only if walked earlier in this same session\n}\n");
    sb.write("dest.rs", "fn audit() { walk(); }\n");
    git(&["add", "-A"]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"]);

    let client = sb.connect().await;
    let ws = "ws-premise";
    let root = sb.dir.to_str().unwrap().to_string();
    let donor = sb.dir.join("donor.rs").to_str().unwrap().to_string();

    for (tool, params) in [
        ("look", serde_json::json!({"workspace": ws, "path": donor})),
        ("fact", serde_json::json!({"workspace": ws, "note": "the donor's walk discipline"})),
        ("look", serde_json::json!({"workspace": ws, "path": root, "grep": "walk"})),
        ("fact", serde_json::json!({"workspace": ws, "note": "every use of walk"})),
        ("target", serde_json::json!({"workspace": ws, "symbol": "walk", "cites": "F2"})),
        ("transplant", serde_json::json!({"workspace": ws, "from": "F1", "into": "T1"})),
        ("claim", serde_json::json!({"workspace": ws, "proposition": "the order carries over", "cites": "F1"})),
        ("prose", serde_json::json!({"workspace": ws, "text": "It carries over. See [C1]."})),
        // Selected from the donor's captured bytes, and not yet answered.
        ("transplant", serde_json::json!({"workspace": ws, "premise": "X1", "text": "walked earlier in this same session"})),
    ] {
        let r = client
            .call_tool(CallToolRequestParams::new(tool).with_arguments(args(params)))
            .await
            .unwrap_or_else(|e| panic!("{tool} call failed at protocol level: {e}"));
        assert_ne!(r.is_error, Some(true), "{tool} was refused: {:?}", r.structured_content);
    }

    let memo = sb.dir.join("memo.md");
    let r = client
        .call_tool(CallToolRequestParams::new("render").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "out": memo.to_str().unwrap(),
        }))))
        .await
        .expect("render call failed at protocol level");
    let body = format!("{:?}{:?}", r.content, r.structured_content);
    assert!(
        body.contains("X1.1"),
        "MCP render --out must refuse a document with an unanswered premise, naming it: {body}"
    );
    assert!(
        !memo.exists(),
        "the refused document must not have been written over MCP either"
    );

    client.cancel().await.expect("clean shutdown");
}

// --- TET-61: `prose --ack` over MCP -------------------------------------

/// Mints a paragraph `P1` in workspace `ws` citing a real claim `C1` —
/// the minimum a `prose --ack` call needs to have something to
/// acknowledge.
async fn mint_a_citable_block(sb: &Sandbox, client: &RunningService<RoleClient, DummyClientHandler>, ws: &str) {
    sb.write(&format!("{ws}.rs"), "fn thing() {}\n");
    let file = sb.dir.join(format!("{ws}.rs")).to_str().unwrap().to_string();
    let call = |tool: &'static str, a: serde_json::Value| {
        let c = client;
        async move {
            c.call_tool(CallToolRequestParams::new(tool).with_arguments(args(a)))
                .await
                .unwrap_or_else(|e| panic!("{tool} must succeed: {e}"))
        }
    };
    call("look", serde_json::json!({"workspace": ws, "path": file})).await;
    call("fact", serde_json::json!({"workspace": ws, "note": "a fact to rest a claim on"})).await;
    call("claim", serde_json::json!({"workspace": ws, "proposition": "a claim", "cites": "F1"})).await;
    call("prose", serde_json::json!({"workspace": ws, "text": "A paragraph.", "cites": "C1"})).await;
}

/// C12's own defect, positive side: before this design, `ProseParams`
/// declared `text: String` with no `Option`, so any prose call omitting
/// it — including every `ack`, which carries none — was refused by
/// deserialisation before the handler, and therefore before
/// `workspace::refuse`, ever ran. Relaxing the field to `Option<String>`
/// is what lets an `ack` call reach the handler at all; this pins that it
/// does, and that it succeeds.
#[tokio::test]
async fn ack_over_mcp_succeeds_without_a_text_field() {
    let sb = Sandbox::new("mcp-ack-no-text");
    let client = sb.connect().await;
    mint_a_citable_block(&sb, &client, "w").await;

    let result = client
        .call_tool(CallToolRequestParams::new("prose").with_arguments(args(serde_json::json!({
            "workspace": "w",
            "ack": "P1",
            "why": "re-read against C1, still accurate",
        }))))
        .await
        .expect("an ack call must reach the handler rather than fail deserialisation");
    assert_ne!(result.is_error, Some(true), "got: {:?}", result.structured_content);
    let out = result.structured_content.expect("ack must carry structured_content");
    assert_eq!(out["action"], "acknowledged");
    assert_eq!(out["id"], "P1");

    client.cancel().await.expect("clean shutdown");
}

/// The other half of relaxing `text`: its absence for every *other* mode
/// (not `ack`) must now be refused in code — the schema no longer does
/// it for us. Refused rather than silently falling back to anything,
/// since there is no stdin to fall back to over MCP.
#[tokio::test]
async fn mcp_prose_with_no_text_and_no_ack_is_refused_in_code() {
    let sb = Sandbox::new("mcp-prose-no-text");
    let client = sb.connect().await;

    let result = client
        .call_tool(CallToolRequestParams::new("prose").with_arguments(args(serde_json::json!({
            "workspace": "w",
        }))))
        .await
        .expect("a missing `text` must reach the handler, not fail deserialisation");
    assert_eq!(result.is_error, Some(true), "got: {:?}", result.structured_content);
    let s = result.structured_content.expect("structured refusal");
    assert!(
        s["guidance"].as_str().unwrap_or("").contains("requires text"),
        "guidance was: {s}"
    );

    client.cancel().await.expect("clean shutdown");
}

/// C12's own worked example: an ack combined with `text` must be refused
/// explicitly, not silently discarded by the mode-selection chain.
#[tokio::test]
async fn ack_combined_with_text_is_refused_over_mcp() {
    let sb = Sandbox::new("mcp-ack-text-conflict");
    let client = sb.connect().await;
    mint_a_citable_block(&sb, &client, "w").await;

    let result = client
        .call_tool(CallToolRequestParams::new("prose").with_arguments(args(serde_json::json!({
            "workspace": "w",
            "ack": "P1",
            "why": "x",
            "text": "should never be read",
        }))))
        .await
        .expect("protocol level");
    assert_eq!(result.is_error, Some(true), "got: {:?}", result.structured_content);
    let s = result.structured_content.expect("structured refusal");
    assert!(
        s["guidance"].as_str().unwrap_or("").contains("cannot be combined with --text"),
        "guidance was: {s}"
    );

    client.cancel().await.expect("clean shutdown");
}

/// The member the design memo says an enumeration written against the
/// MCP shape alone would miss: the CLI has independent `--heading` and
/// `--level` flags, but `ProseParams` folds both into one
/// `heading_level`, so this single field stands in for refusing *both*
/// on the MCP side.
#[tokio::test]
async fn ack_combined_with_heading_level_is_refused_over_mcp() {
    let sb = Sandbox::new("mcp-ack-level-conflict");
    let client = sb.connect().await;
    mint_a_citable_block(&sb, &client, "w").await;

    let result = client
        .call_tool(CallToolRequestParams::new("prose").with_arguments(args(serde_json::json!({
            "workspace": "w",
            "ack": "P1",
            "why": "x",
            "heading_level": 2,
        }))))
        .await
        .expect("protocol level");
    assert_eq!(result.is_error, Some(true), "got: {:?}", result.structured_content);
    let s = result.structured_content.expect("structured refusal");
    assert!(
        s["guidance"].as_str().unwrap_or("").contains("cannot be combined with --level"),
        "guidance was: {s}"
    );

    client.cancel().await.expect("clean shutdown");
}

/// The remaining three of the six: `revise`, `cites` and `before`, each
/// refused independently rather than silently dropped.
#[tokio::test]
async fn ack_combined_with_revise_cites_or_before_is_refused_over_mcp() {
    let sb = Sandbox::new("mcp-ack-other-conflicts");
    let client = sb.connect().await;
    mint_a_citable_block(&sb, &client, "w").await;

    for (field, value, needle) in [
        ("revise", serde_json::json!("P1"), "cannot be combined with --revise"),
        ("cites", serde_json::json!("C1"), "cannot be combined with --cites"),
        ("before", serde_json::json!("P1"), "cannot be combined with --before"),
    ] {
        let mut a = serde_json::json!({"workspace": "w", "ack": "P1", "why": "x"});
        a.as_object_mut().unwrap().insert(field.to_string(), value);
        let result = client
            .call_tool(CallToolRequestParams::new("prose").with_arguments(args(a)))
            .await
            .expect("protocol level");
        assert_eq!(result.is_error, Some(true), "{field} must be refused when combined with ack");
        let s = result.structured_content.expect("structured refusal");
        assert!(
            s["guidance"].as_str().unwrap_or("").contains(needle),
            "{field}: guidance was: {s}"
        );
    }

    client.cancel().await.expect("clean shutdown");
}

// --- TET-51: the overlap report ships keys, not notes -------------------

/// `look` at `path`, then `fact --note note` — fire-and-forget, panicking
/// on any refusal. Multiple calls before the same `fact` fold into one
/// mint's extent, which is exactly what the multi-key tests below need.
async fn look(client: &RunningService<RoleClient, DummyClientHandler>, ws: &str, path: &str) {
    let r = client
        .call_tool(CallToolRequestParams::new("look").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "path": path,
        }))))
        .await
        .expect("look call failed at protocol level");
    assert_ne!(r.is_error, Some(true), "look was refused: {:?}", r.structured_content);
}

async fn fact(client: &RunningService<RoleClient, DummyClientHandler>, ws: &str, note: &str) {
    let r = client
        .call_tool(CallToolRequestParams::new("fact").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "note": note,
        }))))
        .await
        .expect("fact call failed at protocol level");
    assert_ne!(r.is_error, Some(true), "fact was refused: {:?}", r.structured_content);
}

async fn create_claim(
    client: &RunningService<RoleClient, DummyClientHandler>,
    ws: &str,
    prop: &str,
    cites: &str,
) -> serde_json::Value {
    let r = client
        .call_tool(CallToolRequestParams::new("claim").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "proposition": prop,
            "cites": cites,
        }))))
        .await
        .expect("claim call failed at protocol level");
    assert_ne!(r.is_error, Some(true), "claim was refused: {:?}", r.structured_content);
    r.structured_content.expect("a created claim must carry structured_content")
}

/// Same intersection-precision test as the CLI's
/// `overlap_report_names_only_the_key_actually_shared_not_every_key_the_fact_touches`,
/// over MCP: F2 touches both a.rs and b.rs, F1 cites only a.rs, and the
/// overlap entry for F2 must carry a.rs and must not carry b.rs.
#[tokio::test]
async fn overlap_report_over_mcp_names_only_the_shared_key() {
    let sb = Sandbox::new("mcp-overlap-key-precision");
    sb.write("a.rs", "fn a() {}\n");
    sb.write("b.rs", "fn b() {}\n");
    let client = sb.connect().await;
    let ws = "ws";
    let a = sb.dir.join("a.rs").to_str().unwrap().to_string();
    let b = sb.dir.join("b.rs").to_str().unwrap().to_string();

    look(&client, ws, &a).await;
    look(&client, ws, &b).await;
    fact(&client, ws, "reads of both a.rs and b.rs").await; // F1

    look(&client, ws, &a).await;
    fact(&client, ws, "a second, separate read of a.rs alone").await; // F2

    let out = create_claim(&client, ws, "a.rs defines a()", "F2").await;
    assert_eq!(out["action"], "created");
    let overlap = out["overlap"].as_array().expect("overlap must be an array");
    let f1 = overlap.iter().find(|e| e["id"] == "F1").unwrap_or_else(|| panic!("F1 missing from overlap: {out}"));
    let keys: Vec<&str> = f1["keys"].as_array().expect("keys must be an array").iter().map(|k| k.as_str().unwrap()).collect();
    assert_eq!(keys.len(), 1, "F1 must report exactly the one shared key, not its whole extent: {keys:?}");
    assert!(keys[0].ends_with("a.rs"), "the shared key must be a.rs: {keys:?}");

    client.cancel().await.expect("clean shutdown");
}

/// Same multi-key test as the CLI's
/// `overlap_report_names_every_key_a_fact_shares_when_it_shares_several`,
/// over MCP: F3 touches both a.rs and b.rs, both of which are in the
/// cited union, so both keys must come back.
#[tokio::test]
async fn overlap_report_over_mcp_names_every_shared_key() {
    let sb = Sandbox::new("mcp-overlap-multi-key");
    sb.write("a.rs", "fn a() {}\n");
    sb.write("b.rs", "fn b() {}\n");
    let client = sb.connect().await;
    let ws = "ws";
    let a = sb.dir.join("a.rs").to_str().unwrap().to_string();
    let b = sb.dir.join("b.rs").to_str().unwrap().to_string();

    look(&client, ws, &a).await;
    fact(&client, ws, "a.rs alone").await; // F1
    look(&client, ws, &b).await;
    fact(&client, ws, "b.rs alone").await; // F2

    look(&client, ws, &a).await;
    look(&client, ws, &b).await;
    fact(&client, ws, "both a.rs and b.rs together").await; // F3

    let out = create_claim(&client, ws, "both files define their function", "F1,F2").await;
    let overlap = out["overlap"].as_array().expect("overlap must be an array");
    let f3 = overlap.iter().find(|e| e["id"] == "F3").unwrap_or_else(|| panic!("F3 missing from overlap: {out}"));
    let keys: Vec<&str> = f3["keys"].as_array().expect("keys must be an array").iter().map(|k| k.as_str().unwrap()).collect();
    assert!(keys.iter().any(|k| k.ends_with("a.rs")), "F3 shares a.rs: {keys:?}");
    assert!(keys.iter().any(|k| k.ends_with("b.rs")), "F3 shares b.rs too: {keys:?}");

    client.cancel().await.expect("clean shutdown");
}

/// No overlap still reports an empty array, and the create still
/// succeeds — same invariant as the CLI's
/// `no_overlap_reports_nothing_and_the_create_still_succeeds`.
#[tokio::test]
async fn overlap_report_over_mcp_is_empty_when_nothing_overlaps() {
    let sb = Sandbox::new("mcp-overlap-none");
    sb.write("only.rs", "fn only() {}\n");
    let client = sb.connect().await;
    let ws = "ws";
    let only = sb.dir.join("only.rs").to_str().unwrap().to_string();

    look(&client, ws, &only).await;
    fact(&client, ws, "the only fact in this workspace").await; // F1

    let out = create_claim(&client, ws, "only.rs defines only()", "F1").await;
    assert_eq!(out["id"], "C1");
    assert_eq!(out["action"], "created");
    assert_eq!(out["overlap"].as_array().expect("overlap must be an array").len(), 0, "got: {out}");

    client.cancel().await.expect("clean shutdown");
}

/// The whole point of TET-51: no `note` field anywhere on an overlap
/// entry, over MCP either.
#[tokio::test]
async fn overlap_report_over_mcp_never_carries_a_note_field() {
    let sb = Sandbox::new("mcp-overlap-no-note-leak");
    sb.write("shared.rs", "fn shared() {}\n");
    let client = sb.connect().await;
    let ws = "ws";
    let shared = sb.dir.join("shared.rs").to_str().unwrap().to_string();

    look(&client, ws, &shared).await;
    fact(&client, ws, "a wholly distinctive sentinel note nobody else would type by accident").await; // F1

    look(&client, ws, &shared).await;
    fact(&client, ws, "a second read of the same file").await; // F2

    let out = create_claim(&client, ws, "shared.rs defines shared()", "F2").await;
    let overlap = out["overlap"].as_array().expect("overlap must be an array");
    let f1 = overlap.iter().find(|e| e["id"] == "F1").unwrap_or_else(|| panic!("F1 missing from overlap: {out}"));
    assert!(f1.get("note").is_none(), "an overlap entry must never carry a note field: {f1}");
    assert!(f1.get("keys").is_some(), "an overlap entry must carry its shared keys: {f1}");

    client.cancel().await.expect("clean shutdown");
}

/// `verify` is an object with a mandatory `status`, on every one of the
/// three verbs, and it is `off` until someone turns it on.
///
/// Off is the default because the feature makes an outbound call, and a
/// tool that reaches the network the first time it is run without being
/// asked is not one anybody should install. The rest of the assertion is
/// the contract that makes the object worth having over a third array:
/// under any status but `ok`, there is no `findings` key at all, so a
/// caller cannot read a silent outage, a disabled feature or a call still
/// in flight as a clean bill.
#[tokio::test]
async fn every_authoring_verb_carries_a_verify_object_and_it_is_off_by_default() {
    let sb = Sandbox::new("verify-block-default-off");
    sb.write("read_me.rs", "fn a() {}\n");
    let client = sb.connect().await;
    let ws = "ws";
    let path = sb.dir.join("read_me.rs").to_str().unwrap().to_string();

    look(&client, ws, &path).await;
    let minted = client
        .call_tool(CallToolRequestParams::new("fact").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "note": "read_me.rs defines a()",
        }))))
        .await
        .expect("fact must succeed")
        .structured_content
        .expect("fact must carry structured_content");
    let claimed = create_claim(&client, ws, "read_me.rs defines exactly one function", "F1").await;

    let prosed = client
        .call_tool(CallToolRequestParams::new("prose").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "text": "The file defines one function.",
            "cites": "C1",
        }))))
        .await
        .expect("prose must succeed");
    let prosed = prosed.structured_content.expect("prose must carry structured_content");

    for (verb, out) in [("fact", &minted), ("claim", &claimed), ("prose", &prosed)] {
        let v = out.get("verify").unwrap_or_else(|| panic!("{verb} carries no verify object: {out}"));
        assert_eq!(v["status"], "off", "{verb}: {v}");
        assert!(v.get("findings").is_none(), "{verb} reported findings while off: {v}");
        assert!(v.get("queued_for").is_none(), "{verb} queued a call while off: {v}");
        // Every setting must be visible in the output it affects, which
        // for three of these five is true only because of this echo.
        assert_eq!(v["deterministic"], false, "{verb}: {v}");
        for key in [
            "model",
            "approach",
            "timeout_ms",
            "verbs",
            "literals",
            "guidance",
            "refuter_model",
        ] {
            assert!(v.get(key).is_some(), "{verb} omits `{key}`: {v}");
        }
    }

    // The defaults: the configuration the retrodiction measured, and
    // `claim` with `fact` — `fact` at 88% precision once it is addressed
    // with its own prompt, bounded off `overreaches` and refuted, which is
    // above `claim`'s own 83%. `prose` stays off at 80%, the floor of what
    // is worth printing rather than a margin over it.
    assert_eq!(claimed["verify"]["approach"], "split");
    assert_eq!(claimed["verify"]["verbs"], serde_json::json!(["claim", "fact"]));
    assert_eq!(claimed["verify"]["literals"], false);
    // Refutation defaults on: an unrefuted finding is a warning nobody
    // checked, and `prose` does not clear this tool's bar for printing one
    // without it. The echo is the only place an author who set nothing
    // learns a second model is in the loop.
    assert_eq!(claimed["verify"]["refuter_model"], tetel::config::DEFAULT_REFUTER);
    // The timeout default is computed rather than constant — 100s per
    // provider call the approach makes, so `split` is two, plus two more
    // charged for the refutation leg. `tetel config verify.timeout_ms`
    // reports the *file's* value and prints "(unset)" here, which is
    // correct for that command and would leave the number actually in
    // force invisible. This echo is the only place it appears, and this
    // file's rule is that a setting must be visible in the output it
    // affects.
    assert_eq!(claimed["verify"]["timeout_ms"], 400_000);

    client.cancel().await.expect("clean shutdown");
}

/// Nothing the verifier produces may reach a snapshot, and the mechanism
/// is the enumeration rather than anyone's restraint.
#[tokio::test]
async fn a_render_never_ships_verifier_output_beside_the_memo() {
    let sb = Sandbox::new("verify-log-never-snapshotted");
    sb.write("read_me.rs", "fn a() {}\n");
    let client = sb.connect().await;
    let ws = "ws";
    let path = sb.dir.join("read_me.rs").to_str().unwrap().to_string();

    look(&client, ws, &path).await;
    fact(&client, ws, "read_me.rs defines a()").await;
    create_claim(&client, ws, "read_me.rs defines exactly one function", "F1").await;
    client
        .call_tool(CallToolRequestParams::new("prose").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "text": "The file defines one function.",
            "cites": "C1",
        }))))
        .await
        .expect("prose must succeed");

    // Plant the files a completed verification would leave behind, so the
    // test measures the snapshot's behaviour rather than their absence.
    let state = sb.state_home().join("workspaces").join(ws);
    if state.is_dir() {
        std::fs::write(state.join("verify.log"), "{\"seq\":1}\n").expect("plant verify.log");
        std::fs::write(state.join("verify.cursor"), "1").expect("plant verify.cursor");
    }

    let out = sb.dir.join("memo.md");
    client
        .call_tool(CallToolRequestParams::new("render").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "out": out.to_str().unwrap(),
        }))))
        .await
        .expect("render must succeed");

    let snapshot = sb.dir.join("memo.md.tetel");
    assert!(snapshot.is_dir(), "render must write a snapshot");
    for entry in std::fs::read_dir(&snapshot).expect("snapshot readable") {
        let name = entry.expect("entry").file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with("verify"),
            "`{name}` reached the snapshot — non-reproducible model output must not travel with a memo"
        );
    }

    client.cancel().await.expect("clean shutdown");
}

/// The one test that actually reaches the provider, and the only one that
/// can tell you the wiring works end to end.
///
/// `#[ignore]` on purpose. A suite that makes an outbound call on
/// somebody's credential the moment they type `cargo test` is not one
/// anybody should install, and the rest of this file is written so that
/// the contract — the object, the statuses, the absent `findings` key,
/// the snapshot exclusion — is testable without a network. Run it by
/// hand, with a key in the environment:
///
/// ```text
/// cargo test --test mcp_cli -- --ignored live_verification
/// ```
#[tokio::test]
#[ignore = "makes a real provider call and spends real money"]
async fn live_verification_delivers_a_finding_on_a_later_call() {
    if std::env::var("OPENROUTER_API_KEY").is_err() && std::env::var("TETEL_API_KEY").is_err() {
        eprintln!("no key in the environment; nothing to test");
        return;
    }
    let sb = Sandbox::new("verify-live");
    sb.write("counted.rs", "fn a() {}\nfn b() {}\nfn c() {}\n");

    // Turn it on in the sandbox's own config home, which `Sandbox`
    // points every command at.
    let cfg = sb.config_home();
    std::fs::create_dir_all(&cfg).expect("config home");
    std::fs::write(
        cfg.join("config.toml"),
        "[verify]\nenabled = true\nmodel = \"openai/gpt-5.6-luna\"\nverbs = \"claim\"\ntimeout_ms = 120000\n",
    )
    .expect("write config");

    let client = sb.connect().await;
    let ws = "ws";
    let path = sb.dir.join("counted.rs").to_str().unwrap().to_string();
    look(&client, ws, &path).await;
    fact(&client, ws, "counted.rs, read whole").await;

    // A claim the captured evidence plainly contradicts.
    let first = create_claim(&client, ws, "counted.rs defines exactly two functions", "F1").await;
    assert_eq!(first["verify"]["status"], "queued", "{first}");
    assert_eq!(first["verify"]["queued_for"], "C1", "{first}");
    assert!(first.get("findings").is_none());

    // Poll the way an author would: by making further authoring calls.
    let mut delivered = serde_json::Value::Null;
    for i in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let next = create_claim(&client, ws, &format!("counted.rs is a file ({i})"), "F1").await;
        if next["verify"]["status"] != "queued" {
            delivered = next;
            break;
        }
    }
    assert!(!delivered.is_null(), "no verification was delivered within a minute");
    let v = &delivered["verify"];
    eprintln!("delivered: {v:#}");
    assert_eq!(v["for_mint"], "C1", "the delivery must name the mint it concerns: {v}");
    assert_eq!(v["status"], "ok", "the call did not complete cleanly: {v}");
    let findings = v["findings"].as_array().expect("findings under ok");
    assert!(!findings.is_empty(), "a plainly false count went unflagged: {v}");
    for f in findings {
        // Every span that ships has been through `Fact::quotes`.
        if f["quoted"] == true {
            let span = f["evidence"].as_str().expect("a quoted finding carries its span");
            assert!(
                std::fs::read_to_string(sb.dir.join("counted.rs")).unwrap().contains(span),
                "a shipped quotation is not in the captured text: {f}"
            );
        } else {
            assert!(f.get("evidence").is_none(), "a downgraded finding kept its span: {f}");
        }
    }

    client.cancel().await.expect("clean shutdown");
}

/// A `typesafe/` model written by hand into a workspace's settings file is
/// refused on read, and the author is told so — scope and value — rather
/// than told the key is not set, in front of a file where it plainly is.
///
/// Through the file and the server, not the printer alone, because the
/// case this exists for is a workspace-scope file and only the code that
/// resolves settings can see it. Revert: stop `settings()` carrying the
/// refusal to `block()`.
#[tokio::test]
async fn a_typesafe_check_model_in_a_workspace_file_is_named_not_reported_unset() {
    let sb = Sandbox::new("verify-typed-check-model");
    sb.write("read_me.rs", "fn a() {}\n");
    let cfg = sb.config_home();
    std::fs::create_dir_all(&cfg).expect("config home");
    std::fs::write(cfg.join("config.toml"), "[verify]\nenabled = true\nverbs = \"claim\"\n")
        .expect("write global config");
    let client = sb.connect().await;
    let ws = "ws";
    let path = sb.dir.join("read_me.rs").to_str().unwrap().to_string();
    look(&client, ws, &path).await;
    fact(&client, ws, "read_me.rs defines a()").await;
    let state = sb.state_home().join("workspaces").join(ws);
    std::fs::write(state.join("config.toml"), "[verify]\nmodel = \"typesafe/jev-1.13.0\"\n")
        .expect("write workspace config");

    let claimed = create_claim(&client, ws, "read_me.rs defines exactly one function", "F1").await;
    let v = &claimed["verify"];
    assert_eq!(v["status"], "unauthorized", "{v}");
    let detail = v["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("typesafe/jev-1.13.0") && detail.contains("workspace"), "{v}");
    assert!(!detail.contains("is not set"), "told the key is unset: {v}");

    client.cancel().await.expect("clean shutdown");
}

/// A `verify.typed_model` the key refuses, written by hand into a
/// workspace's settings file, turns the typed legs off — and the reply says
/// so, naming the value and the scope, rather than carrying on silent.
///
/// Through the file and the server for the same reason as the check-model
/// test above: only `settings()` can see a workspace-scope file. Revert:
/// stop `settings()` carrying the refusal to `block()`.
#[tokio::test]
async fn a_refused_typed_model_in_a_workspace_file_is_named_in_the_reply() {
    let sb = Sandbox::new("verify-refused-typed-model");
    sb.write("read_me.rs", "fn a() {}\n");
    let cfg = sb.config_home();
    std::fs::create_dir_all(&cfg).expect("config home");
    std::fs::write(cfg.join("config.toml"), "[verify]\nenabled = true\nverbs = \"claim\"\n")
        .expect("write global config");
    let client = sb.connect().await;
    let ws = "ws";
    let path = sb.dir.join("read_me.rs").to_str().unwrap().to_string();
    look(&client, ws, &path).await;
    fact(&client, ws, "read_me.rs defines a()").await;
    let state = sb.state_home().join("workspaces").join(ws);
    std::fs::write(state.join("config.toml"), "[verify]\ntyped_model = \"openai/gpt-5.6-luna\"\n")
        .expect("write workspace config");

    let claimed = create_claim(&client, ws, "read_me.rs defines exactly one function", "F1").await;
    let v = &claimed["verify"];
    let why = v["typed_model_refused"].as_str().unwrap_or_default();
    assert!(why.contains("openai/gpt-5.6-luna") && why.contains("workspace"), "{v}");
    assert!(v.get("typed_model").is_none(), "a refused value was echoed as in force: {v}");

    client.cancel().await.expect("clean shutdown");
}

/// A verification is delivered exactly once, and a refused call cannot
/// consume it.
///
/// The cursor counts delivered records rather than tracking the largest
/// sequence number seen, because sequence numbers are chosen inside the
/// spawned thread by reading the log — neither atomic nor ordered. This
/// plants a log whose records arrive out of sequence and share a number,
/// which is exactly what two verifications in flight can produce.
#[tokio::test]
async fn a_finding_survives_a_refused_call_and_is_delivered_once() {
    let sb = Sandbox::new("verify-delivery-cursor");
    sb.write("read_me.rs", "fn a() {}\n");
    let client = sb.connect().await;
    let ws = "ws";
    let path = sb.dir.join("read_me.rs").to_str().unwrap().to_string();
    look(&client, ws, &path).await;
    fact(&client, ws, "read_me.rs defines a()").await;

    let state = sb.state_home().join("workspaces").join(ws);
    let log = [
        // Out of order, and colliding on `seq` — both states a lock-free
        // read-max-then-increment produces under two threads.
        r#"{"seq":2,"mint":"C9","verb":"claim","status":"ok","model":"m/x","approach":"split","at":2,"findings":[]}"#,
        r#"{"seq":2,"mint":"C8","verb":"claim","status":"ok","model":"m/x","approach":"split","at":1,"findings":[]}"#,
    ]
    .join("\n");
    std::fs::write(state.join("verify.log"), format!("{log}\n")).expect("plant verify.log");

    // A refusal: minting with an empty pending buffer. It must not eat
    // the finding it was carrying.
    let refused = client
        .call_tool(CallToolRequestParams::new("fact").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "note": "nothing was looked at",
        }))))
        .await
        .expect("protocol-level success");
    assert_eq!(refused.is_error, Some(true), "premise: this call must be refused");

    // First real call gets the first record, second gets the second, and
    // neither is skipped despite the sequence numbers.
    let first = create_claim(&client, ws, "read_me.rs exists", "F1").await;
    assert_eq!(first["verify"]["for_mint"], "C9", "{first}");
    let second = create_claim(&client, ws, "read_me.rs is a file", "F1").await;
    assert_eq!(second["verify"]["for_mint"], "C8", "{second}");
    // Nothing left owed.
    let third = create_claim(&client, ws, "read_me.rs is readable", "F1").await;
    assert!(third["verify"].get("for_mint").is_none(), "{third}");

    client.cancel().await.expect("clean shutdown");
}

/// A failed verification reaches the author through the server: the
/// delivered record says why, the guidance says the mint went unchecked,
/// and `unverified` names it until a withdrawal takes it off the list.
///
/// Through the server because `unverified` is computed after the dispatch,
/// which only `verify_block` can show. Reverts: stop leaving withdrawn
/// claims out (the withdrawal reply still names C1); drop the key, or the
/// delivered `detail`, from `block`.
#[tokio::test]
async fn a_failed_verification_is_named_until_its_claim_is_withdrawn() {
    let sb = Sandbox::new("verify-unverified");
    sb.write("read_me.rs", "fn a() {}\n");
    let cfg = sb.config_home();
    std::fs::create_dir_all(&cfg).expect("config home");
    std::fs::write(cfg.join("config.toml"), "[verify]\nenabled = true\nverbs = \"claim\"\n")
        .expect("write global config");
    let client = sb.connect_keyless().await;
    let ws = "ws";
    let path = sb.dir.join("read_me.rs").to_str().unwrap().to_string();
    look(&client, ws, &path).await;
    fact(&client, ws, "read_me.rs defines a()").await;
    let c1 = create_claim(&client, ws, "read_me.rs defines exactly one function", "F1").await;
    assert_eq!(c1["verify"]["status"], "unauthorized", "premise: no key, so nothing runs: {c1}");

    let state = sb.state_home().join("workspaces").join(ws);
    std::fs::write(
        state.join("verify.log"),
        concat!(
            r#"{"seq":1,"mint":"C1","verb":"claim","status":"timeout","model":"m/x","approach":"split","#,
            r#""at":1,"findings":[],"detail":"provider did not answer within the remaining budget","revision":0}"#,
            "\n"
        ),
    )
    .expect("plant verify.log");

    let c2 = create_claim(&client, ws, "read_me.rs is a file", "F1").await;
    let v = &c2["verify"];
    assert_eq!((v["status"].as_str(), v["for_mint"].as_str()), (Some("timeout"), Some("C1")), "{v}");
    assert_eq!(v["detail"], "provider did not answer within the remaining budget", "{v}");
    assert!(v["guidance"].as_str().unwrap_or_default().starts_with("Not a finding."), "{v}");
    assert_eq!(v["unverified"], serde_json::json!({"count": 1, "mints": ["C1"]}), "{v}");

    let withdrawn = client
        .call_tool(CallToolRequestParams::new("claim").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "withdraw": "C1",
            "why": "superseded",
        }))))
        .await
        .expect("claim call failed at protocol level");
    let w = withdrawn.structured_content.expect("structured withdrawal");
    assert_eq!(w["action"], "withdrawn", "{w}");
    assert!(w["verify"].get("unverified").is_none(), "a withdrawn claim is still listed: {w}");

    client.cancel().await.expect("clean shutdown");
}

/// The grounding floor is the setting the whole `config` module was
/// justified by, and grounding passes run through this server. A floor
/// that applied only to the CLI would be a setting silently ignored on
/// the surface it was written for.
#[tokio::test]
async fn the_mcp_brief_honours_the_configured_grounding_floor() {
    let sb = Sandbox::new("mcp-brief-floor");
    sb.write("alpha.rs", "fn a() {}\n");
    std::fs::create_dir_all(sb.config_home()).expect("config home");
    std::fs::write(sb.config_home().join("config.toml"), "[grounding]\nfloor = 4\n")
        .expect("write config");

    let client = sb.connect().await;
    let ws = "author";
    let alpha = sb.dir.join("alpha.rs").to_str().unwrap().to_string();
    look(&client, ws, &alpha).await;
    fact(&client, ws, "alpha.rs defines a()").await;
    create_claim(&client, ws, "alpha.rs defines exactly one function", "F1").await;
    client
        .call_tool(CallToolRequestParams::new("prose").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "text": "One function.",
            "cites": "C1",
        }))))
        .await
        .expect("prose must succeed");
    let memo = sb.dir.join("memo.md");
    client
        .call_tool(CallToolRequestParams::new("render").with_arguments(args(serde_json::json!({
            "workspace": ws,
            "out": memo.to_str().unwrap(),
        }))))
        .await
        .expect("render must succeed");

    let brief = client
        .call_tool(CallToolRequestParams::new("brief").with_arguments(args(serde_json::json!({
            "memo": memo.to_str().unwrap(),
        }))))
        .await
        .expect("brief must succeed");
    let text = brief
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<String>();
    assert!(text.contains("floor 4"), "the configured floor was ignored:\n{text}");

    client.cancel().await.expect("clean shutdown");
}

/// TET-93: Claude Code spills a reply over its threshold to a file an
/// agent restricted to tetel cannot open, and a tool can declare its own
/// threshold. Compared as exact sets, so a tool that is listed without
/// the declaration fails here rather than passing a subset check.
#[tokio::test]
async fn every_listed_tool_declares_its_spill_threshold() {
    let sb = Sandbox::new("spill-threshold");
    let client = sb.connect().await;

    let tools = client.list_all_tools().await.expect("list tools");
    let listed: std::collections::BTreeSet<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    let declared: std::collections::BTreeSet<&str> = tools
        .iter()
        .filter(|t| {
            t.meta.as_ref().and_then(|m| m.get(tetel::reply::MAX_RESULT_SIZE_KEY))
                == Some(&serde_json::json!(tetel::reply::DECLARED_MAX_RESULT_SIZE_CHARS))
        })
        .map(|t| t.name.as_ref())
        .collect();
    assert!(listed.len() >= 12, "premise: the tool list must be populated, got {listed:?}");
    assert_eq!(declared, listed, "every listed tool, and only those, declares the threshold");

    client.cancel().await.expect("clean shutdown");
}

/// `run` has no shaping of its own yet (A5), so its reply is held under the
/// budget by the backstop in `call_tool` alone — which makes it the verb
/// that shows the backstop is wired, through a real client. Three sizes:
/// under the budget untouched, content fitting with the structured copy
/// dropped, and content alone over it cut and marked.
#[tokio::test]
async fn call_tool_holds_every_reply_to_the_budget() {
    let sb = Sandbox::new("reply-budget");
    let client = sb.connect().await;
    let budget = tetel::reply::REPLY_BUDGET;
    let size = |r: &rmcp::model::CallToolResult| {
        r.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.len()).sum::<usize>()
            + r.structured_content.as_ref().map_or(0, |v| v.to_string().len())
    };
    let run = |n: usize| {
        client.call_tool(CallToolRequestParams::new("run").with_arguments(args(serde_json::json!({
            "workspace": "ws-budget",
            "command": ["seq", "1", n.to_string()],
        }))))
    };
    let seq = |n: usize| (1..=n).map(|i| format!("{i}\n")).collect::<String>();

    // Under the budget: both fields, as `structured()` built them.
    let small = run(3).await.expect("run");
    assert_eq!(small.structured_content.as_ref().expect("structured")["output"], seq(3));

    // ~23k of content, ~46k counting both: only the structured copy goes.
    let mid = run(4000).await.expect("run");
    assert!(size(&mid) <= budget, "size {} over {budget}", size(&mid));
    assert!(mid.structured_content.is_none(), "the structured copy must be dropped");
    let text = &mid.content[0].as_text().expect("text").text;
    let parsed: serde_json::Value = serde_json::from_str(text).expect("the content is still the whole JSON");
    assert_eq!(parsed["output"], seq(4000), "nothing may be lost when only the copy is dropped");
    assert_eq!(text, &parsed.to_string(), "the content must be what `structured()` sent, untouched");

    // ~110k of content: cut, marked, within the budget.
    let big = run(20000).await.expect("run");
    assert!(size(&big) <= budget, "size {} over {budget}", size(&big));
    assert!(big.structured_content.is_none());
    let text = &big.content[0].as_text().expect("text").text;
    assert!(text.contains("[tetel: this reply was cut"), "an over-budget reply must say it was cut");
    // Line 5000 sits about 24k bytes into the output: the cut must spend the
    // budget on the output, not stop before it.
    assert!(text.contains("\\n5000\\n"), "the cut shows too little of the output:\n{}", &text[..text.len().min(300)]);

    client.cancel().await.expect("clean shutdown");
}

/// TET-93 C9, C10 and C14 (vii) through a real client: `look` shapes its
/// own reply, so an over-budget file read and an over-budget search both
/// come back within the budget without the backstop firing, and the file
/// read's capture is exactly the lines the reply carried.
#[tokio::test]
async fn look_shapes_its_own_reply_and_the_backstop_never_fires() {
    let sb = Sandbox::new("look-shapes");
    let budget = tetel::reply::REPLY_BUDGET;
    let body: String = (1..=20000).map(|i| format!("line number {i}\n")).collect();
    sb.write("src/big.txt", &body);
    let client = sb.connect().await;
    let look = |a: serde_json::Value| client.call_tool(CallToolRequestParams::new("look").with_arguments(args(a)));
    let text = |r: &rmcp::model::CallToolResult| {
        assert_ne!(r.is_error, Some(true), "{r:?}");
        assert!(r.structured_content.is_none());
        r.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.clone()).collect::<String>()
    };
    let pending = || -> Vec<serde_json::Value> {
        let raw = std::fs::read_to_string(sb.state_home().join("workspaces/ws-look/pending.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    };

    let page = text(&look(serde_json::json!({"workspace": "ws-look", "path": "src/big.txt"})).await.expect("look"));
    assert!(page.len() <= budget, "{} bytes over the budget", page.len());
    assert!(!page.contains("[tetel: this reply was cut"), "the backstop fired for a file read");
    let shown: Vec<&str> = page.lines().skip(2).collect();
    let k = shown.len();
    assert!(page.lines().nth(1).unwrap().starts_with(&format!("[tetel: showed lines 1-{k} of 20000;")), "{}", &page[..200]);
    let entry = pending().pop().unwrap();
    assert_eq!(entry["label"], format!("src/big.txt lines 1-{k}"));
    assert_eq!(entry["output"], shown.join("\n"), "the capture must be exactly the lines returned");

    let search = text(&look(serde_json::json!({"workspace": "ws-look", "path": "src", "grep": "number"})).await.expect("look"));
    assert!(search.len() <= budget, "{} bytes over the budget", search.len());
    assert!(!search.contains("[tetel: this reply was cut"), "the backstop fired for a search");
    assert!(search.find("[tetel: showed ").unwrap() < search.find("src/big.txt:").unwrap(), "the shortfall must lead");
    let captured: usize = pending()
        .iter()
        .filter(|e| e["label"].as_str().unwrap().starts_with("src/big.txt (grep"))
        .map(|e| e["output"].as_str().unwrap().lines().count())
        .sum();
    assert_eq!(captured, 20000, "the capture must hold every match line");

    client.cancel().await.expect("clean shutdown");
}

/// TET-93 C12 and C14 (vii) through a real client: `query` pages its own
/// listings and a fact's extents, so no fact mode leaves a reply for the
/// backstop to cut, and `from` and `extent_from` reach the paging.
#[tokio::test]
async fn query_pages_its_own_reply_and_the_backstop_never_fires() {
    let sb = Sandbox::new("query-pages");
    let budget = tetel::reply::REPLY_BUDGET;
    let client = sb.connect().await;
    let query = |a: serde_json::Value| client.call_tool(CallToolRequestParams::new("query").with_arguments(args(a)));
    let text = |r: &rmcp::model::CallToolResult| {
        assert_ne!(r.is_error, Some(true), "{r:?}");
        let t = r.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.clone()).collect::<String>();
        assert!(t.len() <= budget, "{} bytes over the budget", t.len());
        assert!(!t.contains("[tetel: this reply was cut"), "the backstop fired for a query:\n{}", &t[..300]);
        t
    };

    // Create the workspace, then give it 40 facts of 3 extents each, and
    // F7 of 40, every label longer than a listing shows: most pages hold
    // several facts, and F7 needs a page to itself.
    text(&query(serde_json::json!({"workspace": "ws-q", "what": "facts"})).await.expect("query"));
    let facts: String = (1..=40)
        .map(|i| {
            let extent: Vec<_> = (1..=if i == 7 { 40 } else { 3 })
                .map(|k| {
                    let l = format!("L{i}-{k}:{}", "x".repeat(1500));
                    serde_json::json!({"key": l, "label": l, "world_state": "ws"})
                })
                .collect();
            serde_json::json!({"event": "Create", "id": format!("F{i}"), "note": "n", "extent": extent,
                "output": "", "pin": "pin", "timestamp": 0})
            .to_string()
                + "\n"
        })
        .collect();
    std::fs::write(sb.state_home().join("workspaces/ws-q/facts.jsonl"), facts).unwrap();

    let mut ids = Vec::new();
    let mut from: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut a = serde_json::json!({"workspace": "ws-q", "what": "facts"});
        if let Some(f) = &from {
            a["from"] = serde_json::json!(f);
        }
        let page = text(&query(a).await.expect("query"));
        let here: Vec<String> = page.lines().filter(|l| l.starts_with('F')).map(|l| l.split('\t').next().unwrap().to_string()).collect();
        if here.contains(&"F7".to_string()) {
            assert_eq!(here, ["F7"], "F7 must have its page to itself");
            assert!(page.contains("more extents: read them with id: F7, extent_from: "), "{page}");
        }
        pages += 1;
        ids.extend(here);
        from = page.split("continue with from: ").nth(1).map(|r| r.split(' ').next().unwrap().to_string());
        assert!(ids.len() <= 40, "paging repeats facts: {ids:?}");
        if from.is_none() {
            break;
        }
    }
    assert_eq!(ids, (1..=40).map(|i| format!("F{i}")).collect::<Vec<_>>(), "every fact exactly once");
    assert!(pages < 20, "premise: most pages must hold several facts, got {pages} pages");

    let mut labels = 0;
    let mut next = None;
    loop {
        let mut a = serde_json::json!({"workspace": "ws-q", "what": "facts", "id": "F7"});
        if let Some(k) = next {
            a["extent_from"] = serde_json::json!(k);
        }
        let page = text(&query(a).await.expect("query"));
        labels += page.lines().filter(|l| l.starts_with("  extent: L7-") && l.len() > 1500).count();
        next = page.split("continue with id: F7, extent_from: ").nth(1).map(|r| r.split(' ').next().unwrap().parse::<usize>().unwrap());
        assert!(labels <= 40, "paging repeats extents");
        if next.is_none() {
            break;
        }
    }
    assert_eq!(labels, 40, "every extent of F7 exactly once, uncut");

    let refused = query(serde_json::json!({"workspace": "ws-q", "what": "claims", "extent_from": 2})).await;
    assert!(refused.is_err(), "extent_from without id on a fact must be refused: {refused:?}");

    client.cancel().await.expect("clean shutdown");
}

/// One `ok` verification record for `mint`, carrying `findings` findings
/// whose quoted text is `len` bytes per field, each field led by its
/// finding's index so a cut one can still be matched to it.
fn verify_record(mint: &str, findings: usize, len: usize) -> String {
    let text = |i: usize, what: &str| {
        let head = format!("finding {i} {what}: ");
        format!("{head}{}", "t".repeat(len.saturating_sub(head.len())))
    };
    let findings: Vec<_> = (0..findings)
        .map(|i| {
            serde_json::json!({
                "kind": "contradicts", "clause": text(i, "clause"), "clause_quoted": true,
                "facts": ["F1"], "evidence": text(i, "evidence"), "why": text(i, "why"), "quoted": true,
            })
        })
        .collect();
    serde_json::json!({
        "seq": 1, "mint": mint, "verb": "fact", "status": "ok", "model": "m/x",
        "approach": "split", "at": 1, "findings": findings,
    })
    .to_string()
}

/// A `fact` reply's JSON and its size by the budget's measure, asserting
/// the backstop did not cut it: an untouched structured reply, or (when
/// `verify` alone takes more than half the budget) the same JSON as text
/// with only the structured copy dropped.
async fn fact_reply(
    client: &RunningService<RoleClient, DummyClientHandler>,
    ws: &str,
    note: &str,
) -> (serde_json::Value, bool) {
    let r = client
        .call_tool(CallToolRequestParams::new("fact").with_arguments(args(serde_json::json!({
            "workspace": ws, "note": note,
        }))))
        .await
        .expect("fact call failed at protocol level");
    assert_ne!(r.is_error, Some(true), "fact was refused: {r:?}");
    let text: String = r.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.clone()).collect();
    let size = text.len() + r.structured_content.as_ref().map_or(0, |v| v.to_string().len());
    assert!(size <= tetel::reply::REPLY_BUDGET, "a fact reply of {size} bytes is over the budget");
    assert!(!text.contains("[tetel: this reply was cut"), "the backstop cut a fact reply:\n{}", &text[..300]);
    let v: serde_json::Value = serde_json::from_str(&text).expect("a fact reply's text is its whole JSON");
    (v, r.structured_content.is_some())
}

/// TET-93 C11 and C14 (v), (vii): the attacker's case. A fact folding six
/// searches whose labels total well over the budget, with a note naming six
/// paths outside its extent and a delivered finding, comes back within the
/// budget with nothing cut by the backstop, which drops only the structured
/// copy, the finding whole, and counts for what `folded` and `attention` left
/// out.
///
/// Reverts: return before `fit_lists` (the backstop cuts); show every extent label in `extent_shown` (the first
/// attention entry's `extent` is eighteen labels).
#[tokio::test]
async fn fact_shapes_its_own_reply_and_the_backstop_never_fires() {
    let sb = Sandbox::new("fact-shapes");
    sb.write("src/a.rs", "needle\n");
    sb.write("src/b.rs", "needle\n");
    let client = sb.connect().await;
    let ws = "ws-fact";
    for n in 0..6 {
        let pattern = format!("needle|{}", (0..400).map(|i| format!("q{n}x{i}")).collect::<Vec<_>>().join("|"));
        let r = client
            .call_tool(CallToolRequestParams::new("look").with_arguments(args(serde_json::json!({
                "workspace": ws, "path": sb.dir.join("src").to_str().unwrap(), "grep": pattern,
            }))))
            .await
            .expect("look");
        assert_ne!(r.is_error, Some(true), "{r:?}");
    }
    let raw = std::fs::read_to_string(sb.state_home().join("workspaces").join(ws).join("pending.json")).unwrap();
    let pending: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    let labels: usize = pending.iter().map(|e| e["label"].as_str().unwrap().len()).sum();
    assert!(pending.len() == 18 && labels > tetel::reply::REPLY_BUDGET, "premise: {} entries, {labels} bytes of labels", pending.len());

    let state = sb.state_home().join("workspaces").join(ws);
    std::fs::write(state.join("verify.log"), format!("{}\n", verify_record("F0", 1, 40))).expect("plant verify.log");

    let note = "needle is in src/a.rs; see gone1.rs, gone2.rs, gone3.rs, gone4.rs, gone5.rs, gone6.rs";
    let (v, structured) = fact_reply(&client, ws, note).await;
    assert!(!structured, "a fact reply over half the budget is fitted to the whole of it, its structured copy dropped");
    assert_eq!(v["id"], "F1");
    assert_eq!(v["verify"]["for_mint"], "F0", "{}", v["verify"]);
    let why = format!("finding 0 why: {}", "t".repeat(40 - "finding 0 why: ".len()));
    assert_eq!(v["verify"]["findings"][0]["why"], why, "the finding must come whole");
    let omitted = &v["omitted"];
    assert!(omitted["folded"].as_u64().unwrap() > 0, "{omitted}");
    assert!(omitted["attention"].as_u64().unwrap() > 0, "{omitted}");
    let shown = v["attention"].as_array().unwrap().len() as u64;
    assert!(shown >= 1, "no attention entry fit beside the others: {omitted}");
    assert_eq!(shown + omitted["attention"].as_u64().unwrap(), 6, "every attention entry is shown or counted");
    let first = &v["attention"][0];
    assert_eq!(first["extent"].as_array().unwrap().len(), 4, "{first}");
    assert!(first["extent"].as_array().unwrap().iter().all(|l| l.as_str().unwrap().len() <= tetel::reply::ENTRY_CAP));
    assert_eq!(first["extent_more"], 14);
    assert!(first["guidance"].as_str().unwrap().contains("; and 14 more)"), "{first}");
    assert!(omitted["see"].as_str().unwrap().contains("id: F1"), "{omitted}");

    client.cancel().await.expect("clean shutdown");
}

/// TET-93 C11 and C14 (v): a verification with more findings than fit at
/// full length shows every one of them, shorter, and the reply is not cut.
///
/// Reverts: drop `fit_findings` from `verify_block` (the verify object is
/// ~360 KB and the reply is over the budget); cap by dropping findings
/// instead of shortening them (fewer than 40 come back).
#[tokio::test]
async fn many_findings_are_all_shown_with_shorter_text() {
    let sb = Sandbox::new("fact-findings");
    sb.write("read_me.rs", "fn a() {}\n");
    let client = sb.connect().await;
    let ws = "ws";
    let state = sb.state_home().join("workspaces").join(ws);
    look(&client, ws, sb.dir.join("read_me.rs").to_str().unwrap()).await;
    std::fs::write(state.join("verify.log"), format!("{}\n", verify_record("F0", 40, 3000))).expect("plant verify.log");

    let (v, _) = fact_reply(&client, ws, "read_me.rs defines a()").await;
    let findings = v["verify"]["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 40, "every finding must be shown");
    assert!(v["verify"].get("findings_withheld").is_none());
    for (i, f) in findings.iter().enumerate() {
        for k in ["clause", "evidence", "why"] {
            let s = f[k].as_str().unwrap();
            assert!(s.starts_with(&format!("finding {i} {k}: ")) && s.ends_with(" …"), "{k} of {i}: {s}");
            assert!(s.len() <= tetel::reply::ENTRY_CAP / 2, "{k} of {i} is {} bytes", s.len());
        }
    }

    client.cancel().await.expect("clean shutdown");
}

/// TET-93 C11 and C14 (v): at the floor, where not even the findings'
/// uncuttable fields fit, the reply shows what fits and says how many it
/// withheld — and the delivery is still committed, so the next
/// verification in the workspace is delivered on the next call.
///
/// Reverts: leave out `findings_withheld`; drop the floor's prefix (all
/// 600 at the smallest cap are over the budget).
#[tokio::test]
async fn at_the_floor_findings_are_withheld_and_counted_and_the_next_is_delivered() {
    let sb = Sandbox::new("fact-floor");
    sb.write("read_me.rs", "fn a() {}\n");
    let client = sb.connect().await;
    let ws = "ws";
    let state = sb.state_home().join("workspaces").join(ws);
    let path = sb.dir.join("read_me.rs").to_str().unwrap().to_string();
    look(&client, ws, &path).await;
    let log = format!("{}\n{}\n", verify_record("F0", 600, 100), verify_record("F9", 1, 40));
    std::fs::write(state.join("verify.log"), log).expect("plant verify.log");

    let (v, _) = fact_reply(&client, ws, "read_me.rs defines a()").await;
    let shown = v["verify"]["findings"].as_array().unwrap().len() as u64;
    let withheld = v["verify"]["findings_withheld"].as_u64().expect("the withheld count must be stated");
    assert!(shown > 0 && shown + withheld == 600, "shown {shown}, withheld {withheld}");

    look(&client, ws, &path).await;
    let (v, _) = fact_reply(&client, ws, "read_me.rs still defines a()").await;
    assert_eq!(v["verify"]["for_mint"], "F9", "the next verification must still be delivered: {}", v["verify"]);

    client.cancel().await.expect("clean shutdown");
}

/// TET-93 C11: a finding's quoted text is held to half an entry even when
/// the allowance has room for all of it, so a few long findings cannot take
/// the room the reply's lists need. Revert: raise `FINDING_TEXT_CAP` past
/// 3000 (both findings come back whole).
#[tokio::test]
async fn a_long_finding_is_cut_to_half_an_entry_even_with_room() {
    let sb = Sandbox::new("fact-finding-cap");
    sb.write("read_me.rs", "fn a() {}\n");
    let client = sb.connect().await;
    let ws = "ws";
    let state = sb.state_home().join("workspaces").join(ws);
    look(&client, ws, sb.dir.join("read_me.rs").to_str().unwrap()).await;
    std::fs::write(state.join("verify.log"), format!("{}\n", verify_record("F0", 2, 3000))).expect("plant verify.log");

    let (v, _) = fact_reply(&client, ws, "read_me.rs defines a()").await;
    let findings = v["verify"]["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2);
    for f in findings {
        let s = f["why"].as_str().unwrap();
        assert!(s.ends_with(" …") && (400..=tetel::reply::ENTRY_CAP / 2).contains(&s.len()), "{} bytes", s.len());
    }

    client.cancel().await.expect("clean shutdown");
}

/// TET-93 C11: a fact reply that cannot go out structured has its lists
/// fitted to the whole budget, and the backstop drops only the duplicate, so
/// every `attention` entry is shown whether `verify` takes a third of the
/// budget or over half. Revert: fit to half the budget unless `verify` and
/// the rest without the lists already take that half (the smaller `verify`
/// loses attention entries the larger one keeps).
#[tokio::test]
async fn attention_keeps_the_whole_budget_beside_any_verify() {
    for findings in [5, 10] {
        let sb = Sandbox::new(&format!("fact-attention-room-{findings}"));
        sb.write("src/a.rs", "needle\n");
        let client = sb.connect().await;
        let ws = "ws";
        let pattern = format!("needle|{}", (0..400).map(|i| format!("q{i}")).collect::<Vec<_>>().join("|"));
        let r = client
            .call_tool(CallToolRequestParams::new("look").with_arguments(args(serde_json::json!({
                "workspace": ws, "path": sb.dir.join("src").to_str().unwrap(), "grep": pattern,
            }))))
            .await
            .expect("look");
        assert_ne!(r.is_error, Some(true), "{r:?}");
        let state = sb.state_home().join("workspaces").join(ws);
        std::fs::write(state.join("verify.log"), format!("{}\n", verify_record("F0", findings, 3000))).expect("plant verify.log");

        let (v, structured) = fact_reply(&client, ws, "needle is in src/a.rs; see gone1.rs, gone2.rs, gone3.rs").await;
        assert_eq!(v["verify"]["findings"].as_array().unwrap().len(), findings);
        assert_eq!(
            v["attention"].as_array().unwrap().len(),
            3,
            "attention must be shown beside {} bytes of verify: {}",
            v["verify"].to_string().len(),
            v.get("omitted").unwrap_or(&serde_json::Value::Null)
        );
        assert!(v.get("omitted").is_none(), "{}", v["omitted"]);
        let text = v.to_string().len();
        assert!(!structured && text > tetel::reply::REPLY_BUDGET / 2, "premise ({findings}): {text} bytes, structured {structured}");

        client.cancel().await.expect("clean shutdown");
    }
}

/// TET-93 C11: at the floor, a field shorter than the " …" trailer is not
/// replaced by the longer trailer, which would withhold findings that fit
/// whole. Revert: start `best_cap`'s search at 0 (fewer findings are shown than
/// fit, since each prefix is sized with its one-byte fields lengthened).
#[tokio::test]
async fn at_the_floor_a_short_field_is_not_lengthened() {
    let sb = Sandbox::new("fact-floor-short");
    sb.write("read_me.rs", "fn a() {}\n");
    let client = sb.connect().await;
    let ws = "ws";
    let state = sb.state_home().join("workspaces").join(ws);
    look(&client, ws, sb.dir.join("read_me.rs").to_str().unwrap()).await;
    let findings: Vec<_> = (0..600)
        .map(|_| serde_json::json!({"kind": "contradicts", "clause": "c", "clause_quoted": true, "facts": ["F1"], "evidence": "e", "why": "w", "quoted": true}))
        .collect();
    let record = serde_json::json!({"seq": 1, "mint": "F0", "verb": "fact", "status": "ok", "model": "m/x", "approach": "split", "at": 1, "findings": findings});
    std::fs::write(state.join("verify.log"), format!("{record}\n")).expect("plant verify.log");

    let (v, _) = fact_reply(&client, ws, "read_me.rs defines a()").await;
    assert!(v["verify"]["findings_withheld"].as_u64().unwrap() > 0, "premise: this is the floor");
    let shown = v["verify"]["findings"].as_array().unwrap();
    for f in shown {
        assert_eq!((f["clause"].as_str(), f["evidence"].as_str(), f["why"].as_str()), (Some("c"), Some("e"), Some("w")), "{f}");
    }
    let one_more = shown[0].to_string().len() + 1;
    assert!(
        v["verify"].to_string().len() + one_more > tetel::reply::VERIFY_ALLOWANCE,
        "the shown prefix must be the longest that fits: {} shown, {} bytes",
        shown.len(),
        v["verify"].to_string().len()
    );

    client.cancel().await.expect("clean shutdown");
}
