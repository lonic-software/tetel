# Tetel

**Prevention at authoring time, not review afterwards.** Tetel is a tool for authoring design work in
which every factual claim carries the executable evidence that holds it up — and in which the
evidence cannot be typed, only captured.

## The problem it exists for

An AI can produce a long, fluent, confident design document faster than anyone can check one. The
failure mode that matters is not invention. It is **inconsistency**: a claim resting on nothing, a
section contradicting one four pages earlier, a fix in §7 quietly invalidating the premise §3 was
built on, a number nobody re-ran, evidence described rather than gathered.

Those defects are cheap to produce and expensive to find, and review scales badly against them —
which is precisely the wrong way round when generation is the cheap part.

**The long-term goal is making AI-authored work consistent with itself.** Tetel's bet is that the
consistency has to be *structural* rather than *reviewed*: if a claim cannot be written down without
the observation it rests on, then a document that contradicts itself has to do so in the open, where
a machine can point at it.

## The one property everything else follows from

A linter reads a finished document and reports what is wrong with it. Tetel aims earlier — but the
promise has to be stated at the right strength. Three layers, and only two of them are enforceable:

- **Format-level prevention is real.** A fact's *extent* — what was actually opened or executed — is
  captured by the tool. There is no flag anywhere that supplies one, and that absence is the whole
  guarantee. A required field cannot be omitted; the document does not parse.
- **Values are captured; the sentence about them is still written.** A fact's output is recorded by
  the tool and cannot be typed. The prose a human writes *about* that output can still say anything,
  and nothing today checks that a number in a sentence appears in the evidence beneath it. That gap
  is real, it is what supporting-span selection is meant to close, and until then it is human-owed.
  **`check` does not re-run anything** — it never executes a command a document names, deliberately,
  since a checked-in memo would otherwise be a way to run code on whoever checks it. Drift between a
  captured value and today's reality is closed by observing again, not by the checker.
- **Wrong-but-green evidence is not addressed, and never will be.** A command can run cleanly and
  fail to establish what its author believed. That residue is human-owed permanently, and the job is
  to keep it visible rather than to shrink it.

The mental model is **a lab notebook whose every recorded result gets re-measured, and which says so
loudly when the measurement stops agreeing.**

The reason this beats better review: a refutation that arrives *while* a design is being authored
changes the design. The same refutation arriving afterwards produces a finding that gets patched
around. Prevention converts the second into the first.

## What exists today

Run `tetel --help` for the authoritative surface. The shape of a session:

```sh
tetel look src/parser.rs --lines 40:80     # observe — into a pending buffer
tetel run cargo test --lib                 # observe — output captured verbatim
tetel fact --note "the retry path is unbounded"
                                           # mint F1: extent, output and pin are
                                           #   captured here and never revisable
tetel claim --proposition "retries can loop forever" --cites F1
tetel prose --text "The failure is unbounded retry. See [C1]."
tetel render --out design.md               # the document, plus its snapshot
tetel check design.md                      # two partitions, never one verdict
```

Authoring is `look` / `run` → `fact` → `claim` → `prose` → `render`. A fact's note is revisable with a
required reason; its **extent, output and pin are not, ever**. Claims rest on facts, prose cites
claims, and dependency is derived from those citations rather than stored a second time.

Verification is `check`, `brief` and `record`. `brief` emits every claim with its scope **withheld**,
so an independent pass grades the proposition without seeing what the author declared it ranged over.
`record` appends that pass's verdict to the ledger.

Everything is also exposed over **MCP** (`tetel mcp`), which is how agents author with it — arguments
arrive as JSON with no shell in the path, so text that a shell would corrupt survives byte-exact.

### Where things live

