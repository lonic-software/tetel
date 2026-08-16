# Adjudication: the 28 `contradicts` of the shipped prompt (`fact_123.json`)

Same standard and same adjudicator as
[the v1 set](fact_v1_contradicts_adjudicated.md), so the two compare.
`fact_123.json` was run against `shipped("CHECK_SYSTEM")`, read out of
`src/verify.rs` — this is what the tool does today, after the `overreaches`
bound.

**Result: 11 CORRECT, 17 WRONG — 39%.** Against v1's 10 of 16 — 63%.

## The comparison that decides the prompt

| after the kind bound | flag rate | findings | correct | wrong | **distinct defects caught** |
|---|---|---|---|---|---|
| shipped `CHECK_SYSTEM` | 22.0% | 28 | 11 | **17** | **11** |
| `prompt_fact_v1.txt` | 12.2% | 16 | 10 | **6** | 9 |

v1 removes 11 of the 17 false alarms and costs 2 of the 11 catches. It is not
free, and the two it loses are real:

* `tet30` F6 — *"The loop is defined purely over digests and record membership"*,
  against a captured line branching on `r.derived_kind() == Kind::Attested`.
* `tet47` F6 — *"The `Predicate::pin` doc says 'Nothing reads it yet.'"* The doc
  comment is followed by `world`, then `note`, then `pin`; it documents `world`,
  and `Predicate::pin` carries no doc at all. This is the premise the design's
  rule 3 rests on.

v1 gains one the shipped prompt misses, and it is the subtlest catch in either
set: `tet30` F8, where the note states the executed rule as *"some record's
digest equals sha256 of its current proposition"* and the rule as actually run,
inside a forty-line embedded Python script, is `if dg==cur or dg=='':`.

## What v1 kills, cluster by cluster

Every one of the 11 false alarms v1 removes is answered by a numbered rule in
`prompt_fact_v1.txt`. The prompt was written against these and it worked.

| cluster | shipped | v1 | rule |
|---|---|---|---|
| "verbatim" vs a lossy UTF-8 decode (`tet56` F2, F30) | 2 | 0 | 3 |
| "byte-equivalent" vs whitespace and visibility (`tet30` F9) | 1 | 0 | 3 |
| scope re-read wider than its own sentence (`tet61` F3, F6, F12; `tet56` F17 ×2, F20) | 6 | 0 | 2 |
| reasons its way to "so there is no disagreement", reports anyway (`tet30` F12) | 1 | 0 | 4 |
| objects to a clause it misquoted (`tet47` F16) | 1 | 0 | — |

`tet61` F3 is the clearest of them. The note reads *"skips blank lines, but maps
ANY line's `from_str` failure to `Err(…)` — the whole log fails, no line is
skipped."* The finding reports that blank lines are skipped, which the same
sentence says eight words earlier. Rule 2 of v1 is written from this case.

One shipped false positive is not a cluster and is worth naming on its own:
`tet61` F19 asserts the capture holds 72 matching lines against the note's 73.
Counted directly, the evidence holds 73. The verifier was simply wrong, and v1
does not raise it.

## The 11 correct

`tet30` F2 (*ships exactly 9* vs `if src.exists()`) · `tet30` F6 (*purely over
digests*) · `tet30` F7 (*every multi-record group … two or three distinct
passes*, against `('C15', …)` twice from one pass) · `tet47` F6 (pin doc
attribution) · `tet47` F30 (*FOUR hits, not three*, against three) · `tet56` F9
(*return and capture are the same bytes*) · `tet56` F29 (*a single line above
32768 bytes*, against the note's own `positions [11, 15]`) · `tet61` F14 (*12
occurrences*, against its own 6+3+3+1) · `tet61` F18 (*the idiom `let _ = …`*,
against nine `.ok()`) · `tet61` F20 (*two lines* of `117-119`) · `tet61` F21
(*a search for `mod tests {`*, against `(grep: mod tests)`).
