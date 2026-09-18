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

## Result, 2026-09-17 — the refuter, asked of a model that cannot write

TypeSafe's **Jev** is not an LLM. It generates no strings: you post state plus
a map of typed questions and it returns enumerated answers with calibrated
probabilities. `REFUTE_SYSTEM` already asks for one of three words, so the
refuter is the one leg here that was already written as a question this model
class can answer — the option set is in the shipped prompt.

Same 16 `contradicts` findings of `fact_v1.json`, same adjudications, scored
by the same join that reproduces the two rows below it.

| refuter | true catches kept | precision | $/finding | median latency |
|---|---|---|---|---|
| `anthropic/claude-sonnet-4.5` | 8 / 10 | **89%** | $0.0228 | — |
| `google/gemini-2.5-pro` | **10 / 10** | 77% | $0.0184 | — |
| **`jev-latest`, 3-draw majority** | 8 / 10 | 80% | **$0.00034** | **0.72s** |
| `jev-latest`, single draw | 8 / 10 | 80% | $0.00011 | 0.69s |

**It lands between the two LLMs at a sixty-seventh of the cost.** Sonnet's
recall with nine points less precision; Gemini's precision with two fewer
catches. It loses `tet30 F2` and `tet61 F21`; Sonnet also loses F21, so one of
the two is hard for everything. Every run reported on this page cost, in total,
under a cent.

**It is stable in a way the LLM arms are not.** 15 of 16 verdicts identical
across three draws, probabilities reproducing to two decimals — `0.63/0.63/0.63`,
`0.94/0.94/0.94`. The single flip, `tet46 F8`, sits at p≈0.69 and is a boundary
case rather than noise. The literal check's own stability figure on this page
is 6 of 29 raised in all three draws.

**The probability means what it says — on this corpus.** Across three draws the
bins are monotonic and tight:

| bin | n | mean p | observed correct |
|---|---|---|---|
| 0.2–0.4 | 2 | 0.33 | 0% |
| 0.4–0.6 | 1 | 0.50 | 0% |
| 0.6–0.8 | 10 | 0.71 | 70% |
| 0.8–1.0 | 3 | 0.89 | 100% |

0.71 against an observed 70% is the calibration claim doing what it claims. On
a *single* draw these bins are non-monotonic (0.70→78%, 0.88→75%); that is
small-n noise, and reading it as miscalibration was this measurement's first
wrong answer. **The second was believing it would transfer** — see the prose
result below, where it does not.

**What the calibration buys here is a partition.** The tails separate and the
middle does not, which is the model reporting honestly that 10 of these 16
findings are near-chance:

| band | n | adjudicated | disposition |
|---|---|---|---|
| p ≥ 0.8 | 3 | all CORRECT | accept, no LLM call |
| p ≤ 0.5 | 3 | all WRONG | drop, no LLM call |
| 0.5 < p < 0.8 | 10 | 7 correct, 3 wrong | escalate |

38% of findings resolved for free, at no cost in true catches and none in false
positives. **Only the lower half of that survives prose.**

Jev alone can also buy Sonnet's precision by moving the line rather than
changing vendor: at p ≥ 0.7 it keeps 8 findings, retains 7 of 10 catches, at
88%. That threshold is what `REFUTE_SYSTEM` has to express as prose — *"Default
to WRONG when you are not convinced"* — because a word has no dial.

**What this does not establish.**

- **n = 16, three findings in each tail.** Clean tails are exactly the property
  that breaks on more data, and nothing here has more.
- **One adjudicator**, the same weakness the 2026-08-16 result records: this is
  one reading, not the three-refuter vote.
- **It says nothing about a gate before the check leg**, which is where the
  spend actually is. That question — *is there anything here at all* — runs
  against a population that is ~90% empty and has no labels on disk.
- **Latency is measured at 0.69–0.75s, not the advertised 70–500ms.** Still
  about fourteen times the LLM median this page records, but the published
  figure does not carry ~14KB of captured evidence in the state.

