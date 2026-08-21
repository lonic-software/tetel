# Verifier evaluation — can a model catch a claim its evidence does not support?

Measurements behind the proposal to warn an author at mint time when a claim
disagrees with the evidence captured for it. Landed here so a design can *cite*
these numbers rather than assert them.

Run against `openai/gpt-5.6-luna` through OpenRouter, reasoning effort high,
three runs per case. The key is read from `$OPENROUTER_API_KEY` (or
`$OPENAI_API_KEY`) and is never written to any file here.

    python3 direct_eval.py --repeat 3        # approach A
    python3 judge_eval.py  --repeat 3        # approach C
    python3 extract2.py    --repeat 3        # approach B

## The three shapes measured

| | calls | who decides | findings re-derivable without a model? |
|---|---|---|---|
| **A** `direct_eval.py` | 1 | the model, seeing claim and evidence together | no |
| **B** `extract2.py` | 2 | Python, comparing two extracted records | **yes** |
| **C** `judge_eval.py` | 3 | the model, seeing only the two extracted records | no |

## Result, 2026-08-10

15 cases (11 with a planted defect, 4 sound), 3 runs each.

| | defects caught | sound claims left alone | failed | cost |
|---|---|---|---|---|
| **A** | **32 / 33** | **12 / 12** | 0 | **$0.0060** |
| **C** | 28 / 32 | 7 / 12 | 1 | $0.0532 |

B was measured on the earlier 14-case set: 17/30 caught, 11/12 left alone,
$0.0414. Its string-equality first draft scored 21/30 and 1/12 — several of
those "catches" were accidental, e.g. flagging `13.0M` against `13,003,879` as
a contradiction when the two agree.

**A wins on both axes at a ninth of the cost.** The pipeline hypothesis — that
extracting first and comparing second would beat one call — did not survive
measurement. C's false positives are all one failure mode: the extractor drops
a binding and the judge cannot tell a dropped binding from an absent one. It
reported *"the evidence contains no `pin` name or binding"* against evidence
whose lines 425-436 are the pin computation.

B keeps one property neither sibling has: its findings are re-derivable by
anyone holding the two extracted records, with no model in the loop.

## Result, 2026-08-16 — `fact` gets its own prompt, and one fewer kind

Two changes ship together, and only the second is a prompt. Both are
measured over the same 123 corpus fact notes.

**1. `overreaches` is not reported on a `fact` (`kind_reported_for`).** Of
the 40 the shipped prompt raised, every one was an insufficiency objection —
*the search excluded paths*, *the capture covers only this range* — which the
prompt already forbids in as many words and the model produced anyway. The
third instance of the lesson this directory keeps relearning: instructing a
model away from a move relocates it, and only a mechanical bound removes it.

**2. `fact` is checked with `FACT_SYSTEM`, not `CHECK_SYSTEM`.** One prompt
addressing "a claim from a design memo" was grading all three verbs. A note
is a record of one capture — terse, scope-bound, quoting code loosely — and
the two clusters `CHECK_SYSTEM` failed on are exactly that difference.

Both surviving sets adjudicated one by one against the full capture:

| after the bound | flag rate | findings | precision | distinct defects caught |
|---|---|---|---|---|
| `CHECK_SYSTEM` (was shipped) | 22.0% | 28 | 39% (11/28) | **11** |
| **`FACT_SYSTEM` (ships now)** | **12.2%** | **16** | **63%** (10/16) | 9 |

The trade is real and not free: 11 of 17 false alarms removed, 2 of 11
catches lost. Taken because the design's own principle is that a wrong
warning costs more than a missed one. Readings in
[fact_shipped_contradicts_adjudicated.md](fact_shipped_contradicts_adjudicated.md)
and [fact_v1_contradicts_adjudicated.md](fact_v1_contradicts_adjudicated.md);
notable catches across all three verbs in [CATCHES.md](CATCHES.md).

`FACT_SYSTEM` still describes `overreaches`. That is the measured
configuration — the prompt names both kinds and the code drops one
afterwards. Writing the kind out of the prompt has never been scored, and a
test refuses the tidy-up.

**`prose` keeps `CHECK_SYSTEM`.** Its candidate cuts the flag rate from 35%
to 23% but nothing has adjudicated what survives, and the shipped prompt's
own prose precision is the worst figure in this directory: 0 of 12. Shipping
an unmeasured prompt over a measured-bad one is still shipping an unmeasured
prompt.

