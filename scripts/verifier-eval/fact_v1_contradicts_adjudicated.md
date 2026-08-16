# Adjudication: the 16 `contradicts` findings of `fact_v1.json`

Every finding read against the full note and the full captured evidence for its
subject, under the standard in `refute.py`'s `REFUTE_SYSTEM` — **CORRECT** only if the
clause really disagrees with the capture, in the way and of the kind claimed, such
that the author would have wanted to know; default to **WRONG** when not convinced.

**Result: 10 CORRECT, 6 WRONG — 63%**, over 15 flagged subjects of 123 (12%).
Against 3 of 14 (21%) for a sample drawn from both kinds.

Adjudicator: Opus, reading the evidence directly. Cross-model with respect to the
verifier (`openai/gpt-5.6-luna`), but a single adjudicator — weaker than the
three-refuter protocol, and the 63% should be read as one reading, not a vote.

---

## CORRECT (10)

| # | subject | the clause | what the capture shows |
|---|---|---|---|
| 1 | tet30 F2 | "ships **exactly 9** enumerated files" | `if src.exists() { fs::copy(…) }`, and the doc comment says so: *"A file absent from the workspace is skipped rather than erroring"* |
| 3 | tet30 F7 | "**Every** multi-record group shows the SAME digest graded by two or three **distinct passes**" | `('C15','118d12c4') -> [('c1fa7b44…', …), ('c1fa7b44…', …)]` — two records, one pass, sitting in the output |
| 4 | tet30 F8 | the rule as stated: "some record's digest **equals** sha256 of its current proposition" | the rule as executed: `if dg==cur or dg=='':` — an empty digest counts as proof |
| 6 | tet47 F30 | "**FOUR hits, not three**" | the scan printed exactly three |
| 7 | tet47 F30 | "tet47 C4 -> C5 … is the one unambiguous intra-ledger reference" | no such row in the output — the scan globbed rendered memos, and tet47 was not yet one |
| 8 | tet56 F9 | "Return and capture are **the same bytes** here" | `printed` = `==> path <==` header + `shown` (+ maybe a newline); `output` = `shown` |
| 13 | tet61 F14 | "`ProseRequest` — **12 occurrences** in four files" | the note's own enumeration is 6 + 3 + 3 + 1 = 13 |
| 14 | tet61 F18 | "**Twenty** are … in the idiom `let _ = std::fs::remove_dir_all(&dir);`" | nine of the twenty are `std::fs::remove_dir_all(&dir).ok();` |
| 15 | tet61 F20 | "**two lines** of a test fixture (…:**117-119**)" | three lines; and the note's own total only reconciles at 3 + 3 + 1 = 7 |
| 16 | tet61 F21 | "A search of src/ for **`mod tests {`**" | `(grep: mod tests)` — which is why `pub(crate) mod tests_support {` matched |

Four of these (1, 4, 8, 16) are not arithmetic. They are readings of code or of a
tool's own invocation record: a `.exists()` guard, an `or dg==''` inside a forty-line
embedded Python script, a second String assembled from the first, a grep pattern
quoted with a brace it never had.

## WRONG (6)

| # | subject | why it fails |
|---|---|---|
| 2 | tet30 F4 | The note describes the code correctly — `order.insert(at, …)` on a `before` anchor. The objection rests on a doc comment (*"creation order — which is also document order, since this port has no reordering"*) that the function body contradicts. Real tension, wrong target. |
| 5 | tet46 F8 | "(tet28, tet29, tet30)" vs the full `.tetel` directory names. Word-level; the note's substance survives. |
| 9 | tet56 F25 | `out_len` is derived, not copied — but the same note says so three sentences later (*"the extent already carries a length derived from the captured output"*). |
| 10 | tet61 F4 | Reads "the guarantee this design breaks" as asserting the guarantee holds. The design does break it. |
| 11 | tet61 F6 | Conflates `report::render` (the check report) with the rendered memo. Different function, different output. |
| 12 | tet61 F7 | Re-reads "format-level" at its narrowest when the note's own sentence defines it by enumeration — *"format-level and mechanically decidable"*, then lists existence checks as its examples. |

Note the shape of the residue: **none of the six is an insufficiency objection.** Four
are scope misreadings (rule 2 of the shipped prompt), one is pedantry (rule 3), one is
already answered by the note itself. The insufficiency cluster lives entirely in
`overreaches`, which this partition excludes.