Two things the harness does differently, both deliberate. Each finding is put
as **two questions in one call** — a `choice` over the shipped three verdicts,
and a `noul` returning P(correct) — because the questions evaluate in parallel
and the second is the one no LLM arm can answer. And the `noul` is asked
**neutrally**: the burden-of-proof instruction is a threshold, and with a
probability the threshold is applied when scoring. Writing it into the question
too would apply it twice and make the sweep measure itself.

    python3 refute_jev.py fact_v1.json --out fact_refuted_jev.json \
        --keep CORRECT,WRONG,UNCLEAR,ERROR      # lossless: see below
    python3 score_refuter.py fact_refuted_jev.json --prob p_correct


### The same measurement on `prose`, where the base rate is a quarter as high

44 findings, 6 adjudicated correct — a 14% base rate against `fact`'s 62%. Same
model, same prompt, same three-draw protocol. There is **no LLM refutation on
this population** to compare against: `prose_refuted*.json` refute
`prose_classify_v1.json`'s 24 findings, which share only 6 with these 44. The
available comparison is the unrefuted base rate.

| | base rate | after Jev | true catches kept |
|---|---|---|---|
| `prose_v1`, 44 findings | 14% (6/44) | **33%** (4/12) | 4 / 6 |

Stability holds: 41 of 44 verdicts identical across three draws, the largest
probability spread 0.10, and all three flips inside 0.55–0.76.

**Calibration does not hold.** The same model on the same task, one corpus over:

| bin | n | mean p | observed correct |
|---|---|---|---|
| 0.0–0.2 | 1 | 0.13 | 0% |
| 0.2–0.4 | 9 | 0.30 | 0% |
| 0.4–0.6 | 16 | 0.53 | 6% |
| 0.6–0.8 | 12 | 0.69 | 17% |
| 0.8–1.0 | 6 | 0.83 | **50%** |

Overconfident at every bin that carries weight, and worst where it matters: the
band that was 100% correct on `fact` is 50% here. The ordering is still
informative — the curve rises monotonically — but the *number* is not a
probability of correctness on this corpus, and the `p ≥ 0.8 → accept` rule
above would ship a wrong warning half the time.

**So a threshold fitted on one verb cannot be carried to another.** That is not
a footnote about tuning: `fact` and `prose` already differ in prompt, in kinds
reported and in whether they are on by default, and this adds a fitted number
to that list — fitted per verb, on that verb's own adjudicated population.

**One rule does hold across both.** Dropping everything below p = 0.5 costs
zero true catches on either corpus:

| | dropped | share | catches lost |
|---|---|---|---|
| `fact` | 3 of 16 | 19% | 0 of 10 |
| `prose` | 13 of 44 | 30% | 0 of 6 |

**Jev is a reliable negative filter and an unreliable positive one** — when it
is handed a specific proposed finding to grade. "Is it safe to drop this
finding" is answered on both corpora; "is this finding correct" is answered on
`fact` and fails on `prose`. That negative-filter property is a fact about the
refuter position and **did not transfer to the first gate** measured below —
the whole-note one, which is the first of several presentations tried.

    python3 refute_jev.py prose_v1.json --out prose_refuted_jev.json \
        --kind contradicts,overreaches --keep CORRECT,WRONG,UNCLEAR,ERROR
    python3 score_refuter.py prose_refuted_jev_majority.json \
        --labels labels_prose_v1.json --prob p_correct


### The gate before the check leg, which is where the spend is

The refuter runs per finding and findings are rare. The check leg
(`verify.rs:1008`) runs per mint, carries the evidence, and costs **$0.0054 a
subject on `fact` and $0.0084 on `prose`** — against the gate's $0.000089, some
sixty-one times less. If a cheap typed question could say *nothing to see here*
before the expensive call, that is the whole cost argument for this model class
in this tool.

Every subject in both corpora, three nouls per call (`any`, `contradicts`,
`overreaches`), three draws. No LLM call was made: the expensive side already
ran and is in the results files. 235 subjects, 705 calls, **9.2 cents**.

A subject is `must_keep` if the LLM raised a finding on it that the adjudication
graded CORRECT — the only bucket where skipping is a loss.

| | subjects | must_keep | noise | silent |
|---|---|---|---|---|
| `fact` | 123 | 9 | 6 | 92 |
| `prose` | 112 | 4 | 22 | 86 |

