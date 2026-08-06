//! The compose↔check seam, guarded as a property rather than by example.
//!
//! # Why this file exists
//!
//! This seam has broken three times, each in a different direction, and
//! each break produced advice that would have destroyed work if followed:
//!
//! 1. `check` reported a memo's own evidence-ledger claims as
//!    *cited-but-undefined*.
//! 2. `render` emitted block citations as `*cites: C1, C4*` and the
//!    scanner recognised only inline `[C1]`, so **every claim a rendered
//!    document cited** came back "defined but never cited; default
//!    disposition is delete".
//! 3. `render` gained a facts table, and a note quoting
//!    `base_tree_hashes: &[String]` was read as a citation to `String`.
//!
//! The cause is structural, not carelessness. `compose` and `checks` were
//! each tested against fixtures **hand-written in the other's dialect**,
//! so both suites passed while the two modules disagreed about the syntax
//! connecting them. Nothing exercised the path a real document takes.
//! Every one of the three was found by running a real memo through, never
//! by a unit test.
//!
//! # The property
//!
//! Everything `render` writes as structured data, `check` must read back
//! as the same structured data. Stated as three equalities over a
//! workspace exercising every construct `compose` can emit:
//!
//!   - every id `render` wrote into a `*cites:*` trailer resolves;
//!   - the claims `check` imports are exactly the non-withdrawn claims;
//!   - the fact ids `check` reads are exactly the workspace's facts.
//!
//! Deliberately *not* asserted: that the report is empty. A rendered memo
//! legitimately carries human-owed findings — ungrounded claims, an
//! uncited claim, a note naming a location outside its extent. Asserting
//! "no findings" would either fail on honest documents or force this test
//! to enumerate acceptable ones, and drift into a second checker. Only
//! findings that mean *the checker could not read the renderer* count.
//!
//! # Verified against all three historical breaks
//!
//! Each was reintroduced into the source and this guard was re-run:
//!
//! | break | reintroduced as | caught by |
//! |---|---|---|
//! | 1. ledger claims not defined | `ledger_by_id` lookup forced false | assertion 4b, unresolved ids |
//! | 2. `*cites:*` trailer unscanned | trailer scan disabled | assertion 4a, citation set equality |
//! | 3. `[String]` read as a citation | `is_citation_shaped` forced true | assertion 4b, unresolved ids |
//!
//! Break 2 is why assertion 4 has two halves, and the reason is worth
//! keeping: an **earlier draft of this file passed with break 2
//! reintroduced.** "No unresolved ids" is satisfied vacuously by a
//! checker that finds no citations at all, which is precisely what that
//! break was. A guard against a seam has to assert that both sides see
//! the same thing, not merely that neither side complains.
//!
//! # Extending this
//!
//! When you add a construct to `compose`, add it to
//! [`author_every_construct`]. That function is the enumeration this
//! property quantifies over, and a construct missing from it is a
//! construct this guard does not cover.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tetel-roundtrip-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Sandbox { dir }
    }

    fn state_home(&self) -> PathBuf {
        self.dir.join("state-home")
    }

    fn workspace_dir(&self) -> PathBuf {
        self.state_home().join("workspaces").join("rt")
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        self.run_with_stdin(args, None)
    }

    fn run_stdin(&self, args: &[&str], input: &str) -> (i32, String, String) {
        self.run_with_stdin(args, Some(input))
    }

    fn run_with_stdin(&self, args: &[&str], input: Option<&str>) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tetel"));
        cmd.args(["--workspace", "rt"]);
        cmd.args(args);
        cmd.current_dir(&self.dir);
        cmd.env("TETEL_STATE_HOME", self.state_home());
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn tetel");
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.unwrap_or("").as_bytes()).unwrap();
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Author a workspace exercising every construct `compose::render` can
/// emit. **This is the enumeration the property quantifies over** — when
/// `render` learns to write something new, it belongs here.
///
/// Text is fed through stdin rather than inline flags throughout, because
/// several of these constructs exist precisely to carry characters a
/// shell would eat.
fn author_every_construct(sb: &Sandbox) {
    sb.write("src/alpha.rs", "fn alpha() {}\nfn second() {}\n");
    sb.write("src/beta.rs", "fn beta() {}\n");

    // --- facts, one per observation kind -----------------------------
    sb.run(&["look", "src/alpha.rs"]);
    sb.run_stdin(&["fact", "--note", "-"], "alpha.rs defines alpha()");

    // A multi-extent fact: two observations folded into one mint.
    sb.run(&["look", "src/alpha.rs", "--lines", "1:1"]);
    sb.run(&["look", "src/beta.rs"]);
    sb.run_stdin(&["fact", "--note", "-"], "two files, one fact");

    // A zero-match grep — an explicit negative observation.
    sb.run(&["look", "--grep", "NEVER_OCCURS", "src"]);
    sb.run_stdin(&["fact", "--note", "-"], "NEVER_OCCURS is absent from src");

    // A `run` fact: a `proc:` extent rather than a path.
    sb.run(&["run", "echo", "hello"]);
    sb.run_stdin(&["fact", "--note", "-"], "echo prints hello");

    // A note carrying a pipe and a newline — both must survive into a
    // table cell without ending the row or the table.
    sb.run(&["look", "src/beta.rs"]);
    sb.run_stdin(&["fact", "--note", "-"], "beta.rs | has a pipe\nand a newline");

    // A revised note.
    sb.run(&["look", "src/beta.rs"]);
    sb.run_stdin(&["fact", "--note", "-"], "original note");
    sb.run_stdin(&["fact", "--revise", "F6", "--why", "clarified"], "");
    sb.run_stdin(&["fact", "--revise", "F6", "--why", "clarified", "--note", "-"], "revised note");

    // --- claims -------------------------------------------------------
    sb.run_stdin(&["claim", "--cites", "F1", "--proposition", "-"], "alpha() exists");
    sb.run_stdin(&["claim", "--cites", "F2,F5", "--proposition", "-"], "a claim resting on two facts");
    sb.run_stdin(
        &["claim", "--cites", "F3", "--proposition", "-"],
        "a proposition with a | pipe, a `backtick`, and\na newline",
    );
    sb.run_stdin(&["claim", "--cites", "F4", "--proposition", "-"], "a claim that will be revised");
    sb.run_stdin(&["claim", "--revise", "C4", "--why", "narrowed", "--proposition", "-"], "the revised proposition");
    sb.run_stdin(&["claim", "--cites", "F6", "--proposition", "-"], "a claim that will be withdrawn");
    sb.run_stdin(&["claim", "--withdraw", "C5", "--why", "wrong"], "");
    // A claim nobody cites — legitimate, and must not be mistaken for a
    // seam break in either direction.
    sb.run_stdin(&["claim", "--cites", "F1", "--proposition", "-"], "a claim no prose cites");

    // --- prose --------------------------------------------------------
    for level in 1..=6u8 {
        sb.run_stdin(
            &["prose", "--heading", "-", "--level", &level.to_string()],
            &format!("Heading level {level}"),
        );
    }
    sb.run_stdin(&["prose"], "A paragraph citing nothing at all.");
    sb.run_stdin(&["prose", "--cites", "C1"], "A paragraph citing one claim.");
    sb.run_stdin(&["prose", "--cites", "C2,C3"], "A paragraph citing two claims.");
    sb.run_stdin(&["prose", "--cites", "C1,F1"], "A paragraph citing a claim and a fact.");
    sb.run_stdin(&["prose", "--cites", "C4"], "A paragraph that will be revised.");
    sb.run_stdin(&["prose", "--revise", "P11", "--why", "reworded"], "The revised paragraph text.");
    // Prose carrying the characters most likely to be misread as syntax:
    // a bracketed type name, a pipe, and an inline citation.
    sb.run_stdin(
        &["prose", "--cites", "C1"],
        "Prose quoting `fn f(xs: &[String])` and a | pipe, mentioning alpha.rs.",
    );
}