| artifact | what it is |
|---|---|
| `design.md` | the rendered document — prose plus evidence rows, plain markdown so agents read it, tooling greps it, and review happens in a diff |
| `design.md.evidence.jsonl` | the grounding ledger: append-only [in-toto](https://in-toto.io) statements, one per claim per pass, each carrying a digest of the exact proposition text it graded |
| `design.md.tetel/` | the snapshot — the workspace state that produced the document, shipped beside it so a citation resolves in a repository that never had the workspace |
| `~/.local/state/tetel/workspaces/<name>/` | live authoring state: facts, claims, prose, refusals, identity |

Markdown is what `render` emits, not what tetel is about. **The render target is the most replaceable
piece here**; the evidence and the claims resting on it are not.

## Vocabulary

Partly borrowed from the proof house, where a gun barrel is stamped only after surviving an actual
overpressure firing — **the mark cannot exist without the test having been fired.** That is this
tool's central guarantee stated in four words: a fact's extent is captured, and there is no flag
anywhere that supplies one.

| term | meaning here |
|---|---|
| **observation** | one `look` or `run`, captured into the pending buffer |
| **fact** | observations folded into an immutable record: extent, output, pin. The note is authored; everything else is captured |
| **extent** | what a fact actually opened or executed. Machine-captured — there is no way to type one |
| **pin** | a content fingerprint over a fact's extent, output and the working tree it was taken against |
| **claim** | a proposition resting on one or more facts |
| **grounding** | an independent pass grading claims from source alone, with the author's scope withheld |
| **out of proof** | a claim whose every record grades a wording it no longer carries. The stamp no longer certifies this barrel — a machine failure |
| **reprove** | ground a claim again against what it now says. The only thing that clears *out of proof*, and it adds a record rather than editing one |
| **superseded** | the marks from before a claim was reproved. History, human-owed, never a failure |
| **witnessed / ingested** | whether the tool captured the act itself, or only captured someone *reporting* the act |

Those three describe one state machine: revise a claim and it falls **out of proof**; **reprove** it
and the earlier records become **superseded**. The ledger is append-only, so nothing is ever cleared
by editing — only by adding a later proof.

## What `check` tells you

Two labelled partitions, each stating its own scope, and **never a single document-level verdict**:

- **machine-checked** — grammar, scope subset on enumerated rows, abutting literals, unsettled
  citations, dependency cascades, evidence-ledger import, verdict disagreement, claims out of proof,
  and provenance drift between a document and its own snapshot. These fail the run.
- **human-owed** — ungrounded claims, qualified verdicts in the grounder's own words, whether a claim
  was graded by the workspace that authored it or an independent one, notes reaching past their
  fact's extent, refusals recorded in a fact's mint window, and tetel's own standing non-coverage.
  **None of it is settled by a passing check**, and none of it fails the run.

Exit 2 means no tetel rows were found at all — out of scope, nothing checked, which is *not* a clean
run.

## Constraints — the things this must not become

- **No auto-bless.** A stored value that silently updates to match a fresh run turns *"the claim still
  holds"* into *"the command exited zero"*. Those are different propositions, and the second certifies
  drift with a green checkmark. A mismatch is always a loud failure a human resolves.
- **No green wall.** Mechanical green on the checkable claims leaks confidence onto a document whose
  worst defects are the uncheckable ones. Output partitions, and prints the human-owed list item by
  item. **The job is to concentrate human reading on the residue, not to shrink it.**
- **Never paraphrase.** If a tool returns a clause, it returns the clause's prose. The moment the
  server summarises, every downstream reader inherits the tool's reading of the document instead of
  the document — silently.
- **Prose is never generated from fields.** Facts may be captured and interpolated; arguments may not
  be assembled out of structure. A format that constrains what a design can express constrains what
  can be argued about it, and the strongest refutations on record came from readers engaging prose
  with full generality. Whoever writes it — person or agent — writes the argument themselves.
- **Refusals are format-level, never heuristic.** "You did not point at your evidence" is a decidable
  question. "This looks wrong" is not, and belongs in the human-owed partition or nowhere.

## Direction

Roughly in order, and each gated on measurement rather than enthusiasm — several proposed refusals
have already been built, measured against a real corpus, and **rejected on precision**:

- **Make the guards authoritative.** A commit hook runs the pair-guards locally today; CI would make
  them hold for a contributor who is not using the same setup.
- **More authoring-time refusals, where they are decidable.** Requiring a workspace-rooted search
  behind a claim that names a symbol; requiring a transplanted mechanism to carry its donor's stated
  premises; reporting prose revised since its claims were last grounded.
- **Supporting-span selection.** At mint time, point at the span of captured output that supports the
  assertion — refused unless it is a verbatim substring of a cited fact. The dual of the extent
  guarantee: the author cannot type an extent, and here could not invent support, only select it.
- **An obligation ledger.** Today a warning is a printed string with no lifecycle, so a warning that
  was ignored and one that was correctly resolved are indistinguishable in every artifact. Warnings
  raised, discharged with a *type*, and what is still owed reported by `check`.
- **Then, and only then, an optional model advisory pass** — a smaller model warning that a claim
  looks unsupported by its evidence, so the author verifies during authoring rather than discovering
  it after half the design was built on it. Deliberately last: it is worth building only once the
  deterministic layers have narrowed the question from *"does this transcript support this note?"* to
  *"does this span support this sentence?"*, and it must never become a gate.

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

## Building and installing

Requires a [Rust toolchain](https://rustup.rs). From a clone of this repository:

```sh
cargo install --path .
```

That puts a `tetel` binary in `~/.cargo/bin`, which needs to be on your `PATH`.

### Using it from Claude Code

`tetel mcp` runs an MCP server over stdio, exposing the same authoring and checking commands as
tools. With the binary installed and on your `PATH`, register it:

```sh
claude mcp add --scope user tetel -- tetel mcp
```

That reads as: server name `tetel`, running the command `tetel mcp`. Everything after `--` is the
server command rather than an option to `claude` — not strictly required here, since `mcp` is a bare
argument, but it is the form that keeps working once a command takes flags of its own.

`--scope user` makes it available in every project; drop it to register only in the current one.
Confirm with `claude mcp get tetel`, which should report `Status: ✔ Connected`.

> **Rebuilding requires restarting Claude Code.** `cargo install` replaces the binary by rename, so
> a server process already running keeps the file it opened — reinstalling does not reach it, and
> neither does reloading plugins. Since a stale server would otherwise answer with an old build
> rather than erroring, one that finds its binary replaced underneath it now **refuses every tool
> call** and says so. `check` also names the build that graded it on its last line, so two runs that
> disagree can be told apart.

## Status

**Built, and in use on its own development** — including on the work that produced the features
listed above, which is where most of its defects have been found. Not released, not versioned beyond
`0.1.0`, and the surface still moves.

The kill condition registered before the first line of code still stands and **has not yet been
fairly evaluated**: *if the next two real documents yield only findings an existing lint would have
surfaced anyway, it did not earn its keep.* Documents written under it so far have been about tetel
itself, which is the weakest possible evidence base; the honest test is a design that has nothing to
do with this tool.

## Decisions

- **Licence — MIT**, matching [pult](https://github.com/lonic-software/pult). Tetel is a free tool.
- **Implementation language — Rust**, matching pult. The checker prototype that motivated this is
  Python; it stays a prototype.

## Licence

MIT. See [`LICENSE`](LICENSE).