**It does not separate them.**

| | AUC, must_keep vs silent | must_keep median p | silent median p |
|---|---|---|---|
| `fact` | 0.78 | 0.45 | 0.20 |
| `prose` | **0.52** | 0.38 | 0.37 |

On `prose` that is a coin flip: no threshold exists, and the four subjects
carrying a true catch score 0.81, 0.41, 0.36 and **0.14**. On `fact` there is
real signal and it is not enough — three of the nine true catches sit at 0.22,
0.19 and 0.13, below the median of the subjects the LLM was silent on.

The best zero-loss operating point is `p_contradicts < 0.1`, which is a narrow
ledge rather than a setting: raising it to 0.2 takes savings from 22% to 68%
and loses two of the nine catches on the way.

| `fact`, 123 subjects | cost | catches lost |
|---|---|---|
| ungated | $0.6646 | — |
| gated at `p_contradicts < 0.1` | $0.5297 | 0 of 9 |

**20%, on the corpus where the gate works at all.**

**Why it works as a refuter and not as a gate**, which is the part worth
keeping. `refute.py`'s own docstring says it: *"finding and checking are
different questions and only one of them was ever asked."* The refuter hands
Jev a clause, a reason and a quotation and asks whether that specific assertion
holds — a well-posed local question, and it scores 80%. The gate hands it a
whole note plus up to 14KB of capture and asks whether anything anywhere in
there disagrees. That is a search, and a model that generates no strings has no
way to enumerate candidates; it can only score what it is given. **Jev can check
a proposition. It cannot find one.**

~~That is a property of the model class rather than of this prompt~~ — **that
sentence was wrong the day it was written.** It generalised from one prompt
shape. Cutting the note into sentences mechanically, and asking one noul per
sentence in the same single call, moved `fact` from 0.79 to 0.89 in a one-draw
probe — 0.72 to 0.80 over three draws against the wider defect set of the next
section, and 0.86 for the best presentation found. What one question failed at
is a finding about that question, and the next section is the iteration.

The three draws behind the tables above were written to a session scratchpad
and did not survive it; `gate_fact.json` and `gate_prose.json` are one draw
each. The baseline is re-run as `whole` in `gate_runs/`, three draws kept.

    python3 gate_jev.py fact_v1.json --out gate_fact.json
    python3 score_gate.py gate_fact.json --labels labels_fact_v1.json --field contradicts

## Result, 2026-09-18 — the gate, iterated: presentation is most of it

The first gate failed, and reading its failures said why: the true catches it
scored lowest were small, untruncated notes whose defect the refuter scored at
0.76–0.94 once pointed at the clause. The evidence was all there. The question
asked Jev to *search* the note, and a model that emits no strings cannot list
candidates. So every variant below changes how the note is **presented** so
that less of the search is left to the model — same state, same evidence view,
one lever at a time against the previous best.

**The ground truth widened first.** A gate must not skip a subject with a real
defect, whichever LLM run happened to find it, so it is graded on subjects
rather than on one run's findings: `defects_v1.json`, the union of all three
adjudications — **12** `fact` subjects (9 from v1, plus `tet30` F6, `tet47` F6
and `tet56` F29 that only the shipped prompt caught) and **5** `prose` (adding
`tet56` P27 from the classify run). Everything else is negative, including
subjects an LLM flagged wrongly, which a gate skipping is a gain.

Three draws per variant, every one kept in `gate_runs/`. 60 draw files on
`fact` and `prose`, **$1.19**, no errors.

| variant | lever | `fact` AUC | `fact` skipped, no defect lost (−0.05 margin) | `fact` net saving | `prose` AUC |
|---|---|---|---|---|---|
| `whole` | the first gate, re-run | 0.72 | 1% | −1% | 0.51 |
| `split` | one noul per sentence | 0.80 | 49% | 47% | 0.69 |
| **`pick`** | **one `choice` over the sentences + NONE** | **0.85** | **54%** | **52%** | 0.65 |
| `atoms` | one noul per number, quantifier or quoted name, with its clause | 0.84 | 46% | 40% | 0.65 |
| `pick_recall` | criteria rewritten for recall | 0.83 | 46% | 44% | 0.59 |
| **`pick_cls`** | **+ per-sentence CURRENT/PROPOSED/ARGUMENT; only CURRENT counts** | **0.86** | **60%** | **58%** | 0.55 |
| `pick_both` | recall + classify | 0.83 | 48% | 45% | 0.55 |
| `pick_prop` | classify, discounting only PROPOSED | 0.85 | 52% | 49% | 0.46 |
| `pick_clause` | pick over clauses, not sentences | 0.84 | 43% | 41% | 0.60 |
| `pick_evfirst` | evidence before the text | 0.79 | 22% | 20% | 0.66 |

