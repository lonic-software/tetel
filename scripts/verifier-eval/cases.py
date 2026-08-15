"""Test cases for a local-model grounding verifier.

Each case is what a tetel grounding pass actually sees: a proposition, and the
captured evidence a fact holds (its extent — what was opened — and the output
that opening produced). The grader decides supports / refutes / qualifies.

The cases are not invented failure modes. Every one marked `qualifies` or
`refutes` is a shape that a real grading or attack pass caught in this project,
which is why they are worth asking a smaller model about: they are the errors
that survive a careless read and get caught by a careful one.

`flaw` is the thing a correct grader must notice. `flaw_markers` are strings
that suggest the grader found it — a coarse aid for triage, never the verdict.
Read the notes; the markers only sort them.
"""

CASES = [
    # ---------- should be clean ----------
    dict(
        id="supports-plain",
        scope="in",
        expect="supports",
        flaw=None,
        flaw_markers=[],
        proposition=(
            "`SNAPSHOT_FILES` is an explicit array of ten entries, and `acks.jsonl` is one of them."
        ),
        extent=["/repo/src/snapshot.rs (lines 71:84)"],
        output="""71: pub const SNAPSHOT_FILES: [&str; 10] = [
72:     "facts.jsonl",
73:     "claims.jsonl",
74:     "prose.jsonl",
75:     "targets.jsonl",
76:     "transplants.jsonl",
77:     "acks.jsonl",
78:     "counters.json",
79:     "identity.json",
80:     "pending.json",
81:     "refusals.log",
82: ];""",
    ),
    dict(
        # Split out of the original `supports-narrower-than-evidence` on
        # 2026-08-10. That case asserted *two* things at once — a narrower
        # tuple (id, where the evidence shows id + keys) and a universal
        # over "each overlapping fact" — and only the first is settled by
        # the excerpt. Luna graded it `qualifies` on 2 of 3 runs and was
        # right; the label was wrong, not the model. The two halves are now
        # separate cases so a run can fail one without the other.
        id="supports-narrower-tuple",
        scope="in",
        expect="supports",
        flaw=None,
        flaw_markers=[],
        proposition=(
            "`overlap_report`'s return value includes the fact's id."
        ),
        extent=["/repo/src/claims.rs (lines 146:168)"],
        output="""146: fn overlap_report(dir: &Path, cited: &[String]) -> io::Result<Vec<(String, Vec<String>)>> {
147:     // returns (fact id, the extent keys it shares with the cited facts)
168:     Ok(out)""",
    ),
    dict(
        id="qualifies-quantifier-unsettleable",
        scope="in",
        expect="qualifies",
        flaw=(
            "The claim quantifies over *every* overlapping fact, but the loop "
            "that populates `out` (148-167) is elided, so the excerpt shows the "
            "shape of the return value and cannot settle its completeness."
        ),
        flaw_markers=["each", "every", "complete", "all", "148", "167", "elid", "settle"],
        proposition=(
            "`overlap_report` returns at least the id of each overlapping fact."
        ),
        extent=["/repo/src/claims.rs (lines 146:168)"],
        output="""146: fn overlap_report(dir: &Path, cited: &[String]) -> io::Result<Vec<(String, Vec<String>)>> {
147:     // returns (fact id, the extent keys it shares with the cited facts)
168:     Ok(out)""",
    ),
    dict(
        id="supports-negative-with-sweep",
        scope="out",  # needs world knowledge of what OTHER helpers could exist; the evidence cannot settle a universal negative
        expect="qualifies",  # CORRECTED 2026-08-10: was "supports". Three patterns cannot close a
        # universal negative — an attack pass on the real memo made exactly this argument, widening
        # the sweep to split_at, .get(.., [..n]. The model was right and the label was wrong.
        flaw="Three patterns cannot establish that no helper exists; the sweep is narrower than the claim.",
        flaw_markers=["split_at", "other", "cannot rule out", "narrower", "only searches"],
        proposition=(
            "No helper for cutting a string at a UTF-8 character boundary exists anywhere under `src/`."
        ),
        extent=["/repo (grep: is_char_boundary|char_indices|\\.truncate\\() over src/"],
        output="(no matches)",
    ),

    # ---------- enumeration short by one ----------
    dict(
        id="qualifies-enumeration-short",
        scope="in",
        expect="qualifies",
        flaw="look_path has a third refusal (invalid --lines range); the claim says it refuses only two things.",
        flaw_markers=["third", "invalid --lines", "range", "three", "only two"],
        proposition=(
            "`look_path` refuses only a missing path and a directory, so a plain read of any "
            "existing file always reaches the capture step."
        ),
        extent=["/repo/src/observe.rs (lines 277:312)"],
        output="""277: pub fn look_path(dir: &Path, path: &str, lines: Option<(usize, usize)>) -> Result<LookOutcome> {
280:     if !p.exists() {
281:         return Err(workspace::refuse(dir, "look", format!("no such path: {path}")));
282:     }
285:     if p.is_dir() {
286:         return Err(workspace::refuse(dir, "look", "path is a directory; use --grep to search it"));
287:     }
300:     if let Some((a, b)) = lines {
301:         if a < 1 || a > b {
302:             return Err(workspace::refuse(dir, "look", format!("invalid --lines range {a}:{b}")));
303:         }
304:     }""",
    ),

    # ---------- a count that is off by one ----------
    dict(
        id="qualifies-count-off-by-one",
        scope="in",
        expect="qualifies",
        flaw="The output shows 70 distinct files, not 71. The line count (883) is right.",
        flaw_markers=["70", "seventy", "off by one", "not 71"],
        proposition=(
            "The census for `check` matched 883 lines across 71 distinct files."
        ),
        extent=["/repo (run: grep -rn check src/ tests/ | wc -l; grep -rl check src/ tests/ | wc -l)"],
        output="""$ grep -rn 'check' src/ tests/ | wc -l
883
$ grep -rl 'check' src/ tests/ | wc -l
70""",
    ),

    # ---------- the evidence is the wrong SHAPE for the claim ----------
    dict(
        id="refutes-name-search-for-behaviour",
        scope="in",
        expect="refutes",
        flaw="The claim is about behaviour but the evidence is a search for a word. The second extent shows a truncation that never uses the word 'truncat'.",
        flaw_markers=["behaviour", "behavior", "word", "name", "refuse_incomplete", "lines().next", "does truncate"],
        proposition=(
            "Nothing in this crate truncates author-supplied text today, so this design "
            "introduces the practice rather than following a convention."
        ),
        extent=[
            "/repo (grep: truncat over src/)",
            "/repo/src/transplants.rs (lines 430:436)",
        ],
        output="""$ grep -rn 'truncat' src/
(no matches)

--- /repo/src/transplants.rs 430:436 ---
430: fn refuse_incomplete(text: &str) -> String {
431:     // show the author enough of their premise to recognise it
432:     let first = text.lines().next().unwrap_or("").trim();
433:     let shown = if text.lines().count() > 1 {
434:         format!("{first} …")
435:     } else { first.to_string() };
436:     shown""",
    ),

    # ---------- premise true, inference invalid ----------
    dict(
        id="qualifies-inference-invalid",
        scope="out",  # needs the reader to simulate the release rule; the evidence gives positions, not read behaviour
        expect="qualifies",
        flaw="Holding a long line does not mean the long line comes first. In every file shown, the over-budget line is not line 1, so a read from line 1 stops before it and returns earlier whole lines.",
        flaw_markers=["first line", "position", "line 1", "order", "not the first", "starts"],
        proposition=(
            "Four tracked files hold a single line longer than the 32768-byte budget, so a plain "
            "read of each of those files returns a header and no content at all."
        ),
        extent=["/repo (run: per-line byte lengths of the four files)"],
        output="""a.jsonl: 20 lines; longest line 905745 bytes at position 15; line 1 is 978 bytes
b.jsonl: 21 lines; longest line 574898 bytes at position 15; line 1 is 882 bytes
c.jsonl: 11 lines; longest line  57249 bytes at position  2; line 1 is 1273 bytes
d.jsonl: 10 lines; longest line  34477 bytes at position  2; line 1 is 1237 bytes""",
    ),

    # ---------- over-claiming clause ----------
    dict(
        id="qualifies-never-overclaims",
        scope="out",  # the contradiction is inside the claim text; the evidence never shows the floored form
        expect="qualifies",
        flaw="The label does name which line overran — it says 'first line cut mid-line'. The 'never' is false for the floored case the same sentence describes.",
        flaw_markers=["first line", "does name", "never", "mid-line", "contradict"],
        proposition=(
            "A bounded return reports how many lines were shown but never which line of the "
            "selection overran, so an author must bisect to find it. The floored form is "
            "`showed 0 of M lines; first line cut mid-line at B of L bytes`."
        ),
        extent=["/repo/src/observe.rs (lines 297:316)"],
        output="""297:     let (shown, label) = match lines {
305:         Some((a, b)) => (sel, format!("{path} lines {a}-{end}")),
312:     };
314:     let mut printed = String::new();""",
    ),

    # ---------- negative claim, under-scoped sweep ----------
    dict(
        id="qualifies-sweep-underscoped",
        scope="in",
        expect="qualifies",
        flaw="The sweep was rooted at src/ only; the claim quantifies over the whole repository, which includes tests/ and build scripts.",
        flaw_markers=["src/", "only", "root", "scope", "tests", "whole repo", "not the whole"],
        proposition=(
            "No code anywhere in this repository parses a label by splitting it on a delimiter, "
            "so appending a segment to a label cannot break a consumer."
        ),
        extent=["/repo (grep: label.split|label.strip_prefix|label.starts_with over src/)"],
        output="(no matches)",
    ),

    # ---------- direct contradiction ----------
    dict(
        id="refutes-direct",
        scope="in",
        expect="refutes",
        flaw="The output shows the field is Option<String> with serde(default); the claim says it is a plain required String.",
        flaw_markers=["Option", "default", "optional", "not required"],
        proposition=(
            "`ProseParams` declares `text` as a plain required `String` with neither `Option` "
            "nor a serde default, so a request omitting it fails deserialisation."
        ),
        extent=["/repo/src/mcp.rs (lines 350:358)"],
        output="""350: pub struct ProseParams {
351:     pub workspace: String,
352:     #[serde(default)]
353:     pub text: Option<String>,
354:     #[serde(default)]
355:     pub cites: Option<Vec<String>>,
356: }""",
    ),

    # ---------- wrong line citation ----------
    dict(
        id="refutes-wrong-citation",
        scope="in",
        expect="refutes",
        flaw="The scope literal is at 194-197, not 210-213; lines 210-213 are a different statement.",
        flaw_markers=["194", "not at", "different line", "210", "wrong line"],
        proposition=(
            "The `scope` literal listing the machine-checked categories is at `src/report.rs:210-213`."
        ),
        extent=["/repo/src/report.rs (lines 190:215)"],
        output="""194:     let scope = "grammar, subset (enumerated rows only), abutting literals, \\
195: unsettled citations, dependency cascades, evidence-ledger import, verdict \\
196: disagreement, claims out of proof, uncensused modification targets, \\
197: transplant premises, provenance drift";
210:     if failing {
211:         out.push_str(&format!("machine-checked: {total} failing — {scope}\\n"));
212:     } else {
213:         out.push_str(&format!("machine-checked: clean — {scope}\\n"));""",
    ),

    # ---------- arithmetic ----------
    dict(
        id="refutes-arithmetic",
        scope="in",
        expect="refutes",
        flaw="13.0M + 9.24M is 22.24M, and the parts do not sum to the stated 31.5M total.",
        flaw_markers=["22.2", "22,2", "sum", "does not add", "arithmetic", "not 31.5"],
        proposition=(
            "The loop cost 31.5M base-input-equivalents in total: the designer at 13.0M plus its "
            "children at 9.24M."
        ),
        extent=["/repo (run: cost breakdown)"],
        output="""designer   13,003,879
children    9,240,000""",
    ),

    # ---------- unstated condition ----------
    dict(
        id="qualifies-unstated-condition",
        scope="in",
        expect="qualifies",
        flaw="The guarantee holds only when the root is a directory; for a single-file root the exclusions are not applied at all.",
        flaw_markers=["directory", "single file", "one file", "is_dir", "root"],
        proposition=(
            "Every `look --grep` census skips tetel's own output, so no census can return "
            "content the tool previously wrote."
        ),
        extent=["/repo/src/observe.rs (lines 240:260)"],
        output="""240: fn decide(root: &Path) -> Exclusions {
241:     if !root.is_dir() {
242:         return Exclusions::None { why: "the caller named one file".into() };
243:     }
250:     Exclusions::Applied { memos, ignored }
260: }""",
    ),

    # ---------- multi-fact, consistent ----------
    dict(
        id="supports-multi-fact",
        scope="in",
        expect="supports",
        flaw=None,
        flaw_markers=[],
        proposition=(
            "The pin is computed over each observation's label and output, so changing a label "
            "changes the pin."
        ),
        extent=[
            "/repo/src/facts.rs (lines 400:412)",
            "/repo/src/facts.rs (lines 425:436)",
        ],
        output="""--- 400:412 ---
400:     for e in &buf.entries {
405:         extent.push(ExtentEntry { label: e.label.clone(), key: e.key.clone(), .. });
412:     }

--- 425:436 ---
425:     let mut h = Sha256::new();
427:     for e in &buf.entries {
428:         h.update(e.label.as_bytes());
430:         h.update(e.world_root.as_bytes());
431:         h.update(e.world_state.as_bytes());
432:         h.update(e.output.as_bytes());
433:     }
436:     format!("sha256:{:x}", h.finalize())""",
    ),
]
