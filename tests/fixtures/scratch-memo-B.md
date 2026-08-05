# Design memo: stop treating bare reference numbers as asserted values

## 1. The proposal

Narrow check 3 (the abutting-literal check) so that a *bare, unadorned integer* — no
unit-suffix letter, no decimal point, not quoted — immediately preceding a citation is
downgraded from the failing `Abutting` classification to the informational-only
`Candidate` classification. Everything else `is_literal_token` currently treats as
literal (durations like `29s`, decimals, quoted strings) keeps failing exactly as
today; only the narrowest, most collision-prone shape — a lone digit run — loses its
power to redden a run.

## 2. The mechanism as it stands

Check 3 exists to catch a specific kind of drift: a number gets typed in prose, the row
it cites gets re-measured, and nobody goes back to update the sentence. The check
implements this by looking at the single token immediately before a citation's `[` and,
if that token "looks like a literal," comparing it against the row's `value` field
[E-1] [E-2]. "Looks like a literal" is defined as: quoted, or contains any ASCII digit
[E-3]. That predicate is used both to decide the immediate-abutting case (fails on
mismatch) and to search backward for a same-line "candidate" (printed, never fails)
[E-2].

The trouble is that "contains a digit" is also the shape of an ordinary cross-reference
— "Table 3", "Section 2", "Appendix 2", "the 2nd revision" — and English puts exactly
these right before a citation bracket, because that is the natural way to point at
where something is written down. I confirmed this concretely: a document with the
sentence "The gateway's SLA is documented in appendix 2 [A-1]." against a row whose
`value` is `31s` fails the run with the message `literal '2' abuts [A-1] but row A-1's
value is '31s'` [E-4] [E-5] — a citation of *where a fact lives*, misread as a
disagreement about *what the fact is*.

This isn't a narrow corner case; it's the default shape of a citation that follows a
section reference, which is one of the two or three most common rhetorical positions a
citation occupies in a design document. Every time it fires, the author has two bad
options: rewrite ordinary prose to avoid a bare number landing next to a bracket, or
start reflexively distrusting `abutting-literal` failures — which is the one outcome
the tool's own design explicitly calls out as unacceptable. The README frames a
mismatch as "always a loud failure a human resolves" [E-7]; that framing only holds if
the failures a human sees are ones actually worth resolving. A check that cries wolf on
"Appendix 2" trains exactly the inattention the whole project exists to prevent.

## 3. Why this fix, not a different one

The code already has the right shape for this fix: `Abutting` (fails) and `Candidate`
(printed as informational, never fails) are already two different confidence tiers for
the same underlying "found a literal-looking token" signal [E-2]. I'm not proposing a
new mechanism — I'm proposing that a bare integer, on its own, doesn't clear the bar for
the high-confidence tier. It still shows up as a candidate, which is exactly the "worth
a human's glance, never grounds for failure" treatment the check already gives to
same-line-but-not-adjacent tokens.

The obvious objection is that some row values genuinely are bare integers — a retry
count, a queue depth — and prose like "the retry count is 3 [ID]" would lose its
failing check under this change. That's a real cost, not a free lunch. But it's the
smaller of the two error rates on offer: nothing in the current fixtures or CLI tests
exercises a bare-integer abutting match or mismatch [E-6], while the section/table/
figure/appendix reference pattern is exactly the kind of sentence a design document
writes constantly. Recall that the tool's own stated ceiling is best-effort, not
soundness — durations, decimals, and quoted values, the shapes the existing fixtures
actually test [E-2], keep failing exactly as before. This trades an untested, narrow
positive for a demonstrated, common false positive, which is the right trade for a
prevention-at-authoring-time tool whose entire value proposition depends on authors
trusting what turns red.

## Evidence ledger

```tetel
id:     E-1
claim:  check 3 (the abutting-literal check) only compares a citation's row value against the single token immediately preceding the citation, gated on row.value being present
domain: src/checks.rs#analyze
extent: src/checks.rs#analyze
pin:    working-tree
kind:   READING
status: VERIFIED
```

```tetel
id:     E-2
claim:  abutting_context and is_literal_token classify any digit-bearing or quoted token as a literal, and the same predicate feeds both the failing Abutting arm (gap <= 1) and the non-failing Candidate arm (same line, looser gap)
domain: src/citations.rs#abutting_context
extent: src/citations.rs#abutting_context, src/citations.rs#is_literal_token
pin:    working-tree
kind:   READING
status: VERIFIED
```

```tetel
id:     E-3
claim:  is_literal_token returns true for any stripped token containing at least one ASCII digit, with no requirement of a unit suffix, decimal point, or other qualifying shape
domain: src/citations.rs#is_literal_token
extent: src/citations.rs#is_literal_token
pin:    working-tree
kind:   READING
status: VERIFIED
```

```tetel
id:     E-4
claim:  check exits 1 on an ordinary numeric cross-reference (appendix 2) abutting a citation on a value-bearing row
domain: proc: cargo run -q -- check false_positive2.md
extent: proc: cargo run -q -- check false_positive2.md
pin:    working-tree
kind:   RUN
run:    cargo run -q -- check /private/tmp/claude-501/-Volumes-SSD-Documents-lonic-tetel/9211edfe-d423-426f-893a-8bf7e4a017bf/scratchpad/false_positive2.md >/dev/null 2>&1; echo $?
value:  "1"
status: VERIFIED
```

```tetel
id:     E-5
claim:  the abutting-literal failure line names the coincidental digit 2 as a mismatch against the row value 31s, even though the sentence was pointing at a location (appendix 2), not asserting a measurement
domain: proc: cargo run -q -- check false_positive2.md
extent: proc: cargo run -q -- check false_positive2.md
pin:    working-tree
kind:   RUN
run:    cargo run -q -- check /private/tmp/claude-501/-Volumes-SSD-Documents-lonic-tetel/9211edfe-d423-426f-893a-8bf7e4a017bf/scratchpad/false_positive2.md 2>&1 | grep abutting-literal
value:  "  - [abutting-literal] line 3: literal '2' abuts [A-1] but row A-1's value is '31s'"
status: VERIFIED
```

```tetel
id:     E-6
claim:  no fixture or CLI test in the repository exercises a bare-integer literal (digit token with no letter/unit suffix, decimal point, or quotes) abutting a citation, matching or mismatching
domain: tests/fixtures
extent: proc: grep -rEn '\[[A-Za-z][A-Za-z0-9_-]*\]' tests/fixtures | grep -E ' [0-9]+ \[| [0-9]+\['
pin:    working-tree
kind:   RUN
run:    grep -rEn '\[[A-Za-z][A-Za-z0-9_-]*\]' tests/fixtures | grep -B0 -E ' [0-9]+ \[| [0-9]+\['
value:  ""
status: VERIFIED
```

```tetel
id:     E-7
claim:  the README states that a stored-value mismatch is meant to be "always a loud failure a human resolves," framing the abutting/subset checks' failures as trustworthy signal rather than noise
domain: README.md
extent: README.md
pin:    working-tree
kind:   READING
status: VERIFIED
```