Net saving is the check leg's cost avoided ($0.0054 a `fact` subject) minus the
gate paid on every subject ($0.00009–0.00014), over the ungated cost.

**On `fact`: from 1% to 58%.** The lever that did it is `pick` — asking *which*
sentence disagrees, with NONE as an option, instead of *whether* anything does.
The sentences compete for one probability mass, so a note whose sentences are
all fine puts it on NONE, and the model never has to hold "anywhere in here" in
one judgement. Classification added six points; the other levers subtracted.
Combining variants by rank average bought at most one point and was not pursued.

**What each lever taught, including the ones that lost:**

- **The first gate's exclusions were the wrong half of `CHECK_SYSTEM`.** They
  were ported from the check leg, whose job is precision — they decide what an
  author is *warned* about. `prose` P4's defect is a rule restated without the
  existential gate the code applies, and the ported text says outright that
  "the text says less than the evidence shows" is not a disagreement. The gate
  was told to score it low and did. Rewriting for recall (`pick_recall`) was the
  principled fix and **measured worse on both corpora**: it raised negatives
  faster than positives. P4 stays at 0.28–0.46 under every variant.
- **The classifier is too strict for prose.** It marked *"The count admits
  only records matching a claim's current digest"* 0.34 CURRENT because the
  sentence sits inside an argument — and it is `tet47` P50's defect. Discounting
  only PROPOSED (`pick_prop`) should have fixed that and instead dropped `prose`
  AUC to 0.46.
- **`pick`'s `prose` scores were partly right for the wrong reason.** On
  `tet56` P27 it put 0.45 on a sentence that is fine and 0.06 on the one that
  is wrong. The subject scored high; the pointing did not.
- **Position matters and evidence-first costs.** Same bytes, reordered:
  `fact` 54% → 22%.

**On `prose`: no version separates.** Best AUC 0.69, and the only presentation
with a usable zero-loss point (`pick_cls`, 20% skipped) got it by pushing two
defects *down* — its AUC is 0.55. `prose` verification is off by default
(`verify.verbs` is `claim,fact`, see `docs/verify.md`), so this gate would guard
a check that does not run; it is kept as a stress test, not a decision input.

**How much to believe "no defect lost".** The threshold sits just under the
lowest of *n* known defects. A new defect exchangeable with them falls below
the lowest with probability about 1/(n+1): **~8% on `fact`** (n = 12), before
the margin, which is why the table reports skips at 0.05 below it. It is fitted
in-sample on 12 points with nothing held out; every variant tried is in the
table so the winner is not the only thing on record.

    python3 gate_variants.py fact_v1.json --variant pick_cls --draws 3
    python3 score_gate_variants.py fact whole split pick atoms pick_recall pick_cls \
        pick_both pick_prop pick_clause pick_evfirst

### The claim verb — on by default, and a modest gate

`claim` is the verb that ships on (`verify.verbs` defaults to `claim,fact`) and
had no gate measurement at all. Its population is `retrodict.py`'s: 152 claims
at their first-render wording, against the cited facts plus the overlap set,
which is what `verify.rs` assembles for a claim. **Running it found a defect in
that reconstruction** — see "Null read as empty" below; every claim-gate figure
here is from the corrected one.

**What a claim gate must not skip had to be adjudicated first.** "Every refuted
claim" is the wrong target: the check leg itself flags only 3 of the 9, so
gating cannot lose the other six. "Everything the check leg flags" is too
conservative: the flagged claims Jev scores lowest are LLM false alarms. So the
23 flagged claims were graded — `claim_flagged_adjudicated.md`,
`labels_claim_v1.json`: **10 carry a warning worth printing, 13 are false
alarms**, and all 10 are corroborated by a later grounding pass's note on the
same clause. Positives below are those 10, over the 125 claims the check leg
has results for.

