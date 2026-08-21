# Adjudication: all 44 findings of `prose_v1.json`

Same standard and adjudicator as the two `fact` sets, so the three compare.
`prompt_prose_v1.txt` is the refined candidate; the shipped prompt is
`CHECK_SYSTEM`, which scored 0 of 12 on the baseline run.

**Result: 6 correct, 38 wrong — 14%.** Against `fact`'s 63% and `claim`'s 83%.

| | flag rate | findings | precision |
|---|---|---|---|
| `prose` on `CHECK_SYSTEM` (ships) | 34% | 65 | **0 / 12** |
| `prompt_prose_v1.txt` | 23% | 44 | **6 / 44 = 14%** |

The refinement is a real improvement over zero and nowhere near usable.

## The 6 correct

* **`tet30` P20 ×2** — *"tet29's longest is 57249 bytes, tet28's 34477"*. The
  capture reads `max 57110` and `max 34412`; `57249` is the row below, the
  `render` census. The author read one line down and carried it into both
  figures.
* **`tet47` P50** — *"The count admits **only** records matching a claim's
  current digest"* against
  `.filter(|r| r.proposition_digest.is_empty() || r.proposition_digest == current)`.
  The same empty-digest defect `fact` F8 and claim C4 found, third sighting.
* **`tet56` P21 ×2** — *"883 matching lines across 70 files"* where the capture
  reads `all_lines_incl_note=883 / match_lines=882`. The paragraph *diagnoses*
  this exact off-by-one for the file count (71 → 70, the trailing exclusion
  note) and then commits it one field over. And *"`look --lines` annotates a
  range only when it had to clamp one"* against
  `format!("{path} lines {a}-{end}")`, which annotates always.
* **`tet61` P4** — *"For each non-heading block with a non-empty citation list,
  the check derives an anchor"* against `if cited.is_empty() { continue; }`.
  A block can carry citations with none in proof and is skipped. The paragraph
  is restating the rule the design must key on "exactly ... and no others", so
  the missing existential gate matters.

## Where the 38 go

| cluster | count | share of wrong |
|---|---|---|
| insufficiency — "the capture covers only X, so the universal reaches past it" | 15 | 39% |
| **proposal read as current** | **11** | **29%** |
| misread referent or scope | 9 | 24% |
| word-level pedantry | 3 | 8% |

**The proposal cluster is the diagnostic one.** Eight findings land on `tet56`
P23 alone, every one objecting that the code has no line-length budget — which
is precisely what that paragraph proposes to add. Three more land on `tet61`
P25, objecting that the prose parser rejects `Ack` events, which is the design's
premise for adding them. Evidence captured before a design exists cannot
contradict what the design intends to build, and `prompt_prose_v1.txt` says so
in its opening lines. It is ignored.

That failure is what the classify step exists to prevent. Every subject is split
into `current` / `proposed` / `argument` and the check is told which assertions
the evidence can speak to. On a prose paragraph it is not working — and the
classify prompt opens *"You are given one claim from a software design memo"*
while `check_prompt` heads the text `CLAIM:`. A paragraph of design argument is
being presented to the splitter as an assertion about today.

## What would and would not help

Dropping `overreaches` on `prose`, as `fact` now does, is **not** the answer.
The kind carries 16 of the 44 findings and 1 of the 6 catches, so the bound
would give 5 of 28 — 18%, at the cost of the P4 catch. The noise on `prose`
sits in `contradicts`, which is the opposite of `fact`.

The one untested change with a mechanism behind it is the **classify half**: a
prose-shaped classify prompt and a `PARAGRAPH:` header instead of `CLAIM:`.
That targets 29% of the errors directly and nothing else on the list does. It
was deliberately not made when `fact` got its own check prompt, because the
measured configuration swapped only the check prompt and changing the header
would have shipped something no run has scored.

## Measured, 2026-08-16 — `prose_classify_v1.json`

The classify half was changed and the check prompt held at
`prompt_prose_v1.txt`, so the classify prompt plus a `PARAGRAPH:` header in
place of `CLAIM:` is the only variable. All 24 findings adjudicated.

| | flag rate | findings | precision |
|---|---|---|---|
| `CHECK_SYSTEM` (ships) | 34% | 65 | 0 / 12 |
| `prompt_prose_v1.txt` | 23% | 44 | 6 / 44 = 14% |
| **+ prose classify + `PARAGRAPH:`** | **16%** | **24** | **5 / 24 = 21%** |
| the same, with `overreaches` dropped | 11% | 14 | **5 / 14 = 36%** |

**The mechanism is confirmed. The proposal cluster went from 11 to 0.** Not one
finding now objects to what the design proposes as though it described current
code — the eight on `tet56` P23 and the three on `tet61` P25 are all gone, and
nothing of that shape replaced them. The hypothesis was that a paragraph of
design argument headed `CLAIM:` was being split as an assertion about today,
and it was right.

Twenty fewer false alarms for the same five distinct defects. One real catch
was lost — `tet47` P50, the empty-digest reading — and one gained: `tet56` P27,
*"all three arms of that `match` finish the label before the string the bound
applies to has been created at all"*, against a capture showing
`let (shown, label) = match lines { None => (contents.clone(), path.to_string()), … }`,
where every arm builds the string first. The paragraph has the ordering
backwards, and it is reasoning about where a bound can be inserted.

**All 10 `overreaches` are wrong, and all 5 catches are `contradicts`** — the
same signature that justified the bound on `fact`, now clean on `prose` too.
Applying it gives 36% at an 11% flag rate.

One rejected candidate worth recording, because it looked right: `tet61` P7,
*"Nothing binds a memo to the workspace that rendered it"*, reported against
`identity.json` shipping in the snapshot. In context the clause continues "—
the flag is free on every invocation —" and is about enforcement, not about
whether identity is recorded. `identity.json` records who authored; nothing
stops a different workspace re-rendering, which is exactly the failure the
paragraph goes on to construct.

## Where this leaves the verb

0% → 14% → 21% → 36%, each step from a named mechanism rather than a reword.
That is real, and 36% is still well under `fact`'s 63% and `claim`'s 83%: two
warnings in three would be wrong. On a tool whose stated principle is that a
wrong warning costs more than a missed one, that is not shippable.

The residue is no longer one cluster. Of the 19 wrong, 10 are insufficiency
(which the bound removes), and the remaining 9 are scattered — a scope
misread, a doc comment cited against a verified fact, a zero-match branch
objected to in a paragraph about the matching case. No single mechanism
addresses them, which is the difference between this round and the last two.

`prose` stays off.