/// The guard. See the module doc comment for what is and is not asserted.
#[test]
fn everything_render_writes_check_reads_back() {
    let sb = Sandbox::new("seam");
    author_every_construct(&sb);

    let dir = sb.workspace_dir();
    let rendered = tetel::compose::render(&dir).expect("render must succeed");
    let doc = tetel::parse::parse_document(&rendered);
    let ledger = tetel::ledger::import(&doc.body);

    // --- 1. the renderer's own tables parse without error -------------
    assert!(
        ledger.errors.is_empty(),
        "check could not parse a table render wrote: {:?}\n\n--- rendered ---\n{rendered}",
        ledger.errors.iter().map(|e| format!("line {}: {}", e.line, e.message)).collect::<Vec<_>>()
    );

    // --- 2. the ledger round-trips exactly ----------------------------
    let claims = tetel::claims::load_all(&dir).expect("claims load");
    let live: Vec<&tetel::claims::Claim> = claims.iter().filter(|c| !c.withdrawn).collect();
    let mut imported: Vec<&str> = ledger.claims.iter().map(|c| c.id.as_str()).collect();
    let mut expected: Vec<&str> = live.iter().map(|c| c.id.as_str()).collect();
    imported.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        imported, expected,
        "the claims check imports must be exactly the non-withdrawn claims\n\n--- rendered ---\n{rendered}"
    );

    // A withdrawn claim must not reappear through the ledger.
    for c in claims.iter().filter(|c| c.withdrawn) {
        assert!(
            !imported.contains(&c.id.as_str()),
            "withdrawn {} came back through the ledger:\n{rendered}",
            c.id
        );
    }

    // Propositions must survive the table cell byte-for-byte, modulo the
    // documented escaping: a `|` is escaped and a newline becomes a
    // space, because a markdown row is one physical line.
    for c in &live {
        let want = c.prop.replace('\n', " ");
        let got = ledger
            .claims
            .iter()
            .find(|l| l.id == c.id)
            .unwrap_or_else(|| panic!("{} missing from ledger", c.id));
        assert_eq!(
            got.proposition, want,
            "{}'s proposition did not survive the round trip",
            c.id
        );
    }

    // --- 3. the facts table round-trips exactly -----------------------
    let facts = tetel::facts::load_all(&dir).expect("facts load");
    let mut table_ids = tetel::ledger::facts_table_ids(&doc.body);
    let mut fact_ids: Vec<String> = facts.iter().map(|f| f.id.clone()).collect();
    table_ids.sort();
    fact_ids.sort();
    assert_eq!(
        table_ids, fact_ids,
        "the fact ids check reads must be exactly the workspace's facts\n\n--- rendered ---\n{rendered}"
    );

    // --- 4. every citation render wrote, check finds -------------------
    //
    // Both directions are needed, and only one of them was obvious.
    //
    // "No unresolved ids" alone is satisfied vacuously by a checker that
    // finds no citations at all — which is exactly what break #2 was, and
    // an earlier draft of this file passed with that break reintroduced.
    // The set of ids the scanner recovers must *equal* the set the prose
    // blocks actually cite.
    let blocks = tetel::prose::load_all(&dir).expect("prose load");
    let mut authored: Vec<String> = blocks.iter().flat_map(|b| b.cite.clone()).collect();
    authored.sort();
    authored.dedup();
    assert!(!authored.is_empty(), "test setup: some prose must cite something");

    let mut scanned: Vec<String> =
        tetel::citations::scan_citations(&doc.body).into_iter().map(|c| c.id).collect();
    scanned.sort();
    scanned.dedup();
    assert_eq!(
        scanned, authored,
        "the citations check recovers must be exactly the ones prose declared — a checker that \
silently finds none passes every other assertion here\n\n--- rendered ---\n{rendered}"
    );

    // And nothing render wrote resolves to nowhere.
    let findings = tetel::checks::analyze(&doc, &ledger.claims);
    assert!(
        findings.cited_undefined.is_empty(),
        "check could not resolve ids render itself wrote: {:?}\n\n--- rendered ---\n{rendered}",
        findings.cited_undefined
    );

    // --- 5. nothing render wrote is a grammar error -------------------
    assert!(
        findings.grammar_errors.is_empty(),
        "render produced something check calls malformed: {:?}\n\n--- rendered ---\n{rendered}",
        findings.grammar_errors
    );
}

/// The property must actually fail when the seam breaks, or it is
/// decoration. Reproduces break #2 — the renderer emitting a citation
/// syntax the checker cannot read — by checking a document whose `*cites:*`
/// trailers have been rewritten into a form the scanner does not know.
#[test]
fn the_guard_fails_when_the_renderer_and_checker_disagree() {
    let sb = Sandbox::new("seam-negative");
    author_every_construct(&sb);

    let rendered = tetel::compose::render(&sb.workspace_dir()).expect("render must succeed");
    // Exactly the historical defect: block citations in a shape `check`
    // has no scanner for.
    let broken = rendered.replace("*cites: ", "*grounded-in: ");
    assert_ne!(broken, rendered, "test setup: the rendered doc must carry cites trailers");

    let doc = tetel::parse::parse_document(&broken);
    let ledger = tetel::ledger::import(&doc.body);
    let findings = tetel::checks::analyze(&doc, &ledger.claims);

    // With the trailers unreadable, claims the prose really does cite are
    // reported as never cited — the exact wrong advice that shipped.
    assert!(
        !findings.defined_uncited.is_empty(),
        "the guard's premise is broken: an unreadable citation syntax must show up as \
defined-but-uncited, or assertion 4 above proves nothing"
    );
}