Nine variants, three draws each, 27 draw files:

| variant | lever | AUC | spread | skipped, none lost (−0.05) | net | warning precision after gate |
|---|---|---|---|---|---|---|
| `whole` | the first gate | 0.69 | 0.18 | 14% | 8% | 53% |
| `split` | one noul per sentence | 0.77 | **0.42** | 27% | 20% | 53% |
| `pick` | choice over sentences | 0.74 | 0.11 | 21% | 14% | 56% |
| `pick_cls` | + classify | 0.75 | 0.13 | 14% | 7% | 53% |
| **`pick_clause`** | **choice over clauses** | **0.77** | **0.13** | **27%** | **21%** | 53% |
| `atoms` | one noul per checkable token | 0.74 | 0.19 | 10% | 2% | 50% |
| `split_cites` | `split`, cited facts only | 0.69 | 0.24 | 5% | 1% | 48% |
| `pick_cites` | `pick`, cited facts only | 0.63 | 0.14 | 10% | 6% | 53% |
| `pick_clause_cites` | `pick_clause`, cited facts only | 0.65 | 0.15 | 16% | 13% | 53% |

Ungated warning precision is 43% (10 of 23). Check leg $0.00561 a claim.

**`pick_clause` is the claim gate: 21% net, AUC 0.77, and stable.** `split`
matches it on the headline and moves a subject by up to 0.42 between draws,
which is not a threshold anyone can set. The mechanism is the one that won on
`fact`, adapted: a claim is usually one long sentence, so `pick` over sentences
degenerates to a yes/no, and cutting at clauses restores the competition.

**Showing only the cited facts lost everywhere** (0.63–0.69), though it halves
the evidence (9.5KB against 18.8KB at the median). `tet56` C1's defect — the
untracked file behind "a clean working tree" — drops from 1.00 to 0.12–0.44
without the overlap set. The gate needs what the check needs.

**Classification hurts here** (`pick_cls` 7%): a design claim is often
legitimately about what the design builds, and discounting PROPOSED removes the
score from claims whose defect sits in a clause about today.

**Against `fact`'s 58%, 21% is modest, and it is fitted on 10 points** (a new
defect falls below the lowest about one time in eleven). The gate's second
effect is the one the check leg cannot produce for itself: skipping 4–5 of the
13 false alarms raises the precision of what an author is shown from 43% to
53–56%, at no loss of a correct warning.

    python3 gate_variants.py claims --variant pick_clause --draws 3
    python3 score_gate_variants.py claim whole split pick pick_cls pick_clause atoms \
        split_cites pick_cites pick_clause_cites --positives adjudicated

## Result, 2026-09-18 — classify and literals: the legs that grade what code already named

Both legs have the shape the refuter had and the gate lacked: the string work
can move into code, and what is left for the model is a classification.
`CLASSIFY_SYSTEM` says *"You are only sorting the author's own words"*, so the
split can be mechanical. `LITERALS_SYSTEM` already leaves the model one
judgement, *"every factual part of the finding is then decided in code"*, so
candidate generation can be code too. Screened at one draw, finalists
confirmed at three. $0.06 on classify and $0.17 on literals.

### Classify — on by default: equal quality downstream, at 39% of the cost

`classify_jev.py`. Clauses cut mechanically, one `choice` per clause over
`CLASSIFY_SYSTEM`'s three definitions verbatim, all of a claim in one call, no
evidence (the LLM leg sees none either). There is no adjudicated classify
ground truth, so two measures over the 152 corpus claims:

- **decisive clauses**, from the adjudications: the clause of each of the 10
  CORRECT claim warnings (`labels_claim_v1.json`) must reach the check as
  *current*, or the check is told it cannot speak to it and the warning dies;
  the clause of each of the 3 proposal-read-as-current false alarms (tet56 C8,
  C11, C19) should not. The LLM classifier is scored the same way, from the
  labels `retro_full125x3.json` stored.