**Two things this does not establish.** The adjudication is one reader, not
the three-refuter vote that produced the cross-model numbers below. And the
`CLAIM:` header in `check_prompt` still heads every subject, including a
note — plainly wrong, untested, and left alone rather than fixed blind.

    python3 verbs_eval.py --verb fact --out fact.json      # reads FACT_SYSTEM
    python3 verbs_eval.py --verb claim --out claim.json    # reads CHECK_SYSTEM

## Result, 2026-08-15 — the literal check, after three rounds

Final configuration, over the 88 corpus claims whose facts carry usable
captured output, three draws each, majority vote:

| | first measured | after iteration | the two shipped kinds |
|---|---|---|---|
| precision | 41% (7/17) | **80%** (8/10) | 83% |
| recall | 14% | **16%** | 30% |
| supports-only flagged | 26.3% (10/38) | **5.3%** (2/38) | 6.5% (4/62) |
| refuted claims surfaced | 0 / 9 | **0 / 9** | 2 / 9 |
| named in the grader's note | 29% | **70%** (7/10) | — |
| cost per draw | $0.0035 | **$0.0013** | — |

**Kill condition 1 (steering hazard, >11.3% of supports-only flagged): now
PASSES** at 5.3%, below the shipped kinds' own 6.5%.
**Kill condition 2 (≤2 of 9 refuted surfaced): still FIRES** at 0 of 9 — the
same condition the shipped kinds fail, at 2 of 9.

Three changes got it there, and only the first was a prompt:

