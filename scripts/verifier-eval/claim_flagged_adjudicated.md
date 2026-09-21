# Adjudication: the 23 claims the `claim` check leg flags

The population is every claim `retro_full125x3.json` flags by majority of its
three draws — the shipped classify+check pipeline over `retrodict.py`'s
first-render claims. Graded per **claim**, because the question it answers is a
gate's: would skipping this claim lose a warning worth printing? A claim is
**CORRECT** if any finding raised on it, in any draw, really disagrees with the
capture in the way and of the kind claimed, such that the author would have
wanted to know — `REFUTE_SYSTEM`'s standard, the one the `fact` and `prose`
adjudications use. Default **WRONG** when not convinced.

None of the 23 is among the 14 claims the reconstruction defect mangled (see
README, "Null read as empty"), so every finding here was raised against the
claim's real wording and real evidence.

**Result: 10 CORRECT, 13 WRONG — 43%.** Machine-readable in `labels_claim_v1.json`.

Adjudicator: Opus, one reader. Unlike the earlier sets, each claim also carries
the later grounding passes' own notes, read as a second opinion: an independent
reader, who never saw the check leg's finding, writing down what it found
narrow or wrong. **All 10 CORRECT have a grounding note on the same clause**,
or on its later revision where the author reworded it (tet30 C12, tet47 C6).
That is the strongest corroboration any adjudication on this page has had.

## CORRECT (10)

| claim | the clause | what the capture shows |
|---|---|---|
| tet28 C14 *(minor)* | "a claim is **only** { id, proposition, cited fact ids, withdrawn }" | the struct also carries `revisions`; three grounding passes qualified on the same word |
| tet30 C12 | "without inspecting **a single character** of prose or proposition text" | the executed rule hashes `c['prop']` and compares `e['text']`; the author later rewrote it as "the one place text is consumed" |
| tet30 C3 | "two passes **918** seconds apart" | the timestamps differ by 910 |
| tet30 C7 | "a `now_unix` that is **byte-identical** in the two places" | one is `pub fn` on one line, one a private `fn` over four; revised to "token-for-token" |
| tet47 C18 | "with **no test** or fixture depending on its wording" | a test asserts `SELF-GROUNDED`; raised in 1 of 3 draws |
| tet47 C6 | "**Every** evidence record already stores a pin" | `pin: Option<String>`, unset on the ingested path. *Not* the defect the refutation named |
| tet56 C1 *(minor)* | "with a **clean** working tree" | status lists the memo untracked; tet61's pin sentence was later revised to name its untracked outputs |
| tet56 C3 | "the `Search` label is **the only part** … that reaches a committed memo" | per-file `GrepMatch` labels are joined into the Extent column too |
| tet56 C9 | "the returned bytes and the captured `output` are **the same value**" | `printed` prepends a header — the premise the refutation took apart, and `fact` tet56 F9 on the same code |
| tet61 C15 | "the other **31** occurrences" | the capture enumerates 32 |

Three of the 10 are refuted claims (tet47 C6, tet56 C9, tet61 C15); on tet47 C6
the check leg found a real defect but not the one the refutation turned on.

## WRONG (13)

| cluster | claims |
|---|---|
| insufficiency — "the capture covers only X" | tet30 C1, tet46 C2, tet47 C1, tet47 C7, tet47 C19, tet61 C1, tet61 C12 |
| proposal read as current | tet56 C8, tet56 C11, tet56 C19 |
| misread referent or scope | tet30 C5, tet47 C12 |
| reason misreads the evidence | tet47 C4 |

Four of the seven insufficiency findings are on a memo's pin statement
(*"every observation in this memo was taken against … commit …"*), which no
capture can ever cover by construction. tet47 C4 is the near miss: its clause
*is* inexact, and grounders qualified it, but in the other direction from the
reason the finding gives.

## The kind signature, a third time

Every CORRECT claim carries a `contradicts` finding; every claim flagged by
`overreaches` alone is WRONG. `fact` and `prose` showed the same signature and
`fact` bounds `overreaches` out (`verify.rs`, the kind filter); `claim` does not.
Re-voting each draw with `overreaches` removed:

| | flagged | correct | wrong | precision | correct lost |
|---|---|---|---|---|---|
| shipped, both kinds | 23 | 10 | 13 | 43% | — |
| `contradicts` only | 14 | 8 | 6 | 57% | tet47 C18, tet56 C3 |

Not free: those two raised their correct `contradicts` finding in only one
draw, and their majority came from `overreaches` draws. A trade of 2 of 10
catches for 14 points of precision, recorded here rather than recommended.
