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
| `verify.timeout_ms` | integer ≥ 1000 | `60000` | how long one verification may take, **end to end across retries** |
| `verify.verbs` | any of `fact`, `claim`, `prose` | `claim` | which verbs are verified |

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

Every response also echoes the five settings in force (`model`, `approach`, `timeout_ms`, `verbs`)
plus `deterministic: false` and a `guidance` string — because tetel only admits a setting that is
visible in the output it affects, and three of those five would otherwise be invisible.

### A finding

```json
{
  "kind": "contradicts",
  "clause": "counted.rs defines exactly two functions",
  "fact": "F1",
  "evidence": "fn a() {}\nfn b() {}\nfn c() {}",
  "why": "The captured file defines three functions, not exactly two.",
  "quoted": true
}
```

- `clause` is **your** wording, the part being judged.
- `evidence` is the captured span it was judged against, present **only** when that span was verified
  as a verbatim substring of a single captured observation — the same relation `transplant` uses to
  refuse a premise that is not the donor's own words.
- `quoted: false` with no `evidence` means the model offered a span that could not be found in the
  captured record. The finding still ships; its quotation does not.
- There is **no confidence score**, deliberately. A number invites deference; a quotation invites
  checking, and checking is the only safe response when the check is wrong.

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

The one bucket the corpus cannot settle: 60 claims carried a *qualifies* verdict and 18 were flagged.
A qualification is often legitimate scope-narrowing rather than a defect, so nothing in that data
says whether those flags were welcome or noise. That is what `verify-report` exists to answer, going
forward, on your own memos.

---

## Where findings are logged

Each workspace keeps `verify.log`, one JSONL record per completed verification, carrying the
findings, the status and why a failure failed, plus `cost`, `elapsed_ms` and `attempts`. It also keeps
the spans that failed quote verification — withheld from you at mint time, kept here, because a
fabrication rate you can never look at is not one you can act on.

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
  findings         2
  quoted verbatim  1   (50%)
  span rejected    1

  [F1] proposition number 2
    the model offered: pub fn never_captured() -> usize
```

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
