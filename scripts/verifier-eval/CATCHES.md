# What the mint-time verifier catches

A cheap model, reading only what the author's own tool captured, catching errors an
expensive model made while looking at the same bytes.

Every memo cited here was authored by `lonic-designer` on **Opus**, and later graded
by `lonic-grounder` — also Opus, running independently, with the author's scope
withheld. The verifier is **`openai/gpt-5.6-luna`**, temperature 0, one call per
subject, at **$0.0055–$0.0091** per claim, fact or paragraph.

The examples below are not a sample of what the verifier says. They are the ones
where an independent Opus grounding pass later reached the same conclusion, or
where the author's own sentence contradicts itself arithmetically. Corpus, flag
rates and the false-positive side are in [README.md](README.md).

---

## `claim` — a proposition against the facts it cites

### The claim's whole conclusion rested on a false premise

`tet56` C9 argued that one code path had to be left out of scope:

> …because in `look_path` the returned bytes and the captured `output` are the same
> value `shown`; bounding the return there would necessarily change what is captured.

The verifier, at mint time:

> **contradicts** — *the captured entry stores `shown`, but the returned value is
> `printed`, which adds a header and may add a trailing newline.*

The grounding pass **refuted** the claim later, for exactly that reason:

> The claim's premise is right and its inference is false. In `look_path` `printed`
> is a separate String assembled by `printed.push_str(&shown)` at line 316 — a byte
> copy — and `shown` is moved into `PendingEntry.output` afterwards at line 325.
> Two independent Strings exist between those points.

Five lines of captured Rust decided it. The design had already excluded a whole
code path on the strength of the misreading.

### A census that does not add up

`tet61` C15 bounded an implementation's blast radius by counting:

> …it has exactly one production call site, in `lib.rs`, **the other 31 occurrences**
> being its own definition, eighteen unit-test calls in `checks.rs` and doc-comment
> or test-name references.

> **contradicts** — *the captured matches show 1 definition, 18 unit-test calls, and
> 13 doc-comment or test-name references, in addition to the production call. Thus
> there are 32 occurrences other than the production call, not 31.*

Grounding pass, independently: *"one production call site plus **32** other
occurrences, not 31 … No scoping I could find yields 31."*

### The census could not see what the census was about

`tet47` C18 declared a change safe because nothing depended on the wording it changed:

> `grounding_provenance` is named only in src/checks.rs, src/lib.rs and src/report.rs,
> **with no test or fixture depending on its wording**, so the change has one producer
> and one consumer.

> **contradicts** — *the captured test asserts on the literal `SELF-GROUNDED` report
> text, so at least one test depends on the wording of the provenance output.*
> `1028: assert!(combined.contains("SELF-GROUNDED"), "got:\n{combined}");`

Grounding pass: *"The false clause is 'with no test or fixture depending on its
wording'. Grepping the emitted strings rather than the identifier turns up five
end-to-end tests."*

The author's census was by identifier. No identifier grep can find a test that
asserts on a string. The verifier found it because the failing test was already
inside the evidence the claim cited.

### Arithmetic on two timestamps

`tet30` C3, presenting a measurement as its central evidence:

> tet28's C13 was graded `qualifies` by two passes **918 seconds** apart on unchanged text

> **contradicts** — *the two captured timestamps differ by 910 seconds, not 918.*
> `('C13','65ec3111') -> [('2a166dfe…', 1786133118, 'qualifies'), ('c1fa7b44…', 1786134028, 'qualifies')]`

Grounding pass: *"One numeric clause is wrong … is **910 seconds** apart, not 918."*

### A filter one line above the code being described

`tet47` C4 characterised an existing predicate it wanted to reuse:

> rule 2 is membership in `out_of_proof` (**records exist, all with a digest differing
> from `sha256_hex(claim.proposition)`**)

> **contradicts** — *the partition excludes records with an empty digest, so a claim
> with one stale record and one no-digest record has no `fresh` records and is added
> to `out_of_proof`, even though not all of its records have a digest differing from
> the current proposition.*
> `.filter(|r| !r.proposition_digest.is_empty()).partition(…)`

