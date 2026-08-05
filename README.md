# Tetel

**Nothing is built yet.** This repository holds the intent, so the first line of code is written
against a decision rather than a memory.

Tetel is a tool for authoring design documents in which **every factual claim carries executable
evidence**.

## The one property everything else follows from

**Prevention at authoring time, not detection afterwards.**

A linter reads a finished document and reports what is wrong with it. Tetel aims earlier — but the
promise has to be stated at the right strength, because the file is plain markdown and nothing stops
anyone typing anything into it. Three layers, and only two of them are enforceable:

- **Format-level prevention is real.** A claim's scope is a set of things you name, not a sentence, so
  a scope ranging wider than what you opened is a set comparison rather than a judgement call. A
  required field cannot be omitted — the document does not parse.
- **Numbers are held by re-execution, not by provenance.** You *can* type a value instead of running
  the command. It buys nothing: the checker re-runs the command, and a disagreement is a loud failure
  a human resolves. Whether the author or the checker executed it was never the property that
  mattered.
- **Wrong-but-green evidence is not addressed, and never will be.** A command can run cleanly and
  fail to establish what its author believed. That residue is human-owed permanently, and the job is
  to keep it visible rather than to shrink it.

The mental model is **a lab notebook whose every recorded result gets re-measured, and which says so
loudly when the measurement stops agreeing.**

The reason this matters more than better checking: a refutation that arrives *while* a design is being
authored changes the design. The same refutation arriving afterwards produces a finding that gets
patched around. Prevention converts the second into the first.

## Shape

Three pieces, each doing what it is good at:

1. **A markdown file** — prose plus fenced evidence rows. Plain markdown on purpose: agents read the
   raw file, tooling greps it, and review happens in a diff. A structured blob format would break all
   three, which is the `.ipynb` failure and it is disqualifying here.
2. **A checker** — re-runs stored commands and fails loudly on a mismatch. Constraint queries
   (does a claim's scope exceed what was opened? does anything still depend on this?) run against a
   derived index, never a checked-in database.
3. **An MCP server** — the write path mints rows from actual executions, which removes the
   transcription step but is not where the guarantee lives; the query path answers dependency
   questions cheaply. **The read path stays the file.**

One rule generates the format: **each fact has exactly one home, and everything else points at it.**
There is no metadata section. Dependency is derived from citations already present in the prose rather
than stored a second time. References run one way only.

## Vocabulary

Borrowed from the proof house, where a gun barrel is stamped only after surviving an actual
overpressure firing — the mark cannot exist without the test having been fired.

| term | meaning here |
|---|---|
| **proof** | the execution behind a claim |
| **mark** | the evidence row it mints |
| **view mark** | the you-actually-opened-it check |
| **out of proof** | a stored value no longer matching a re-run |
| **reprove** | re-running to restore a claim's standing |

## Constraints — the things this must not become

- **No auto-bless.** A stored value that silently updates to match a fresh run turns
  *"the claim still holds"* into *"the command exited zero"*. Those are different propositions, and
  the second one certifies drift with a green checkmark. A mismatch is always a loud failure a human
  resolves.
- **No green wall.** Mechanical green on the checkable claims leaks confidence onto a document whose
  worst defects are the uncheckable ones. Output must partition into machine-verified and human-owed,
  and print the human-owed list item by item. **The job is to concentrate human reading on the
  residue, not to shrink it.**
- **Never paraphrase.** If a tool returns a clause, it returns the clause's prose. The moment the
  server summarises, every downstream reader inherits the tool's reading of the document instead of
  the document — silently.
- **Prose is never generated from fields.** Facts may be captured and interpolated; arguments may not
  be assembled out of structure. A format that constrains what a design can express constrains what
  can be argued about it, and the strongest refutations on record came from readers engaging prose
  with full generality. Whoever writes it — person or agent — writes the argument themselves; the
  typing was never the point.

## What it will not solve, stated up front

- **A word used in two incompatible senses.** A glossary forces the author to pick one while writing,
  but nothing checks that the pick was right.
- **Design judgment.** Whether an approach is correct is not a document-format problem.
- **Wrong evidence, as opposed to absent evidence.** A command can run cleanly and fail to establish
  what its author believes it established. **Tetel makes the empty claim impossible and leaves the
  mistaken one intact**, and that limit belongs wherever its output is read.

## The name

Hungarian *tétel*: a **line item in a ledger**, and a **proposition** in logic. The evidence rows are
ledger entries that are propositions, so the word already names the central object rather than
describing it by analogy. Short, unambiguous to pronounce, and free of prior meaning in English.

## Decisions

- **Licence — MIT**, matching [pult](https://github.com/lonic-software/pult). Tetel is a free tool.
- **Implementation language — Rust**, matching pult. The checker prototype that motivated this is
  Python; it stays a prototype.

## Status

Design intent only — no code, no format specification, nothing piloted. **The first deliverable should
be small enough to throw away**, and the kill condition should be registered before it is written: if
the next two real documents yield only findings an existing lint would have surfaced anyway, it did
not earn its keep.

## Licence

MIT. See [`LICENSE`](LICENSE).
