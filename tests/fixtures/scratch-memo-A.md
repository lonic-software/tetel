# Design memo: fix the abutting-literal check's value comparison for multi-token values

## 1. What the check does

Check 3 exists to catch prose that quietly drifts from the evidence a row
recorded — the canonical example is a document that says "answers at 29s"
next to a row whose stored `value` is `31s` [E-2]. To decide whether a
citation has a literal to check at all, `abutting_context` looks at only
the single whitespace-delimited token immediately before the bracket, at
zero or one space of gap [E-2], and a token counts as literal if it
contains a digit or is a quoted string [E-1]. Once a token is classified
Abutting, the check normalizes it and requires it to equal the row's
*entire* `value` field, verbatim [E-3]. A mismatch is not informational —
it lands in the machine-checked partition and flips the process exit code
to failure [E-4].

## 2. The gap

That comparison is single-token-against-whole-field. It is exact for a
row whose value happens to be one token, like `31s`. It cannot ever
succeed for a row whose value is naturally written as more than one
token — and the format's own documentation says values include "counts"
alongside durations and exit codes [E-5], which is exactly the shape of
`5 retries`, `code 137`, `exit 0`. Prose that cites such a row correctly,
with the abutting token being precisely the number the row documents,
still reddens: citing row X-3 (`value: code 137`) as "...exits with code
137 [X-3]..." produces a hard abutting-literal failure and exit code 1
[E-6], because the abutting token `137` is compared against the full
string `code 137` rather than against the word of the value it actually
corresponds to.

## 3. Why this isn't a corner case

This isn't a manufactured edge case; it's the intersection of two things
already in the crate. The value shape is spec'd by the crate's own
comment [E-5], and the crate's own kitchen-sink fixture already has a
row with a two-token value, `T-1`'s `value: exit 0` [E-8]. That fixture's
prose cites `T-1` without ever placing the citation in abutting position
— no digit-bearing token sits immediately before `[T-1]` — so the
existing suite doesn't exercise a multi-token value through this
comparison at all [E-8]. Both shipped abutting fixtures use only the
single-token value `31s` [E-7]. The gap has simply never been hit by a
test, not never been reachable by real input.

The practical cost is architectural, not cosmetic: any document that
states a count- or code-shaped value in the natural way an author would
write it, and then cites it at the distance the tool itself calls
"abutting," is unfixable by rephrasing the row — only by rephrasing the
prose to avoid abutting distance, which defeats the point of the check
existing.

## 4. Proposed fix

In the check-3 comparison (`checks.rs`, inside `analyze`, the branch that
computes `norm != *value.trim()` [E-3]), compare the normalized abutting
token against the *last whitespace-delimited word of `value`* rather
than the whole trimmed field. For a single-token value this is a no-op —
the tail of a one-word string is the string — so every currently-passing
case (`31s`, `matched`, `31s` in `demo.md`) is unaffected. For a
multi-token value, the citation now correctly matches when the abutting
digit is the value's own trailing number (`code 137` → tail `137`,
`exit 0` → tail `0`, `5 retries` → tail... `retries`, which is a
separate, narrower question of whether counts should be written value-
first; not this memo's problem to solve). This is a one-line change at a
single call site, touches no other check, and needs one new fixture
pairing a multi-token `value` with a citation placed at true abutting
distance to lock in the behavior.

## 5. Scope

This leaves `is_literal_token`'s digit-based classification [E-1] and
the Abutting/Candidate distance rule [E-2] untouched — those decide
*whether* a token is treated as a value citation at all, which is a
separate, harder precision question (a stray section number like
"section 3.2 [X-1]" also reads as literal today, and tightening that
would be a different, riskier change to the classifier rather than the
comparison). The fix proposed here only repairs the comparison's target
once a token has already been classified as the citation's literal,
which is the narrowest change that makes a documented, intended value
shape actually checkable.

## Evidence ledger

```tetel
id:     E-1
claim:  is_literal_token classifies a whitespace-delimited token as literal if it is a quoted string or contains any ASCII digit, independent of any row's value.
domain: src/citations.rs#is_literal_token
extent: src/citations.rs#is_literal_token
pin:    working-tree
kind:   READING
status: VERIFIED
```

```tetel
id:     E-2
claim:  abutting_context classifies a citation as Abutting only from the single trailing whitespace-delimited token on its line, at zero or one space of gap, and Candidate for a literal-looking token found further back.
domain: src/citations.rs#abutting_context
extent: src/citations.rs#abutting_context
pin:    working-tree
kind:   READING
status: VERIFIED
```

```tetel
id:     E-3
claim:  For an Abutting citation, analyze() reports a check-3 failure when the normalized abutting token differs from the row's entire trimmed value field, comparing one token against the whole field rather than against any single word of it.
domain: src/checks.rs#analyze
extent: src/checks.rs#analyze
pin:    working-tree
kind:   READING
status: VERIFIED
note:   the comparison is the `if norm != *value.trim()` branch, around line 140.
```

```tetel
id:     E-4
claim:  A check-3 (abutting-literal) failure is counted into Findings::machine_check_failed and, via report::render, sets the process exit code to EXIT_CHECK_FAILED (1).
domain: src/checks.rs#Findings::machine_check_failed
extent: src/checks.rs#Findings::machine_check_failed, src/report.rs
pin:    working-tree
kind:   READING
status: VERIFIED
```

```tetel
id:     E-5
claim:  The crate's own doc comment states that the value field's intended shapes are "durations, counts, exit codes, verbatim strings" — a set that is not restricted to single-token literals.
domain: src/citations.rs
extent: src/citations.rs
pin:    working-tree
kind:   READING
status: VERIFIED
note:   lines 67-69, the doc comment immediately above is_literal_token.
```

```tetel
id:     E-6
claim:  Checking a document that cites row X-3 (value: "code 137") as "...exits with code 137 [X-3]..." at zero-gap abutting distance produces a check-3 failure and process exit code 1, even though the abutting token is exactly the number the row documents.
domain: proc: cargo run --quiet -- check <scratch fixture citing "code 137 [X-3]", row value: code 137>
extent: proc: cargo run --quiet -- check <scratch fixture citing "code 137 [X-3]", row value: code 137>
pin:    working-tree
kind:   RUN
run:    cargo run --quiet -- check <fixture>
value:  exit 1, with output line: "line 1: literal '137' abuts [X-3] but row X-3's value is 'code 137'"
status: VERIFIED
note:   fixture built and run for this memo, then deleted; not committed to the repo.
```

```tetel
id:     E-7
claim:  Both shipped abutting-literal fixtures (abutting_fail.md, abutting_pass.md) use only the single-token value "31s", so the shipped test suite never exercises a multi-token value through check 3.
domain: tests/fixtures/abutting_fail.md, tests/fixtures/abutting_pass.md
extent: tests/fixtures/abutting_fail.md, tests/fixtures/abutting_pass.md
pin:    working-tree
kind:   READING
status: VERIFIED
```

```tetel
id:     E-8
claim:  demo.md's row T-1 has a two-token value ("exit 0"), and the sentence citing T-1 ("The build passes on a clean checkout [T-1].") does not place the citation at abutting distance from any digit-bearing token, so this multi-token value is never run through the check-3 comparison by the existing suite.
domain: tests/fixtures/demo.md
extent: tests/fixtures/demo.md
pin:    working-tree
kind:   READING
status: VERIFIED
```
