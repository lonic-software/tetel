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
| N1 | **TET-96** — typed legs in the mint-time verifier: Jev as gate, classify, literal judge and refuter | **Done**: all three PRs merged. Slice 1 (plumbing and the typed refuter on `fact`: routing, the per-verb table, the `verify.model` refusal, credentials, per-provider budget, pricing, version flag) **merged** (PR #15, 2026-09-21); its review added `RefuterLeg::Itself` and a `reason` on `refuter_not_run`. Slice 2 (the gates on `fact` and `claim`, `verify.typed_model`, `Status::Gated`, `gate_incomplete`, and the tests for invariants 2, 3, 5 and 10 that slice 1 deferred) **merged** (PR #17, 2026-09-21); it also fixed slice 1's typed requests, which went out with sorted keys, and its review made a gate answer that leaves an option out a failed gate. Slice 3 (Jev classify under `split` and the Jev literal leg on `claim`, the `typed_classify_calls`/`typed_literal_calls` accounting, and invariant 8, that no row running the literal leg allows a typed refuter) **merged** (PR #19, 2026-09-21); its review split the report's literal rates so Jev's code-proposed candidates stay out of the LLM leg's. Both new defaults ship off; turning them on is N2's. |
| N2 | **TET-98** — the two runs owed before Jev's typed legs become the default | **Done** (PR #22 runs, PR #23 flip; closed 2026-09-23). Runs done 2026-09-23, through tetel's own `verify::spawn` over the fitted memos and three held out (`scripts/verifier-eval/README.md`, "Result, 2026-09-23"; $27.04; every Jev call answered by `jev-1.13.0`, recorded per draw). They support flipping `verify.typed_model`: on `fact` 37% cheaper, and held out the gate skips only minor defects. On `claim` it flags 8 of 91 sound claims, the same as today, once the literal findings are set aside. They do not support flipping `verify.literals` as it stands: Jev's literal leg puts `claim` at 11 of 91, one claim over the 11.3% line, through the counted-word limitation. `verify.typed_model` is flipped: `typesafe/jev-1.13.0` by default when `TYPESAFE_API_KEY` is set, passed over with a `typed_model_not_run` notice when it is not. `verify.literals` stays off. Separately, the default `verify.timeout_ms` would cut off 11–30% of the draws that ran the check, and those draws carry findings more often. The raw draws are in the tetel-eval-data repository, `verifier-eval/tet98/` (N3). On 2026-09-23 a one-draw screen moved the check model to `openai/gpt-6-luna` (58–69% cheaper, no clear loss; README "Screen, 2026-09-23"). Every figure above is 5.6's. |
| N3 | **TET-97** — move the verifier-eval corpora out of the worktree tetel censuses | **Done** (2026-09-23). Decided: a public data repository, [lonic-software/tetel-eval-data](https://github.com/lonic-software/tetel-eval-data), checked out beside tetel; no history rewrite. All 147 JSON files (43 MB) moved byte-identical to `verifier-eval/`, with TET-98's draws beside them. The scripts find it through `scripts/verifier-eval/data.py` (`$TETEL_EVAL_DATA`, default `../tetel-eval-data`), and `.gitignore` keeps JSON out of `scripts/verifier-eval/`. No history rewrite: memo pins are commit hashes, and every file a memo cites is still at tetel `612f59f`. The old files stay in history, so repo weight is unchanged; census noise is what this fixes. |

---

## A. Blocking — before tetel is usable enough and efficient

Roughly half of these are one defect wearing different hats: **a reply is sized for a terminal and
delivered to a context window.** They are listed separately because they are fixed in different
places, but A2–A7 should be settled together, behind one decision about what a bounded reply is.

| # | Ticket | Why it blocks |
|---|---|---|
| A1 | **TET-91** — Fable 5.1 refuses any agent carrying `look`, `fact` or `record` | The tool cannot be handed to a whole model tier. Nothing else on this list matters for an agent that cannot be spawned. **Done** (PR #34, 2026-09-24). The refusal is a thresholded classifier score over the whole tool set, not one trigger phrase: removing any of several unrelated chunks flips it, and a larger tool set can pass where a subset refuses. Reworded the look/fact/target/render/review/brief/record descriptions and field docs. The grounder, the design attacker and the 14-tool set go from 3/3 refused to 10/10 passing on Fable 5.1, and a 4-turn grounder run completes end to end. Residual: `fact`, `record`, `render` and `target` still refuse as single-tool agents. No CI guard, because the check needs a live model. |
| A2 | **TET-93** — over-cap replies silently replaced by a file stub | The worst failure mode in the tool: an agent restricted to tetel's own verbs gets a path it cannot open, and nothing says evidence was lost. Silence is what makes it first. **Design done** (2026-09-25): `docs/design/tet93-bounded-reply.md` sets the contract (32 KiB budget over content + structured content, declared `maxResultSizeChars`, one backstop in `call_tool`) and the `look`/`fact`/`query` slices; `check` clean after 3 attack + 3 grounding rounds. Slice 1 (the backstop in `call_tool` and the declared threshold) **merged** (PR #36, 2026-09-25). Slice 2 (`look`: a file read pages to whole lines and captures only what it showed; a search shows what fits, captures every match, and leads its reply with the partial caveat, exclusion note and shortfall, where the backstop's cut cannot reach) **merged** (PR #37). Slice 3 (`query`: listings page by `from`, a fact's extents by `extent_from`, `id` on facts and claims; paging line leads the reply) **merged** (PR #38). Slice 4 (`fact`: `advice` names four labels, each cut, and counts the rest; `verify_block` shows every finding with its quoted text cut to 512 bytes or an equal share, withholding and counting only past the floor, for `fact`, `claim` and `prose` alike; `fact`'s lists are kept whole-entry in priority order with an `omitted` count and pointer) **merged** (PR #39, 2026-09-26). The memo was then revised through tetel for slice 4's departures: C9 and C12 now say the paging line leads the reply; C11 sets verify's allowance at the budget minus `ENTRY_CAP`, cuts findings to one common cap found by search, and points omitted refusals at `check`; C14 (vii) now says the backstop never *cuts* a `fact` reply, and only drops its structured copy. A new pin claim, C15, pins the revision at d9474d9. `check` is clean after a final attack pass on the closed text. Surviving qualifications: C14's (ii) is tested through `call_tool` only for `is_error` false, and no red-against-revert run is recorded; C15 credits the revision with ledger writes that the grading passes also made. Follow-up **TET-104** (small): `look` echoes a caller's grep pattern uncut (a 48,889-byte pattern gave a 49,001-byte reply), and `query` echoes an uncut id or `from` in its not-found line. Both reach the backstop's cut only on contrived input; the fix is to cap the echo at `ENTRY_CAP`. **Done** (PR #41): every echo of caller input in `look` (pattern, missing path) and `query` (unmatched id, `from`) is cut to `ENTRY_CAP`, eleven sites, each pinned by a case through a real client that goes red when its cap alone is reverted. |
| A3 | **TET-56** (f2, f3) — bound the census return | The general mechanism the rest of this cluster needs. Decide here what "bounded" means and what a reply says about what it left out; A4–A6 then apply it. **Decided** in the A2 memo (`tet93-bounded-reply.md`); grep paging and a partial verify-delivery cursor deferred. f3's bounded `look` return is A2's slice 2. |
| A4 | **TET-92** — `look --grep` and `fact` name every skipped git-ignored path | A quarter of an attacker run's output. One `fact` reply reached 130k characters that no agent could read. Pure waste — the paths were skipped. **Done** (PR #42): a search's label names the first 8 git-ignored paths, directories first, counts the rest and ends with a sha256 over the whole list, so two sets still pin differently; every name moves to a new `ignored` field on the pending entry and the fact's extent. Not the ticket's proposal, which kept the list in the label and cut it per reply: the label reaches about six consumers besides replies. `attention` no longer counts a note's location as covered by a search that skipped it. Left open: rendered-memo paths are still listed whole in the label, and so still cover; `query` does not show the `ignored` names. |
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
| A16a | **TET-99** — a refused `verify.typed_model` value turns the typed legs off without a word in the reply | **Done** (PR #26, 2026-09-23), taken ahead of A1–A16 as a follow-up to PR #23. The refusal is now stated as `typed_model_refused`, beside `typed_model_not_run` and under the same conditions. It is not in `detail` as first planned: `verify.model`'s refusal reaches `detail` only by turning the status `unauthorized`, and a refused typed model leaves the status alone. The value is named only when it is a well-formed model name, otherwise withheld as `hides_rejected_value` withholds it. Same class as A16, which is still open. |
| A17 | **TET-84** — `verify` returns `unavailable` on 31% of mints | **Done**: all three parts merged, the last on 2026-09-24 (PR #32). Part three follows `docs/design/tet84-loud-status.md` (PR #31). A failed verification's reply carries its `detail`, and the reply text now goes to the log-only `Record.reply`. Under the three failure statuses `guidance` says the mint was not checked. `unverified` names every mint whose latest verification failed, on every reply; withdrawn claims and verbs no longer verified are left out. Known gap: if `claims.jsonl` cannot be read, withdrawn claims appear in `unverified`. Parts one and two, as recorded 2026-09-23: The 19 `unavailable` mints were budget expiry reported under the wrong status. The 19 `unavailable` mints were budget expiry reported under the wrong status. The provider sends its headers at once and its body when the model is done, so the budget ran out during the body read, and that path reported `unavailable` / "reply body could not be read". It is now `timeout` (PR #28). It was not out-of-credit: that is a 402, which already had its own detail. The "four attempts" were the verification's calls across its legs, not retries. The default budget is now 100s per OpenRouter leg, up from 60s (PR #29), because on `gpt-6-luna` 60s still cut off 12 of 113 answered gated `fact` draws. Re-running a verification without changing the text is deferred to TET-100 (A17a). |
| A17a | **TET-100** — re-run a mint's failed verification without changing its text | Split out of A17 by its design. An unchanged revision starts no verification, so today the author can usually clear a mint from `unverified` only by rewording it, changing its citations or withdrawing it (design memo, "Deferred"). A transient provider failure therefore stays on every reply until the author changes something that was not wrong. |
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
| B4a | **TET-97** — the verifier-eval corpora sit in the worktree every census searches | **Done**, see N3. |
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