- **agreement** with the LLM classifier, per character: can Jev stand in for
  it, whether or not either is right.

| | correct warnings kept | proposal false alarms kept away | agreement with LLM | $/claim |
|---|---|---|---|---|
| LLM classifier (majority of 3) | 10 / 10 | **0 / 3** | — | part of $0.0056 |
| Jev, sentences | 7 / 10 | 3 / 3 | 65% | $0.00004 |
| Jev, clauses | 9 / 10 | 2 / 3 | 75% | $0.0001 |
| Jev, clauses outside brackets | 10 / 10 | 2 / 3 | 76% | $0.0001 |
| **Jev, clauses outside brackets, current at P ≥ 0.4 — 3 draws** | **10 / 10 every draw** | **2 / 3 every draw** | **75%** | **$0.0001** |

**On the clauses that decide an outcome, Jev is strictly better**: every
warning the LLM classifier lets through, plus two of the three false alarms it
does not stop. The two levers were both about presentation. The plain clause
cut split `{ id, proposition, cited fact ids, withdrawn }` into four one-word
units; cutting only outside brackets and code spans recovered the lost warning.
`tet61` C15's clause is a near-tie (P(current) 0.58 / 0.53 / 0.44 across draws)
because its neighbours are proposals, and treating a unit as current at
P ≥ 0.4 rather than by plurality holds it in every draw — the asymmetry is the
reason (a hidden current clause loses a warning; a visible proposed one costs a
false alarm), but the 0.4 was set on this one case. 96% of unit labels are
identical across three draws.

    python3 classify_jev.py --unit clause0 --draws 3
    python3 classify_jev.py --score

**Settled end to end, 2026-09-18 — equal quality, 39% of the cost, and the
decisive-clause advantage does not survive the check.** The labels only matter
through what the check leg does with them, so both arms were re-run the same
day on the corrected reconstruction — `retrodict.py --question split --arm union
--repeat 3` over the same 125 claims, differing only in `--classifier`
(`retro_classify_{llm,jev}_x3.json`). The August run was not reused: it was made
on the defective reconstruction, and the model has moved since (its cost for
the identical job doubled, $2.05 to $4.07). Every flagged claim was graded:
the claim adjudication's labels where a finding lands on a clause it graded,
and 18 claims neither arm had been graded on read by hand
(`score_classify_ab.py`, `ADJUDICATED`).

| | flagged | correct | wrong | precision | sound claims flagged | same answer every draw | cost |
|---|---|---|---|---|---|---|---|
| August, LLM classify (defective inputs) | 23 | 10 | 13 | 43% | 4 | — | $2.05 |
| **LLM classify + check** | 26 | 11 | 15 | 42% | 6 | 102 / 125 | $4.07 |
| **Jev classify + check** | 27 | 11 | 16 | 41% | 8 | **111 / 125** | **$1.59** |

- **Same catches, different ones.** 11 correct each; the LLM arm alone has
  `tet47` C18 (a one-draw-in-three finding in August too), the Jev arm alone
  `tet56` C3.
- **The LLM's classify call is half the claim verb's spend**, not the cheap
  half. Measured on the draws where nothing was labelled current and the check
  never ran, it costs **$0.0054 a draw** on its own, against $0.0110 for the
  pair. Its only input is the claim text, so that spend is output — at
  reasoning effort high, most plausibly reasoning. Jev's labels cost $0.0001.
  Jev also leaves the check nothing current more often (29 draws of 374,
  against 13), but that accounts for about $0.07 of the $2.48 difference.
- **The decisive-clause result did not carry through.** Kept away from the
  check as `argument`, `tet56` C19's clause was still objected to in 2 of 3
  draws (3 of 3 with LLM labels); C11 went from 1 of 3 to 0. The check leg
  reads the labels as advice and objects to proposals regardless — which is the
  check prompt's problem, not the classifier's.
- **Sound claims flagged, 8 against 6, is inside draw noise** — the two arms'
  flagged sets differ by 13 claims, and the same LLM configuration moved from
  4 to 6 between August and today — but read literally it puts the Jev arm over
  the design's 7-of-62 line and the LLM arm under it. It is a claim-verb
  default: that is a reason to re-measure before shipping, not to drop it.
