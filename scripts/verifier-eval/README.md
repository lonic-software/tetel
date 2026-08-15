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
| `*_luna2.json`, `*.log` | the runs the table above reports |