1. **Narrow the prompt to quantities.** The noise was designators —
   `look_grep`, `facts::mint`, `--why`, `2.6.0-FreeBSD`, `check 5`, TET-36.
   Instructing against them took false positives from 7/10 to 3/10 on the
   screening set and did **not** hold: the model stopped naming symbols and
   started naming quantifiers ("any depth", "a single event", "no exclusions
   at all"), which is `overreaches`' territory.
2. **So make it mechanical.** `is_checkable` keeps a literal only if it
   carries a digit, a cardinal word on a word boundary, or a path. No prompt
   wording routes around it. This is what took precision to 80%.
3. **Skip the call when nothing was captured.** With no observations every
   literal is trivially unevidenced and the filter can reject nothing: 37
   such claims raised 506 literals and lost none of them, flagging 78% of
   draws. Those claims now make no call at all.

**An error worth recording, because the evidence contradicted the category.**
Round 2 excluded file paths along with symbols and flags, on the strength of
calling them all "designators". The corpus says otherwise: of 67 findings in
the first run exactly two were path-shaped, both `acks.jsonl`, and *neither
was a false positive* — one on a refuted claim, one on a claim that needed
work. No measured false positive carries a `/` or a file suffix. Re-admitting
paths cost nothing and gained a catch in the refuted bucket, the only bucket
where this check had never scored.

**What it does not do, and `contradicts` already does.** Of the 17 claims the
first run flagged, 5 were already flagged by the shipped kinds; reading the
findings, 2 were the same problem. One of those was the best example this
check had — `918 seconds` — where `contradicts` not only caught it but named
the right value: *"The two captured timestamps differ by 910 seconds, not
918."* The remaining niche is narrow and real: a value that appears **nowhere**
in the capture, against a value the capture **contradicts**. Absence, not
disagreement.

Still unstable: 6 of 29 distinct literals were raised in all three draws.

## Superseded: the first measurement, 2026-08-15

`literals_eval.py` measures the `unevidenced` kind that ships behind
`verify.literals`: does the author's text state a number, path or name as
current fact that no cited capture carries. Same corpus as the retrodiction —
125 claims at their first-render wording, cited facts plus the overlap set,
three draws each, majority vote. The prompt is parsed out of `src/verify.rs`
at runtime, so the harness cannot measure a prompt nobody ships.

37 of the 125 claims cite only facts with no usable captured output — 28 of
them from `tet28` and `tet29`, written before `out_len` existed. On those,
"no capture carries this literal" is trivially true of everything and the
containment filter has nothing to search. They are excluded from every
denominator and reported separately; see the last row below.

| | measured | the two shipped kinds, same corpus |
|---|---|---|
| precision | **41%** (7/17) | 83% |
| recall | **14%** (7/50) | 30% |
| supports-only claims flagged | **10 / 38 = 26%** | 4 / 62 = 6% |
| refuted claims it surfaced | **0 / 9** | 2 / 9 |
| cost | $0.0035 per draw | — |

Both kill conditions the design declared in advance fire, one of them worse
than for the kind that already failed it:

- **Steering hazard.** The threshold was 7 of 62 supports-only claims flagged
  (11.3%). This flags 26.3% — more than double.
- **Does not catch what forces a revision round.** The threshold was 2 or
  fewer of the 9 refuted claims surfaced. This surfaces **none of them**. It
  raised literals on 6 of the 9, and the containment filter killed all but 3
  of those; none of the survivors was the defect the grader named.

Two secondary numbers, both bad:

- **Unstable.** 11 of 37 distinct literals were raised in all three draws. The
  other 70% appeared in some draws and not others, so most findings are a coin
  flip rather than a reading.
- **Wrong about 30% of what it raises**, by its own checkable question: 29 of
  96 raised literals were in the capture after all, found by substring search.
  The filter is doing real work, which is the one part that held up.

What it flags, looking at all 17 surviving findings, is mostly **identifiers
the memo names** — `look_grep`, `facts::mint`, `--why`, `acks.jsonl`, TET-30,
`2.6.0-FreeBSD` — rather than measurements. The prompt already tells it to
skip "a name the text introduces for something it is proposing" and it does
not. Two genuine catches are in there (`918 seconds` against timestamps 910
apart, and `28%`), both arithmetic, both on claims that later needed work.
That is a hypothesis about where the value is, not a result: 17 flags is too
few to split, and separating "numeric" from "name" by looking for a digit puts
ticket ids and version strings in the numeric bucket.

Two things this does **not** establish. The claim-level precision uses a
deliberately wrong denominator — a literal flag is about a clause and "this
claim later needed work" is about a claim, so a correct flag on a sound claim
counts against it. And the blind population below is a property of legacy
records, not of the check.

| the blind population | |
|---|---|
| claims with no captured output | 37 (111 draws) |
| literals raised | 506 |
| survived the filter | **506** — it rejected none, and could not |
| draws flagged | 78% |

That last row is a defect in the shipped code, not in the check: with nothing
captured, the filter that makes an `unevidenced` finding trustworthy has
nothing to search.

    python3 literals_eval.py --populations
    python3 literals_eval.py --repeat 3 --out literals_full125x3.json
    python3 literals_eval.py --summarise literals_full125x3.json

## What these numbers do not establish

- **The case set is small and self-authored** — 15 cases, written by the same
  process that then measured against them. One (`supports-narrower-than-evidence`)
  was found mislabeled *by* the model disagreeing with it, and was split into
  `supports-narrower-tuple` and `qualifies-quantifier-unsettleable`. Others may
  be wrong in ways nothing has surfaced yet.
- **The cases are synthetic.** They imitate the shape of a tetel claim and its
  captured evidence; none was taken from a real memo.
- **Nothing here measures whether warning an author earlier removes a grounding
  round**, which is the actual cost argument. That needs the retrodiction test:
  run a verifier over each memo's claims as they stood at first render, and
  count how many findings the grounding and attacker passes actually raised
  that it would have surfaced first.

## Two harness defects worth not repeating

Both inflated results before being found, and both are fixed here.

**Errors were scored as answers.** An errored row was recorded `flagged=False`,
which reads as a *miss* on a defective case and as a *correct silence* on a
sound one — wrong in both directions at once. On the pre-fix C run this turned
28/28 into an apparent 28/30 and flattered its sound-claim column. Errors are
now their own column and enter no denominator.

**`max_tokens` was too tight for a reasoning model.** Measured on identical
input at temperature 0, reasoning length swings 516..2000 tokens; on the draws
that reach the cap the response returns `finish_reason=length` with content of
length **zero**. Every failure in the pre-fix C run was this. The retry now
raises the ceiling — an earlier version lowered it, which is exactly backwards.

## Files

| | |
|---|---|
| `cases.py` | the case set: proposition, extent, captured output, planted defect |
| `extra_cases.py` | note-vs-extent cases — the `fact` mint comparison |
| `direct_eval.py` / `judge_eval.py` / `extract2.py` | approaches A / C / B |
| `extract_eval.py` | B's first draft, flat-list extraction; keeps `auth_headers` |
| `run_eval.py` | the original verdict harness |
| `literals_eval.py` | the `unevidenced` kind, over the retrodiction's corpus |
| `*_luna2.json`, `*.log` | the runs the table above reports |
| `*_refuted.json`, `*_refuted_gemini.json` | the same findings put to a second model — `anthropic/claude-sonnet-4.5` and `google/gemini-2.5-pro`, scored against the same adjudications, which is what makes the refuter a dial rather than a fixed price |