- **More stable**: 111 of 125 claims get the same answer in every draw, against
  102. The labels vary less, so the check's input varies less.

    python3 retrodict.py --question split --arm union --repeat 3 --subset claims125.json \
        --classifier jev --out retro_classify_jev_x3.json
    python3 score_classify_ab.py retro_classify_llm_x3.json retro_classify_jev_x3.json

### Literals — off by default, and Jev matches the LLM at a sixth of the cost

`literals_jev.py`. Code proposes every number, cardinal word and path in the
claim with the word it counts ("918 seconds", `acks.jsonl` — the shape the LLM's
own surviving literals take); the shipped filters run unchanged; Jev answers
two nouls per survivor — a quantity stated as current fact, and carried by the
capture in another form, both ported from `LITERALS_SYSTEM`. Same 88 claims,
scored by `literals_eval.summarise`, the scorer that produced the LLM's row.

| | flagged | precision | recall | sound claims flagged | refuted surfaced | literals stable in every draw | $/draw |
|---|---|---|---|---|---|---|---|
| LLM, shipped prompt (`literals_final_88x3.json`) | 10 | 80% | 16% | 2 / 38 | 0 / 9 | 6 / 29 | $0.0013 |
| **Jev, P(quantity) ≥ 0.7** | **11** | **82%** | **18%** | **2 / 38** | **1 / 9** | **17 / 21** | **$0.00023** |
| Jev, P(quantity) ≥ 0.5 | 17 | 71% | 24% | **5 / 38** | 2 / 9 | 26 / 30 | $0.00023 |
| Jev, one noul per criterion | 7 | 86% | 12% | 1 / 38 | 1 / 9 | 12 / 16 | $0.00024 |

Three-draw majority throughout. **Jev at 0.7 matches or beats the LLM on every
claim-level column**, is far more stable, and costs a sixth. At 0.5 it flags 5
of 38 sound claims — 13%, over the design's 11.3% steering-hazard line — so
that setting is out.

**At the literal level it is noisier, and one gap is a capability.** It shows 19
lines on 11 claims against the LLM's 12 on 10. Some are numbers that measure
nothing ("Two things", "five things"); asking that criterion as its own noul
cut them and cost half the recall. The rest are values the capture carries only
after arithmetic — "1690 seconds", "ten records", on `tet30` C3 beside the real
catch "918 seconds" — which `LITERALS_SYSTEM` asks the model to do and a model
that computes nothing cannot. Dropping a literal whose bare value is a token in
the capture removed one line, and it was a real catch ("38 lines"): rejected.

The grader-note measure (70% for the LLM, 31% for Jev) is not used to decide:
a grader names values it confirmed as readily as values it refuted, so "1690
seconds" scores as named.

**What it changes:** `literals` is off because it costs a call per mint for a
thin niche (docs/verify.md: "roughly +50% on `split`"). At $0.00023 that cost is
about +4%.

**Cost is not what binds, though — attention is.** docs/verify.md says to judge
the leg by what it adds, so, on the same 88 claims, against the check leg's own
flags (`retro_full125x3.json`, majority of 3):

| | needed work, flagged (of 50) | sound claims flagged (of 38) |
|---|---|---|
| check alone (ships) | 18 | 4 — 10.5% |
| check + LLM literals | 23 | 6 — 15.8% |
| check + Jev literals | 23 | **5 — 13.2%** |
| Jev claim gate (`pick_clause`) + check | 15 | 3 — 7.9% |
| **gate + check + Jev literals** | **20** | **4 — 10.5%** |

Added to today's check, either literal leg crosses the design's 11.3%
steering-hazard line. Behind the Jev claim gate, which removes false alarms,
Jev literals fit under it: two more claims that needed work, today's
false-alarm rate, and a cheaper mint than today's, since the gate skips a fifth
of the check calls. The gate's 18 → 15 is not a lost correct warning — this
join credits a flag on any claim later qualified, and the three it drops are
warnings `labels_claim_v1.json` grades WRONG. Each sound claim is 2.6 points
of that rate, so the line sits one claim away either way.

    python3 literals_jev.py --draws 3 --out literals_runs/jev_x3.json
    python3 literals_jev.py --summarise literals_runs/jev_x3.json --q 0.7 --c 0.5

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

