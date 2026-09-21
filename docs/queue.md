# Queue

Every open TET ticket, ordered. Written 2026-09-20 against the 62 issues then open in the TET
project; statuses there are authoritative, this file is the ordering they do not carry.

The split is by a single question: **does this stand between tetel and being pleasant to work in?**
Group A says yes. Everything after it is worth building and does not block the tool being used.

The ordering rule inside each group is *cost to the caller per session*, not severity in the
abstract. Tetel's callers are agents with a fixed context budget, so a defect that spends 100k
characters of that budget every session outranks a rare wrong answer.

The measurements quoted below come from one instrumented session (a design memo on an unrelated, private
project, September 2026): ~1.5M characters of tetel tool output across an author, an attacker and a
grounder, of which `look` was ~51% and `run` ~8%.

---

## Now — chosen out of order

These three came out of the Jev verifier design (`docs/design/tet-verifier-jev.md`, 2026-09-21), after
the ordering below was written. TET-96 was picked to go next by decision, not by the rule below; the
other two are placed where they would stand on their own.

| # | Ticket | Note |
|---|---|---|
| N1 | **TET-96** — typed legs in the mint-time verifier: Jev as gate, classify, literal judge and refuter | In progress, as three PRs. Slice 1 (plumbing and the typed refuter on `fact`: routing, the per-verb table, the `verify.model` refusal, credentials, per-provider budget, pricing, version flag) **merged** (PR #15, 2026-09-21); its review added `RefuterLeg::Itself` and a `reason` on `refuter_not_run`. Slice 2 (the gates on `fact` and `claim`, `verify.typed_model`, `Status::Gated`, `gate_incomplete`, and the tests for invariants 2, 3, 5 and 10 that slice 1 deferred) **merged** (PR #17, 2026-09-21); it also fixed slice 1's typed requests, which went out with sorted keys, and its review made a gate answer that leaves an option out a failed gate. Slice 3, Jev classify and literals on `claim`, is what remains. Nothing blocks it: TET-85 covers `verify.enabled = false`, not the enabled-path `unauthorized` arms this adds. Ships with both new defaults off. |
| N2 | **TET-98** — the two runs owed before Jev's typed legs become the default | Alongside N1, not after it; gates only the default flips. Every Jev call must keep the resolved model version, which no run so far has. |
| N3 | **TET-97** — move the verifier-eval corpora out of the worktree tetel censuses | Ranked in B, beside TET-87, for the same reason: it distorts the censuses the queue is gated on. Decide it before N2 lands tens more megabytes of runs. |

---

## A. Blocking — before tetel is usable enough and efficient

Roughly half of these are one defect wearing different hats: **a reply is sized for a terminal and
delivered to a context window.** They are listed separately because they are fixed in different
places, but A2–A7 should be settled together, behind one decision about what a bounded reply is.

| # | Ticket | Why it blocks |
|---|---|---|
| A1 | **TET-91** — Fable 5.1 refuses any agent carrying `look`, `fact` or `record` | The tool cannot be handed to a whole model tier. Nothing else on this list matters for an agent that cannot be spawned. `reasoning_extraction` fires on the tool descriptions alone, so it is likely a wording fix. |
| A2 | **TET-93** — over-cap replies silently replaced by a file stub | The worst failure mode in the tool: an agent restricted to tetel's own verbs gets a path it cannot open, and nothing says evidence was lost. Silence is what makes it first. |
| A3 | **TET-56** (f2, f3) — bound the census return | The general mechanism the rest of this cluster needs. Decide here what "bounded" means and what a reply says about what it left out; A4–A6 then apply it. |
| A4 | **TET-92** — `look --grep` and `fact` name every skipped git-ignored path | A quarter of an attacker run's output. One `fact` reply reached 130k characters that no agent could read. Pure waste — the paths were skipped. |
| A5 | **TET-95** — `run` returns its whole capture | ~125k characters in one session, mostly a test runner's passing lines. The capture must stay whole as evidence; only the reply needs bounding. |
| A6 | **TET-72** — `check` outgrows its own consumers | 22–31k characters on a 16-claim memo, 75k on a 30-claim one. `check` is called repeatedly during the loop, so its size multiplies. Not a licence to re-open TET-51's rejected trim — the residue stays printed item by item. |
| A7 | **TET-73** — failing partition reprints each prior verdict | Appears fixed in `bf8079f` but still To Do. Verify and close, or finish. Cheap either way. |
| A8 | **TET-66** — a wrong-block `prose --revise` is indistinguishable from a right one | Silent data loss during authoring, twice in one memo in the session measured. Ids never change but positions do, so counting goes wrong past the first insertion, and the reply says nothing that would reveal it. |
| A9 | **TET-43** — a change to what `render` emits drifts every committed memo | Sequenced here because A10 changes render output. Settle the drift story before, not after. |
| A10 | **TET-83** — heading prose carrying literal `## ` renders as `## ## Foo` | Visible corruption of the artifact, and `check` has no heading grammar rule to catch it. |
| A11 | **TET-89** — `look --grep` descends into `.git` | Silently inflates every repo-root census, which means it silently corrupts the measurements the rest of this queue is gated on. |
| A12 | **TET-86** — TET-56 bounded the return, not the storage | Over-limit output ships in the snapshot and is then committed forever. Permanent, and it grows. |
| A13 | **TET-80** — `run`'s capture is copied four times | 200 MB of output costs 1 GB of RSS, and the only bound is wall-clock. A long test run can take the machine down. |
| A14 | **TET-79** — `look` reaches an unbounded outcome by two routes, one with no child to kill | A hang with no recovery. |
| A15 | **TET-82** — the write half of the same class: `render --out` and the evidence append block on a FIFO | Same shape as A14, other direction. Fix alongside it. |
| A16 | **TET-81** — `tetel config` prints `(unset)` for three keys whose defaults spend money and kill processes | An operator cannot see the settings that have consequences. Small fix, disproportionate trust value. |
| A17 | **TET-84** — `verify` returns `unavailable` on 31% of mints | A third of the mint-time verifier's work is lost after a full timeout and four attempts, so it also costs the wall-clock it wasted. |
| A18 | **TET-85** — the `unauthorized` diagnostic is unreachable | Gated behind the condition it diagnoses. Trivial, and it hides A17's neighbours. |
| A19 | **TET-94** — minting a fact and grading a claim are always two round trips | The single biggest reduction in call count available, and call count is what the loop is made of. |
| A20 | **TET-3** — run a real forklift memo through the tool, instrumented | Last in the group and the reason for the group. The registered kill condition has still never been fairly evaluated: every memo so far has been about tetel itself. Group C is worth building only if this passes. |

## B. Correctness and hygiene — real defects, not in the way

| # | Ticket | Note |
|---|---|---|
| B1 | **TET-88** — absolute spellings through a symlinked ancestor are never relativized | `world_root` is canonical, the spelling is literal. Wrong answers, but only under symlinks. |
| B2 | **TET-90** — `check` should report a stale extent when a fact's subject has moved | Human-owed row, in keeping with "no auto-bless". |
| B3 | **TET-67** — TET-46's exclusion is scoped to `look`; `run` reaches tetel's own output freely | 40 of 74 proc extents do. Closes a hole rather than opening a feature. |
| B4 | **TET-87** — a memo under authoring sits inside its own measurement population | Corpus counts over `docs/design` silently include the memo being written. Distorts exactly the measurements this queue is gated on — the reason it is near the top of B. |
| B4a | **TET-97** — the verifier-eval corpora sit in the worktree every census searches | Same family as B4: 44 MB of JSON quoting memo prose makes common symbols uncensusable (three of the eight in the Jev design) and was once captured by accident. See N3. |
| B5 | **TET-6** — overlap-report keying fix | Known, scoped, small. |
| B6 | **TET-64** — warn the author at `prose --revise` time | The mitigation for A8; do it after, once there is an act to name. |
| B7 | **TET-49** — revising a claim leaves its dependents unexamined | Same family: a change whose blast radius nothing reports. |
| B8 | **TET-75** — pass kind is a substring of a workspace name, and the namespace is flat | "Has an attacker seen this text?" should be machine-checkable. Blocks tidy multi-pass work rather than any single pass. |
| B9 | **TET-18** — observations carrying who made them | Makes pass-independence derivable instead of a typed string. Pairs with B8. |
| B10 | **TET-14** — capture hygiene: `look`/`run` output now reaches git history via the snapshot | Related to A12, but the policy question rather than the size one. |
| B11 | **TET-57** — honour other tools' ignore files, starting with `.forkliftignore` | Both of these reduce census noise at the source; A4 makes them less urgent, not unnecessary. |
| B12 | **TET-58** — a `.tetelignore`, on semantics tetel controls | |
| B13 | **TET-25** — command-time debt warnings, if the caps prove leaky | Explicitly conditional on A3–A5 measuring a leak. |

## C. Capability — the loop the README's Direction describes

Ordered by what the loop is missing most, which is not the same as what is most interesting.

| # | Ticket | Note |
|---|---|---|
| C1 | **TET-34** — supporting-span selection, in general | The named next step in the README, and the one that closes the gap between "values are captured" and "the sentence about them is still written". The narrow case (transplant premises) already ships and should be measured first. |
| C2 | **TET-36** — obligation ledger | Today a warning is a printed string with no lifecycle, so ignored and resolved look identical in every artifact. |
| C3 | **TET-12** — `BRIEF.md` required in the harness directory, enforced by refusal | Cheap, and it makes C4/C5 well-posed. |
| C4 | **TET-71** — the designer has no channel to ask the human a question | An unanswerable decision currently becomes a silent omission. |
| C5 | **TET-69** — a brief's question list silently becomes the design's scope | The other half of C4: nothing records what was never asked. |
| C6 | **TET-74** — an author cannot mint attested evidence | A design resting on a human premise has no way to mark it. |
| C7 | **TET-7** — `external:` designator and `attested` evidence kind | Same mechanism as C6 from the other end; scope them together or merge them. |
| C8 | **TET-70** — a design about something that does not exist yet cannot be grounded | The loop currently answers only the half with readable source, which is the wrong half for a design. |
| C9 | **TET-13** — `tetel reopen`, restoring a committed snapshot into a workspace | Makes a shipped memo editable again. |
| C10 | **TET-44** — re-measure a fact's command in CI | Reader learns the memo says 14 and it is now 12. Note it must not become auto-bless. |
| C11 | **TET-78** — a verifier that captures its own evidence | The recall ceiling is structural, not a tuning problem. Large. |
| C12 | **TET-8** — optional `builds-on` edge, claim to claim | |
| C13 | **TET-65** — the world-state marker as a shared node, with validity edges | |
| C14 | **TET-40** — ground the crate's own load-bearing doc comments | Dogfooding with a real payoff: they are unchecked factual claims today. |
| C15 | **TET-37** — compare a grounding pass's working tree against the memo's | Measurement first, as filed. |
| C16 | **TET-22** — the recurring defect shape: paired artifacts drifting with nothing asserting they agree | |
| C17 | **TET-20** — project agent: surface-keeper, for the judgement half of CLI/MCP parity | |
| C18 | **TET-4** — `tcontract` / `tdecide` / `tobligation` | Overlaps C2; re-read it once the obligation ledger exists, it may shrink. |
| C19 | **TET-38** — retroactive insertion, writing an already-built design into tetel as an audit technique | A use case, not a feature. Worth trying only after A20. |

## D. Explorations — no committed shape yet

These are questions, and each should produce a decision (possibly "no") rather than code.

| # | Ticket |
|---|---|
| D1 | **TET-48** — the remaining design-loop optimizations; items 2 and 5 are closed, four remain |
| D2 | **TET-62** — one graph per project rather than per document |
| D3 | **TET-27** — design and implementation co-evolving, with code changes linked to the graph |
| D4 | **TET-26** — goals as a primitive: record what should be, not only what is |
| D5 | **TET-77** — tense on claims, as-is vs to-be |
| D6 | **TET-45** — a *why* comment asserting a mechanism is an ungraded claim with no channel |
| D7 | **TET-33** — an optional model-backed critic gating ingestion | The README places this deliberately last, after the deterministic layers have narrowed the question. Keep it there. |

## E. Housekeeping

| # | Ticket | Action |
|---|---|---|
| E1 | **TET-39** — whether a captured value is ever re-executed | Its own summary says rejected on safety and never to be re-weighed. Close it; the decision belongs in the README, where it already is. |
| E2 | **TET-50** — batching record creation | Superseded by TET-60, which is done. Close. |
| E3 | **TET-1** — epic: authoring and evidence capture, backlog from the S9–S12 runs | Re-scope or close: the backlog it names has been overtaken by the tickets above. |