Grounding pass, same reading: *"Rule 2's parenthetical gloss is not [exact] …
Both `fresh` and `stale` are computed after `.filter(|r| !r.proposition_digest…)"*.

Not a number — the semantics of a `.filter()` one line above the `.partition()` the
claim was describing.

---

## `fact` — a note against the observation it was minted from

### The sentence contradicts itself, and the total gives it away

`tet61` F14 censused a symbol:

> `ProseRequest` — **12 occurrences** in four files: the enum at src/prose.rs:211, the
> doc reference at :222, `dispatch`'s signature at :228 and its three exhaustive match
> arms at :230/:235/:240; three construction sites in the CLI…; three in the MCP
> server…; and one mention inside a refusal string in src/scope.rs:793.

> **contradicts** — *6 occurrences in src/prose.rs, 3 in src/main.rs, 3 in src/mcp.rs,
> and 1 in src/scope.rs: 13 occurrences total, not 12.*

The enumeration is in the author's own sentence. Grounding pass, independently:
*"ProseRequest 13 matching lines in four files."*

### 3 + 2 + 1 = 7

`tet61` F20:

> `machine_check_failed` — **7 matching lines**, of which 3 are in src/ … The others
> are **two lines** of a test fixture (tests/fixtures/scratch-memo-A.md:**117-119**)
> and one line of .git/COMMIT_EDITMSG.

> **contradicts** — *the captured fixture output contains three matching lines, 117,
> 118, and 119, not two.*

The stated total only reconciles if the fixture contributes three, and the stated
line range spans three. One word in the middle is wrong.

### The same false premise, caught at both mint points

`tet56` F9, the note the C9 claim above was built on:

> Return and capture are the same bytes here, unlike `look_grep`.

> **contradicts** — *the returned `printed` value includes the `==> path <==` header
> and may include a trailing newline, while the captured `output` is only `shown`.*

The defect was catchable when the fact was minted, and again when the claim cited
it. Both times, before render.

---

## `prose` — a paragraph against the evidence its claims rest on

*Both from the refined run (`prompt_prose_v1.txt`), and both checked here by hand
against the stored capture — no grounding pass grades prose.*

### A number lifted from the wrong row of the author's own table

`tet30` P20:

> This repository's previous two snapshots hold whole file dumps on single JSON lines
> — **tet29's longest is 57249 bytes, tet28's 34477** — so a search for a symbol this
> crate discusses re-captures those dumps.

> **contradicts** — *the captured measurement reports tet29's maximum fact line as
> 57110 bytes, not 57249.*
> **contradicts** — *…tet28's maximum fact line as 34412 bytes, not 34477.*

The capture reads:

```
tet29-imported-mechanism-premises.md.tetel  lines 11  max  57110  total 116734
tet28-modification-target-census.md.tetel   lines 10  max  34412  total 127314
render  matching lines 486  total bytes 373271  max line  57249
```

`57249` is real — it is the row below, the `render` census. `34477` appears nowhere.
The author read one line down and carried the error into the second figure.

### The paragraph diagnoses the off-by-one, then commits it

`tet56` P21 is *about* a miscount, and explains it correctly:

> On this HEAD that census is **883 matching lines** across 70 files — 70 being the
> distinct filenames among those matching lines… An earlier draft said 71, from
> counting distinct filenames over the whole return, **in which the trailing exclusion
> note contributes a spurious extra one**.

> **contradicts** — *the captured census has 882 matching lines; 883 is the total
> including the trailing exclusion note.*

The capture:

```
all_lines_incl_note=883
match_lines=882
files_f5_method_whole_output=71
files_body_only=70
```

The author caught the note's spurious contribution in the file count, fixed 71 to 70,
and left the identical error one field over.

---

## What it costs, and what it does not do

| | per subject | flag rate on published, already-grounded material | precision |
|---|---|---|---|
| `claim` (`contradicts` + `overreaches`) | $0.0055 | 6.5% of sound claims | **83%** |
| `fact` | $0.0060 | 39% | 3 of 14 adjudicated findings held up |
| `prose` | $0.0084 | 35% → **23%** after one prompt round | 0 of 12, on the pre-refinement run |

`claim` is the one that ships on its own numbers. `fact` and `prose` flag far more
than they should — but their noise is not the same noise, and each verb's sits almost
entirely in **one kind**:

| refined run | `contradicts` | `overreaches` |
|---|---|---|
| `fact` | 16 — every confirmed catch above | 21 — **all** insufficiency |
| `prose` | 28 — the proposal-as-current cluster | 16 |

On `fact`, the `overreaches` findings are uniformly *"the search excluded paths"* or
*"the capture covers only this range, so the universal reaches past it"* — objections
to what the evidence fails to **establish**, when the exclusion is the tool's own
disclosed behaviour. The shipped prompt already forbids this in as many words
(`insufficiency is not disagreement`) and the model does it anyway.

On `prose` the noise is in the opposite kind: `contradicts` findings objecting to what
a design **proposes** as though it described current code.

Dropping `overreaches` on `fact` subjects is a mechanical bound rather than a third
prompt round — the same move that took the literal check from 41% to 80%. It ships
(`kind_reported_for`), and it takes the shipped prompt's flag rate from 39% to 22%.

Both surviving sets were then adjudicated one by one against the full capture:

| after the bound | flag rate | findings | precision | distinct defects caught |
|---|---|---|---|---|
| [shipped `CHECK_SYSTEM`](fact_shipped_contradicts_adjudicated.md) | 22.0% | 28 | **39%** | **11** |
| [`prompt_fact_v1.txt`](fact_v1_contradicts_adjudicated.md) | 12.2% | 16 | **63%** | 9 |

Not one false alarm in either set is an insufficiency objection: that failure mode
lived entirely in the kind the bound drops. What remains in the shipped prompt is
scope re-reading and pedantry — 6 findings objecting to a clause read wider than its
own sentence, 3 to "verbatim" against a lossy decode — and v1's numbered rules answer
all 11 of the ones it removes, at a cost of 2 of the 11 catches.

A single cross-model refutation pass is what moves them. On a 14-finding sample with
known ground truth, asking `anthropic/claude-sonnet-4.5` to refute each finding
dropped 11 of 11 wrong ones and kept 2 of 3 right ones — 100% precision on what
survived. The same model refuting itself scored 17%. Finding and checking are
different questions, and a single pass only ever asks one of them.

Recall is the standing limit on all three: `claim` surfaces 2 of the 9 propositions a
grounding pass later refuted. This is an advisory at authoring time, not a gate — it
is worth what it catches, and nothing rests on what it misses.