## Five harness defects worth not repeating

Each inflated or hid a result before being found, and all four are fixed here.

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

**A filter that deleted what it filtered.** `refute.py` writes only the
surviving findings back, which also discards the scores of the dropped ones.
That is harmless for a verdict and fatal for a probability: the first threshold
sweep over the Jev run could only see findings the `choice` arm had already
kept, so it was measuring the survivors of one threshold against another and
reported a flat line. `refute_jev.py` keeps every finding with its refutation
attached and `score_refuter.py` applies the keep rule itself, which also makes
the two file shapes gradeable by one join. Re-validated against the Sonnet and
Gemini runs after the change.

**The scorer hardcoded one finding kind.** `kept_keys` filtered to
`contradicts`, which is the whole `fact` adjudication and two thirds of the
`prose` one. On `prose` it silently dropped 16 `overreaches` findings — one of
them a true catch, `tet61 P4`, which the scorer then reported as *lost by the
refuter*. A defect that turns the harness's own omission into a finding about
the thing it measures. The population is now defined by the label set, which is
what the label set is for.

**Null read as empty, in the claim reconstruction.** A claim-log `Revise`
carries `prop: null` when only the cites changed and `from: null` when only the
text did — null means *unchanged*. `retrodict.load_memo`, and the copy of it in
`literals_eval.py`, took both literally. Of 152 claims, **3 went to the model
with the wording `None`** (`tet30` C11, `tet56` C5, `tet61` C8) and **11 were
compared against no evidence at all**. None of the 14 is a refuted claim, so no
recall figure on this page moves. What it touched: `retrodict_union.json` — the
"is this supported?" arm — flagged **all 14**, five of them supports-only
(`tet29` C13, `tet46` C11, `tet47` C10, `tet47` C11, `tet61` C8), which is what
a claim shown nothing is bound to draw from that question; `retro_full125x3.json`
and the scope arms flagged none of them, so the design's kill-condition figures
(4 / 62) stand; and the literal check's 88-claim population silently excluded
the empty-cite claims, so its denominators are from the defective
reconstruction. Found by the Jev claim gate, whose `sentences()` raised on a
`None` wording where an f-string had quietly printed it. Fixed in both copies,
2026-09-18; no earlier result file was re-run.

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
| `gate_jev.py` / `score_gate.py` | the gate before the check leg, and its scoring |
| `gate_variants.py` / `score_gate_variants.py` | the gate iterated: every presentation tried, one lever each, scored on one footing; `claims` runs the claim verb |
| `gate_runs/` | every draw of every gate variant, three per variant per corpus |
| `defects_v1.json` | subject-level ground truth for a gate: the union of all three adjudications |
| `classify_jev.py` / `classify_runs/` | the classify leg asked of Jev: mechanical clauses, one `choice` each |
| `score_classify_ab.py` / `retro_classify_{llm,jev}_x3.json` | the check leg fed LLM labels vs Jev labels, same day, same claims, every flag graded |
| `literals_jev.py` / `literals_runs/` | the literal leg asked of Jev: code proposes, shipped filters, two nouls per survivor |
| `claim_flagged_adjudicated.md` / `labels_claim_v1.json` | the 23 claims the claim check leg flags, graded: 10 warnings worth printing, 13 false alarms |
| `jev.py` | the System One adapter — `one_call`'s sibling, and a smoke test |
| `refute_jev.py` | the refutation pass asked of Jev: one call, two typed questions |
| `labels_fact_v1.json` / `labels_prose_v1.json` | the two adjudications, machine-readable |
| `score_refuter.py` | grades any refutation against those labels; reproduces the published rows |
| `fact_refuted_jev*.json`, `prose_refuted_jev*.json` | the Jev runs the 2026-09-17 tables report |
| `*_refuted.json`, `*_refuted_gemini.json` | the same findings put to a second model — `anthropic/claude-sonnet-4.5` and `google/gemini-2.5-pro`, scored against the same adjudications, which is what makes the refuter a dial rather than a fixed price |
