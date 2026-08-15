# The mint-time verifier

**A second pair of eyes at the moment of writing.** When you mint a fact, assert a claim or write a
paragraph, the verifier compares what you just wrote against the evidence tetel already captured for
it, and tells you when the two disagree.

It is **off by default**, it makes a network call, and it is wrong about one finding in six. All
three of those are load-bearing; this document is mostly about what they mean in practice.

The design that produced it, including the test it half-failed, is
[`docs/design/tet-verifier-mint-warning.md`](design/tet-verifier-mint-warning.md).

---

## What it does, exactly

Every authoring verb writes text, and every piece of that text rests on evidence tetel holds:

| verb | what you wrote | what it is compared against |
|---|---|---|
| `claim` | the proposition | the captured output of the facts you cited **plus the overlap set** |
| `fact` | the note | that fact's own captured output |
| `prose` | the block's text | the facts under the claims the block cites |

A model reads both sides and reports **disagreements only** — of which there are exactly two kinds:

- **`contradicts`** — the evidence shows something incompatible with what you wrote: a different
  number, name, type, line, or behaviour.
- **`overreaches`** — what you wrote ranges wider than what was captured. It says *every*, *never*,
  *only*, *no*, *always* or *cannot* about a population the evidence samples rather than covers.

Nothing else counts. Evidence that fails to fully *establish* your claim is not a disagreement;
"the captured material doesn't touch X" is not a disagreement; a claim saying *less* than the
evidence shows is not a disagreement. Those rules are in the prompt because without them the thing
reports insufficiency all day, which is noise.

### Why the overlap set is in there

For a `claim`, the captured side is deliberately **not** just the facts you cited. It is those facts
*together with* every uncited fact touching the same files — the overlap report `claim` already hands
you.

This is the whole reason a semantic comparison is worth building. `scope.rs` exists because tetel's
original `Domain ⊆ Extent` check was abandoned as vacuous: with one author supplying both sides,
they agreed byte-for-byte about four times in five. What rescued it was making one side *structural*
— an extent is captured, and there is no flag to type one in. For a claim, half of that survives (you
cannot type a cited fact's output) and half does not (you choose which facts to cite). An
overreaching proposition could be made to agree with its evidence by citing only the facts that
agree with it. Adding the overlap set closes that: neither side can be typed, and neither can be
narrowed by selection.

---

## What it is not

- **Not a refusal.** It cannot fail a mint, delay a reply, or move an exit code. Ever.
- **Not part of `check`.** It does not run inside `check`, does not enter the evidence ledger, the
  memo or the snapshot, and appears in neither of `check`'s two partitions. `check` makes no network
  calls and never will — a memo arriving in a pull request must not be able to make the tool that
  checks it reach anywhere.
- **Not reproducible.** The same input can produce a different answer. Every response carries
  `deterministic: false` for that reason, sitting as it does beside two findings (`attention`,
  `overlap`) that *do* recompute identically every time.
- **Not a grading pass.** It compares wording to captured bytes. It does not know whether your claim
  is true, and it never sees another workspace's verdicts.

---

## Turning it on

Two things are needed, and they live in different places on purpose.

**1. A key, from the environment.** Never from a config file — those get shared, committed, and
pasted into issues.

```sh
export OPENROUTER_API_KEY=...      # or TETEL_API_KEY
```

There is no `verify.api_key` setting and `tetel config` refuses to create one. `verify.model` also
refuses a credential-shaped value, and its refusal does not echo what you gave it, in case you
pasted a key.

**2. The settings.**

```sh
tetel config verify.enabled true
tetel config verify.model openai/gpt-5.6-luna
```

That is the minimum. Everything else has a default. Check what is in force with:

```sh
tetel config                       # every setting, its value, and which file supplied it
```

Settings live in `~/.config/tetel/config.toml` (or `$TETEL_CONFIG_HOME` / `$XDG_CONFIG_HOME/tetel`),
and a workspace can override any of them in its own state directory with `--workspace-scope`.

---

## Settings

| key | accepts | default | what it decides |
|---|---|---|---|
| `verify.enabled` | `true` / `false` | `false` | whether any comparison happens at all |
| `verify.model` | `vendor/model` | *(none)* | which model compares. No default — nothing runs until you set one |
| `verify.approach` | `split` / `direct` | `split` | one call or two — see below |
| `verify.timeout_ms` | integer ≥ 1000 | 60000 **per call** | how long one verification may take, **end to end across retries**. Unset, the default scales with the number of calls: 60s `direct`, 120s `split`, 180s with `literals` |
| `verify.verbs` | any of `fact`, `claim`, `prose` | `claim` | which verbs are verified |
| `verify.literals` | `true` / `false` | `false` | whether to also report literals your text states and no capture carries — see below |

### `approach`

- **`split`** (default) — two calls. The first labels each assertion in your text as `current` (about
  how things behave today), `proposed` (about what this design will build) or `argument` (a reason or
  entailment). The second checks, and may only report against `current` ones. This matters: without
  it, the verifier reports your *proposals* as contradicted by code that predates them.
- **`direct`** — one call instead of two, so cheaper, but **not by half**: the call it drops carries
  only your claim, while the one it keeps carries the evidence. One-call arms on the same corpus cost
  between 0.26 and 0.65 of `split` depending on the prompt; the ones that report disagreements — the
  shape a finding has to have — sit at **0.63–0.65**, so expect about a third off. It won an earlier
  fifteen-case evaluation on synthetic cases; one-call comparisons *have* been run over real memos,
  but not with this arm's prompt pairing, and no one-call figure was carried into the decision to
  ship. `split` is the default because `split` is the configuration whose numbers are on the page.

The design also names a third mode — a three-call pipeline whose findings are re-derivable without a
model — and it is **not built**. `verify.approach extract` is refused by name rather than quietly
served by one of the two above.

### `literals`, and why it is off

> **Measured on 2026-08-15.** Judge it by what it adds, not by what it scores alone. Turning it on
> takes the verifier's recall from **30% to 38%** at a cost of three points of precision (83% → 80%)
> — it finds seven claims the other two kinds miss entirely, five of which needed work. It still
> surfaces none of the nine claims a grading pass later refuted, which is why it is off by default:
> it widens the net, it does not save you a grounding round.

This adds a **third finding kind** to the two the retrodiction measured, so it gets its own call and
its own prompt: what the gate measured stays byte-identical with this off, and the numbers below
keep describing what earned them.

**What it looks for is narrower than "a literal".** A finding survives only if it names something
mechanically checkable — a value carrying a digit, a cardinal word (`four`, `one unit test`), or a
file path. A bare symbol, flag or version string is dropped in code, no matter what the model says
about it. That filter is the difference between 41% precision and 80%: instructed to skip
identifiers, the model stopped naming symbols and started naming *quantifiers* — `any depth`, `no
exclusions at all` — which is `overreaches`' job, already done and better measured.

**Know what it overlaps.** `contradicts` is defined as "a **different number**, name, type, line, or
behaviour", so a wrong value is already its job, and on the sharpest example this check has,
`contradicts` did it better — it named the right figure rather than only reporting an absence. The
niche that remains is genuine but thin: a value appearing **nowhere** in the capture, as against a
value the capture **disagrees with**.

With it on, each verification makes **one extra call**, which asks a narrower question: does your
text state a *literal* — a number, a count, a size, a version, a file path, a symbol name, a flag, a
quoted string — as a fact about how things behave **today**, where nothing you cited carries it?

```sh
tetel config verify.literals true
```

The model's job here is only the judgement: is this literal an assertion of current fact, and could
a capture have carried it. **Every factual part of the finding is then decided in code**, by two
filters the model cannot talk its way past:

1. the literal must be a verbatim substring of your own text, or the finding points at nothing;
2. no observation the model was shown may contain it — which is the entire assertion, and is a
   substring search anyone can rerun.

That second filter is what makes this kind cheaper to trust than the other two: a `contradicts`
finding rests on the model's reading, an `unevidenced` one rests on a search you can repeat. It also
biases hard toward silence, and deliberately. A literal occurring *anywhere* in the capture is
dropped, so a `40` inside a line range will suppress a real finding about a different `40`.
Under-reporting is the right direction for an advisory that costs you attention.

One cost to know before turning it on: **one more call per mint** — roughly +50% on `split`, +100%
on `direct`. See [what it costs](#what-it-costs).

**A failure in the literal leg no longer fails the verification.** If it times out or comes back
unreadable, the status stays `ok`, the disagreement findings arrive intact, and the reply carries
`literals_incomplete` naming what went wrong:

```json
{ "status": "ok", "findings": [], "literals_incomplete": "timeout" }
```

This used to discard everything, on the argument that `ok` should mean the whole configured
comparison happened. That principle is right and the trade was wrong: the disagreement kinds carry
30% recall against this one's 13%, so losing the stronger half because the weaker half returned a
429 costs more than it protects. The principle is kept by *reporting* rather than by failing — with
`literals_incomplete` present, "found no literals" and "never asked" stay different payloads, which
is the whole reason `verify` is an object.

`verify.timeout_ms` bounds the whole verification end to end, not each call, so its **default scales
with the number of calls**: 60s per leg, meaning 60s for `direct`, 120s for `split`, and 180s for
`split` with literals on. Measured over the corpus a single call's median is under 10 seconds and
its p90 around 50, so a flat budget would have left `split` no headroom and three legs none at all.
Set it explicitly and your number is used as-is:

```sh
tetel config verify.timeout_ms 120000
```

One consequence to know for `verify-report`: a claim flagged only by an `unevidenced` finding still
counts as flagged in the precision and recall join, because that join reports what you were actually
shown. The report says how many flags came from the literal check alone, so you can read those two
fractions knowing which part of them has an evaluation behind it.

### `verbs`, and why the default is `claim` alone

- **`claim` is on** because it is the comparison nothing in tetel performs today. `attention` reads a
  note against its own extent; `overlap` reports ids and shared file paths, never notes. No existing
  check reads a proposition against the *content* of the evidence cited for it.
- **`fact` is off** because half its work is already done, deterministically and for free, by the
  `attention` array — and nothing has measured how large the remaining semantic residue is.
- **`prose` is off** because it is the highest-volume verb of the three and prose-against-propositions
  is the comparison with the least evidence behind it. Turning it on spends the largest share of the
  budget on the least-evidenced check.

Turn more on when you want them:

```sh
tetel config verify.verbs "claim,fact"
```

---

## What comes back

Every `fact`, `claim` and `prose` result now carries a `verify` object. It is an object rather than a
third array for one reason: **"found nothing" and "did not look" must not be the same payload.** A
model call can be switched off, refused for want of a key, fail in transport, time out, or come back
unreadable, and an empty array would read as a clean bill in every one of those states.

`status` is mandatory, and always one of:

| status | meaning |
|---|---|
| `off` | disabled, or this verb is not in `verify.verbs` |
| `unauthorized` | on, but nothing to call with. `detail` says which — an unset `verify.model` or a missing key |
| `queued` | a verification started for this mint. `queued_for` names it. Ask again on your next call |
| `skipped` | the verb is on, but this call had nothing to compare — a heading, a block citing no claim, a withdrawal, or a revision that left the compared text unchanged |
| `ok` | a verification completed. **`findings` is meaningful only here** |
| `unavailable` | transport failure, a non-2xx reply, or a draw that came back empty |
| `timeout` | the budget expired |
| `unparsable` | a good reply whose content was not a usable answer |

Under any status but `ok`, **there is no `findings` key at all**. Do not treat its absence as "no
disagreements found".

Every response also echoes the settings in force (`model`, `approach`, `timeout_ms`, `verbs`,
`literals`) plus `deterministic: false` and a `guidance` string — because tetel only admits a setting
that is visible in the output it affects, and four of those five would otherwise be invisible.

### A finding

```json
{
  "kind": "contradicts",
  "clause": "counted.rs defines exactly two functions",
  "clause_quoted": true,
  "facts": ["F1"],
  "evidence": "fn a() {}\nfn b() {}\nfn c() {}",
  "why": "The captured file defines three functions, not exactly two.",
  "quoted": true
}
```

- `kind` is `contradicts`, `overreaches`, or — only when `verify.literals` is on — `unevidenced`.
- `clause` is **your** wording, the part being judged.
- `evidence` is the captured span it was judged against, present **only** when that span was verified
  as a verbatim substring of a single captured observation — the same relation `transplant` uses to
  refuse a premise that is not the donor's own words.
- `quoted: false` with no `evidence` means the model offered a span that could not be found in the
  captured record. The finding still ships; its quotation does not.
- There is **no confidence score**, deliberately. A number invites deference; a quotation invites
  checking, and checking is the only safe response when the check is wrong.

#### The two fidelity marks

Both system prompts demand two quotations verbatim — your clause, and the captured span. Neither is
taken on trust, and each finding says how it fared:

- **`facts`** lists every cited fact whose captured output contains the span. Not the model's word
  for it: the model is never asked which fact it read, because it is never asked to track ids. An
  empty list means no capture contained the span, which is the same thing `quoted: false` says. **A
  list of more than one is not a bug** — a short span (a number, a path, a common identifier) genuinely
  lives in several captures, and naming all of them is honest where naming the first would send you
  to a fact picked by the order you happened to type `--cites` in.
- **`clause_quoted: false`** means the clause shown is the model's paraphrase, not your words. It is
  reported rather than withheld, unlike a rejected span: the span points *outside* the finding, so an
  unverified one sends you to text that does not exist, whereas the clause points at prose you are
  already looking at, where a paraphrase is still a usable pointer.

#### An `unevidenced` finding

```json
{
  "kind": "unevidenced",
  "clause": "the read buffer is 4096 bytes",
  "clause_quoted": true,
  "literal": "4096",
  "facts": [],
  "why": "No captured output carries this size; the buffer's declaration was never opened.",
  "quoted": false
}
```

It names a `literal` instead of quoting evidence, because **the finding is the absence**. `quoted` is
always `false` here and `facts` always empty — there was nothing in the capture to quote. That is why
`verify-report` scores quote fidelity over the two disagreement kinds alone.

---

## When it arrives

**Nothing in the reply path waits.** Your mint returns immediately, carrying only what can be decided
without a call. The comparison runs afterwards, and its result is delivered **on your next authoring
call in the same workspace**.

```
claim  C7  →  {"status": "queued", "queued_for": "C7"}
claim  C8  →  {"status": "ok", "for_mint": "C7", "findings": [...], "queued_for": "C8"}
```

`for_mint` names which mint the findings concern, because by the time they arrive it is no longer the
id sitting beside them.

This is not fussiness. A mint result already carries findings that arrive with certainty *because* a
mint is instant — `attention`, `folded`, `refused_since_previous_fact`, `overlap`. Putting a provider
call in front of the reply would make every one of them contingent on the provider: interrupt the
call, or have a client timeout shorter than `verify.timeout_ms`, and you would get the record
committed and a note-outside-extent warning thrown away that no model was ever involved in producing.

The price is that a warning lands one tool call after the writing it concerns. What is bought is the
removal of the whole latency budget from your critical path — and one call later is still many rounds
earlier than a grounding pass.

Two related rules. `verify.timeout_ms` bounds one mint's verification **end to end, across every
retry** — a per-attempt bound would quietly license three times the declared spend. And a revision
whose compared text is unchanged makes no call at all, which is where most of the volume would
otherwise go; you get `status: "skipped"` for those.

---

## What it costs

Measured over 366 priced verifications against real memos, in the default `split` configuration:

```
mean     $0.0056 per verification    (one claim, checked once — two provider calls)
median   $0.0043
```

A mint starts at most one verification, and **a revision that leaves the compared text alone starts
none** — the previous wording is read and compared first. So what you pay for is *distinct wordings*,
not log events. That matters: in the largest memo on disk, two thirds of claim traffic is revision.

**About half a cent per wording**, so roughly 20–25 cents for a 40-claim design that settles quickly,
and more for one you rework hard.

Mind the unit. The run that produced these numbers checked each claim *three times*, to see whether
the answers were stable, so it spent $0.0164 per claim. You are not doing that.

The evidence sent with each comparison is bounded at **14,000 bytes**, the same bound the harness used
— otherwise the numbers above would describe a smaller input than the code actually sends. What is
cut is disclosed in the text the model reads, so it can obey the instruction never to report a
disagreement resting on material it was not shown.

And be careful with the much smaller figure in the earlier fifteen-case evaluation — a hundredth of a
cent. That is real, and it is the cost of judging a short synthetic proposition against a tiny
captured extent. A real claim is a paragraph judged against the joined output of the facts it cites
*and* its overlap set, and costs about fifty times more.

`direct` is cheaper than `split` but not by half — the call it drops is the one carrying no evidence.
The closest measured analogues on the same corpus sit at 0.63–0.65 of `split`, so expect about a third
off. Nothing measures the shipped `direct` pairing itself.

`verify.literals` adds one more call, and it is one of the **expensive** ones: like the check call, it
carries the whole evidence blob. Nothing has measured it, so take the arithmetic rather than a
figure — on `split` it is a third call of roughly check-call size, so budget **about +50%**; on
`direct` it doubles the calls and roughly doubles the cost.

Revisions are where the volume is: in the largest memo on disk, two thirds of claim traffic is
revision. A revision that changes the text being compared is a new comparison and makes a new call.

---

## How good is it

The design gated itself on a retrodiction test — replay 125 claims from seven finished memos at their
first-render wording, and compare what the verifier would have said against what the graders actually
concluded later. Three draws per claim, majority vote.

```
claims flagged                              23 of 125
  landed on a claim that later needed work  19      → precision 83%
  landed on a claim that was already sound   4

claims a later pass did not simply support  63
  flagged                                   19      → recall 30%
```

Read that honestly:

- **It rarely disturbs sound work.** 4 flags across 125 claims landed on something already fine —
  about 7% of the sound population. Two declared kill conditions were set in advance and this one
  passed comfortably.
- **It misses most of what needs fixing.** Of nine claims a later pass refuted, it named the defect on
  two. That was the second kill condition, and **it fired**. The verifier does not remove a grounding
  round, and the design withdraws that argument rather than defending it.
- **Expect roughly 8 warnings on a 40-claim design, 6 of them real.** Arriving when the fix costs one
  re-look rather than one revision round, blocking nothing, for a few tens of cents.

**These numbers describe the two disagreement kinds only.** They were produced by the `split`
configuration with `verify.literals` off, and that prompt is untouched by the literal check — the
third kind runs as a separate call with its own prompt precisely so these figures keep describing
what produced them.

### The literal check, measured separately

Same corpus, same method, three draws, majority vote, over the 88 claims whose facts carry usable
captured output:

```
precision                      80%   (8/10)        against 83%
recall                         16%   (8/50)        against 30%
supports-only claims flagged   2/38 = 5.3%         against 6.5%, threshold 11%
refuted claims it surfaced      0/9                against 2/9
named in the grader's own note 70%   (7/10)
cost                           $0.0013 per draw
```

**One kill condition passes, one fires.** It no longer disturbs sound work — 5.3% is below the
threshold and below what the shipped kinds do. But across the nine claims a later pass refuted it
surfaced none, and that is the condition to weigh: this does not save you a grounding round.

### What it adds, which is the number that should decide it

The two checks are separate calls with separate prompts and no shared state, so their results
combine exactly. Over all 125 claims — with the literal leg making no call at all on the 37 whose
facts carry no captured output, as it does in practice:

| | flagged | precision | recall | sound flagged | refuted |
|---|---|---|---|---|---|
| `contradicts` + `overreaches` | 23 | 83% | 30% | 6.5% | 3/9 |
| `unevidenced` alone | 10 | 80% | 13% | 3.2% | 0/9 |
| **all three together** | **30** | **80%** | **38%** | **9.7%** | 3/9 |

**Recall 30% → 38% for three points of precision.** The seven claims it adds are ones the other two
kinds missed entirely, and five of them needed work — 71% precision on the addition itself. Combined
sound-flagging stays at 9.7%, under the 11.3% threshold.

It adds nothing on the refuted claims. That is the real limit, and it is why this is off by default:
it widens the net, it does not catch what forces a revision round.

Where it is good is narrow and worth naming. When it does flag a claim that needed work, **70% of
the time the literal it named appears in what the grader went on to write** — it is pointing at the
thing that turned out to matter, not near it.

Two standing weaknesses. It is **unstable**: 6 of 29 distinct literals were raised in all three
draws, so a finding you see once may not return. And **49% of what it raises is dropped by a filter
before you see it** — 30 literals were in the capture after all, 16 named nothing checkable. That
number is the machinery working, and it is also a measure of how often the model is simply wrong
about the one question it is asked.

The one bucket the corpus cannot settle: 60 claims carried a *qualifies* verdict and 18 were flagged.
A qualification is often legitimate scope-narrowing rather than a defect, so nothing in that data
says whether those flags were welcome or noise. That is what `verify-report` exists to answer, going
forward, on your own memos.

---

## Where findings are logged

Each workspace keeps `verify.log`, one JSONL record per completed verification, carrying the
findings, the status and why a failure failed, plus `cost`, `elapsed_ms` and `attempts`. It also keeps
the spans that failed quote verification — withheld from you at mint time, kept here, because a
fabrication rate you can never look at is not one you can act on. The same reasoning puts
`not_verbatim` and `literals_refuted` on each record: they count what was dropped *before* a finding
reached you, which nothing in the findings themselves can show.

**`verify.log` is never copied into a snapshot.** Snapshot contents are an explicit enumeration and
this name is not in it, so non-reproducible model output cannot travel beside a memo. Nothing in the
log is needed to check a document.

---

## `tetel verify-report`

```sh
tetel verify-report docs/design/my-memo.md          # the join
tetel verify-report docs/design/my-memo.md --spans  # plus the quotations that failed verification
```

It finds the workspace that authored the memo (by matching the identity its snapshot carries), reads
that workspace's `verify.log`, reads the memo's evidence ledger, and joins the two by claim id:

```
VERIFICATIONS   5
  ok             3
  timeout        1
  unavailable    1

  cost           0.0013 total, 0.00026 each
  elapsed        1000ms median
  retried        1

  why the non-ok ones failed:
    - provider did not answer within the remaining budget
    - provider replied 429

FLAGS AGAINST WHAT THE GRADERS LATER SAID
  claims verified  3
  of those graded  3   (ungraded so far: 0, entering no denominator)
  flagged          2
    later needed work  1
    later only supported  1   <- flags on claims that were already sound
  never flagged, later refuted   0   <- what it did not catch

  precision        50%   (1/2)
  recall           50%   (1/2)

QUOTATIONS
  findings         3
  quoted verbatim  1   (50% of 2 evidence-bearing)
  span rejected    1
  span in >1 fact  0   <- attributed to all of them, not the first
  clause verbatim  3   (100% of all findings)
  dropped, not the author's words   1   <- returned as a quotation, absent from the text

LITERALS
  unevidenced      1   <- stated as current fact, in no capture
  machine-refuted  2   <- the literal was in the capture after all
  wrong about 67% of what it raised, by a check anyone can rerun

  [F1] proposition number 2
    the model offered: pub fn never_captured() -> usize
```

The three fidelity lines are what you tune on:

- **`span in >1 fact`** counts findings whose quotation lives in several captures. High is not a
  fault — it usually means short spans — but it tells you how often the attribution is a set rather
  than a name.
- **`clause verbatim`** is how often the model quoted *your* words back rather than paraphrasing.
- **`dropped, not the author's words`** counts what never became a finding at all: classify
  assertions and literals the model attributed to you that were not in your text. These are dropped
  before anything reaches you, and counted here so the drop is visible rather than silent.
- **`machine-refuted`** is the only accuracy number the `unevidenced` kind has. It counts literals the
  model called unevidenced that a substring search found in the capture anyway. Nothing was shown to
  you for those — the filter ran first — but a high rate means the literal check is guessing.

Claims nobody has graded yet leave every denominator rather than counting as correct silences.

It is a CLI command only, not an MCP tool — it reads grader verdicts, which `brief` withholds from
authors on purpose, and it would additionally hand an agent a calibration for how much to ignore the
verifier. Run it yourself.

---

## Limits, stated up front

- **It is wrong about one finding in six**, and an agent that treats a finding as a verdict will
  "fix" correct claims. That is why findings quote rather than score.
- **It misses most defects.** 30% recall. It is a cheap extra look, not a gate.
- **It is non-deterministic.** Two identical mints can warn differently, or warn once and stay silent
  the next time.
- **It requires network access at authoring time.** No `check`, `brief`, `record`, `render` or any
  read-only verb ever reaches the network — only `fact`, `claim` and `prose`, and only when enabled
  with a key present.
- **It has been measured on tetel's own memos and nowhere else.** Seven memos, one corpus, one
  authoring style. `verify-report` is how you find out whether any of it holds for yours.
- **`verify.literals` reaches 80% precision but surfaces nothing that forces a revision.** Zero of
  the nine claims a grading pass later refuted. It is off by default for that reason — turn it on
  for the arithmetic and path errors it does catch, not as a safety net.
