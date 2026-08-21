//! Renders the output contract: exactly two labelled partitions, each
//! stating its own scope on the same line, no standalone document-level
//! verdict anywhere. See the crate README / design memo for the contract
//! this implements.

use crate::checks::Findings;
use crate::parse::Document;
use crate::snapshot::Provenance;

/// Exit codes. 0 and 1 are the conventional pass/fail; 2 is the D8 state —
/// "no tetel rows found" — which must never be confusable with clean.
pub const EXIT_CLEAN: i32 = 0;
pub const EXIT_CHECK_FAILED: i32 = 1;
pub const EXIT_NO_ROWS: i32 = 2;

/// Canonical category names for the MACHINE-CHECKED partition, in the
/// order `render` sums their failures below. The single enumerated source
/// for that partition: this module's own scope string and the MCP `check`
/// tool description (`mcp::check_description`) both build their lists from
/// this array rather than each restating it by hand, which is how the
/// three hand-maintained enumerations this array replaced drifted apart —
/// see `tests/mcp_cli.rs`'s `tool_descriptions_stay_tied_to_the_behaviour_they_promise`,
/// which pins set membership against it, and the README test that does the
/// same for the one consumer that cannot execute Rust.
pub const MACHINE_CHECKED_CATEGORIES: &[&str] = &[
    "grammar",
    "subset (enumerated rows only)",
    "abutting literals",
    "unsettled citations",
    "dependency cascades",
    "evidence-ledger import",
    "verdict disagreement",
    "claims out of proof",
    "uncensused modification targets",
    "transplant premises that are not the donor's words or that nothing answers",
    "an unreadable acknowledgement log",
    "provenance drift",
];

/// Canonical category names for the HUMAN-OWED partition. Same rule and
/// same guards as [`MACHINE_CHECKED_CATEGORIES`].
///
/// Two entries here — "whether a claim was graded by the workspace that
/// authored it or an independent one" and "a missing snapshot" — were
/// absent from this module's own scope string before this array existed,
/// even though `render` already printed both (`findings.grounding_provenance`
/// below, and the `Provenance::Missing`/`unverifiable_targets`/
/// `unverifiable_transplants` cases). Both were already named correctly in
/// the MCP description and, for the first, in the README — this array is
/// the union of what every site got right, not a transcription of any one
/// of them.
pub const HUMAN_OWED_CATEGORIES: &[&str] = &[
    "every READING/OBSERVED/ATTESTED row",
    "every row whose domain or extent contains a proc:/external designator",
    "the working-tree states this memo's facts were taken against",
    "the RUN command\u{2194}proposition correspondence",
    "cited-but-undefined and defined-but-uncited ids",
    "ungrounded ledger claims",
    "claims grounded only by attested (ingested) evidence",
    "evidence sources that do not resolve",
    "ledger claims with no declared scope at all",
    "qualified verdicts",
    "superseded evidence",
    "facts whose note names a location outside their own captured extent",
    "refusals recorded in a fact's own mint window",
    "prose revised after the claims it cites settled",
    "prose whose revised-after-proof listing was acknowledged",
    "whether a claim was graded by the workspace that authored it or an independent one",
    "a missing snapshot",
    "a pre-dialect extent — no-match or match — whose pattern contains an unescaped ERE metacharacter (| + ? ( ) { })",
    "tetel's own standing non-coverage",
];

/// Joins category labels into a natural-English list — plain commas
/// between all but the last pair, `", and "` before the last — since these
/// are read as prose, not printed as a slice. Shared by this module's own
/// scope strings and by `mcp::check_description`, so the same join style
/// renders both partitions everywhere they appear.
pub fn join_categories(categories: &[&str]) -> String {
    match categories {
        [] => String::new(),
        [only] => (*only).to_string(),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

const NON_COVERAGE: &[&str] = &[
    "dependents that never declared themselves",
    "deleted premises",
    "one word used in two senses",
    "a command that runs green without establishing its proposition",
    "unfalsifiable-shaped claim phrasing",
    // --- what a modification-target census cannot see ------------------
    // The census refusal is textual and rooted, and each of these is a
    // way a use of a symbol escapes a textual search or a way a found
    // site still tells you nothing. Printed rather than left implicit,
    // because a refusal that fired is easily read as coverage it never
    // claimed.
    "uses of a censused symbol reached by dynamic dispatch, re-export, aliased import, \
or a name assembled by a macro or string concatenation",
    "whatever the platform's grep skipped — symlinked directories and binary files bound \
what a whole-worktree search physically visited",
    // A separate line, deliberately not folded into the one above. That
    // entry names two *platform accidents* and contemplates nothing tetel
    // chose; widening it would turn "rooted at the worktree" into "rooted
    // at the worktree except what the tool decided to hide" inside a
    // sentence about symlinks. A deliberate exclusion earns its own line,
    // by the same standard as every other entry here. This can only ever
    // be the general statement — every string in this list is static and
    // none is per-search — which is why the per-search exclusion set is
    // also written into the search record's own label, where a reader of
    // the Facts table sees exactly which paths a given search withheld.
    "tetel's own output, which `look --grep` skips when traversing: snapshot directories, \
evidence ledgers, and rendered memos are not visited by a search rooted anywhere above \
them, so a census reports the tree minus what tetel previously wrote into it. Each \
search's own record names the exclusions it was given; `look` on a named path, and a \
search rooted inside tetel's output, are not filtered at all",
    "whether the censused worktree is the tree this design is actually about; rooting says \
where a search started, never that it started in the right place",
    "which mode a call site selects: a census enumerates sites, never the argument values \
those sites pass, so a parameter choosing between materially different mechanisms is \
invisible even to a careful reader of the census",
    "a modification target the author never declared — no refusal can reach it, and the \
empty section is its only trace",
    // --- what a transplant's premise inventory cannot see --------------
    // The refusal establishes that a premise is the donor's words and
    // that something answers it. Each of these is a way a load-bearing
    // premise never becomes a row at all, or a way a row that exists
    // still settles nothing — printed for the same reason as the census
    // entries above.
    "a transplant the author never declared — the mechanism is carried across and no premise \
is ever asked for; the empty section is its only trace",
    "premises the donor never wrote down — a premise carried by a function's structure rather \
than its comments has nothing to select from",
    "premises outside what the donor fact captured — a narrow look cannot yield a premise from \
lines it never opened, and the remedy is looking again rather than typing",
    "whether the selected premises are all the load-bearing ones — the refusal establishes that \
selected bytes are the donor's, never that stopping there was honest; deciding a selection is \
too small is a truth-check, not a format one",
    "whether an answering claim actually addresses the premise it answers, and whether the cited \
target is really where the mechanism lands — a grounding pass grades each claim as written, and \
the linkage between them is graded by nobody",
    "a donor comment that is itself wrong — quotation establishes provenance, never truth",
    // --- what "prose revised after the claims it cites settled" cannot see --
    // The check compares two timestamps and two content-equality tests;
    // each of these is either a case the comparison structurally cannot
    // reach or a question the comparison was never built to answer.
    // Printed for the same reason as the entries above: a residue that
    // fired is easily mistaken for coverage it never claimed.
    "false prose that ships alongside a claim revised in the same round — the ticket this check \
answers concedes it, and this design does not fix it",
    "a paragraph that cites nothing — there is no citation edge to compare its wording against",
    "a citation added to a claim not yet in proof — by the time any pass proves that claim, its \
first proof necessarily postdates the edit, so this can never be listed",
    "whether the two clocks behind this ordering — the authoring workspace's and the grounding \
pass's — actually agree",
    "whether a listed paragraph is wrong at all; nothing here judges that, only that its wording \
postdates its evidence",
    // --- added on the round-2 code review that found the anchor was
    // verdict-blind and the empty-digest grandfather was being read as a
    // timestamp — see `prose_after_proof`'s doc comment for the full
    // argument. Both entries name a case where this check now stays
    // silent that an earlier version of it did not.
    "a claim graded only by a refutation — a refuting record examined the wording and rejected \
it, so it does not put the wording in proof and cannot anchor a block; a paragraph resting only \
on such a claim is never listed here, whatever `verdict-disagreement` says about the refutation \
itself",
    "a claim graded only by evidence recorded before its proposition carried a digest — that \
record cannot say which wording it graded, so it cannot anchor a block even in the case it did \
in fact grade today's wording; only a record whose digest matches the current text counts",
    "a citation whose claim settled before the block's wording, when another citation in the \
same block settled after it — the anchor is the latest first proof among all of a block's \
citations, so the earlier-settled one is never compared against the wording on its own; a \
co-cited claim that keeps being revised and re-entering proof keeps raising the anchor, and \
whether the wording was ever actually examined against the earlier-settled claim is never \
answered by any listing here",
    // --- what a `tetel prose --ack` acknowledgement does not establish --
    // An acknowledgement records that a human said they re-read a block;
    // it does not verify the reading, does not judge the paragraph, and
    // is only ever as current as the last render. Printed for the same
    // reason as every entry above: a discharge that fired is easily read
    // as a stronger finding than it is.
    "whether an acknowledged paragraph is actually faithful to the claims it cites — an \
acknowledgement records that a human said they re-read it and found nothing to change; nothing \
here verifies the reading happened or judges the paragraph itself",
    "an acknowledgement minted after this memo's last render — it is not in the snapshot this \
check reads, so it suppresses nothing until the memo is rendered again",
    "a hand-edited or hand-added acknowledgement log — a snapshot's acks.jsonl is shipped but \
never re-rendered, the same standing exclusion as counters.json, pending.json, refusals.log and \
identity.json, so provenance drift does not cover it",
];

/// Renders a Unix timestamp (seconds) as a UTC calendar date and time a
/// reader can place at a glance, without pulling in a date/time dependency
/// this crate does not otherwise need. Civil-from-days conversion after
/// Howard Hinnant's public-domain `civil_from_days` algorithm
/// (<https://howardhinnant.github.io/date_algorithms.html>), which this
/// crate has no test suite of its own to validate against a calendar
/// library — spot-checked instead against this machine's own `date -u -r`
/// for the two epochs this fix's own bug report cited (1754683001 →
/// 2025-08-08 19:56:41 UTC, 1754682955 → 2025-08-08 19:55:55 UTC), which is
/// what `format_unix_renders_a_known_epoch_as_a_known_calendar_date` pins.
fn format_unix(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let secs_of_day = ts % 86_400;
    let (y, m, d) = civil_from_days(days);
    let (h, mi, s) = (secs_of_day / 3600, (secs_of_day % 3600) / 60, secs_of_day % 60);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

/// Days since the Unix epoch (1970-01-01) to a proleptic-Gregorian
/// (year, month, day). See `format_unix`'s doc comment for provenance.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Renders a duration in seconds as a human-legible interval ("46s", "3m
/// 5s", "2h 5m", "1d 3h") — coarsened to the two largest non-zero units so
/// a reader gets "how far apart" at a glance rather than a strict duration
/// they'd have to parse. What made this fix necessary: `1754683001` minus
/// `1754682955` is not a computation a reader does in their head, and the
/// finding this prints is entirely about that gap.
fn format_interval(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    let units: [(u64, &str); 4] = [(days, "d"), (hours, "h"), (mins, "m"), (s, "s")];
    let parts: Vec<String> = units
        .iter()
        .filter(|(v, _)| *v > 0)
        .take(2)
        .map(|(v, u)| format!("{v}{u}"))
        .collect();
    if parts.is_empty() { "0s".to_string() } else { parts.join(" ") }
}

/// `build` names the binary that produced this report (see `buildid.rs`).
/// It is passed in rather than read here so this function stays a pure
/// function of the document it grades — and it is *always* printed,
/// including on the no-rows path, because the whole point is that two
/// outputs which disagree can be told apart by their checker. A report
/// that does not name its build cannot be disbelieved.
pub fn render(display_path: &str, doc: &Document, findings: &Findings, build: &str) -> (i32, String) {
    if doc.row_groups_found == 0 && findings.ledger_claims_found == 0 {
        let msg = format!(
            "no tetel rows found in {display_path} — out of scope, nothing was checked. \
This is a distinct state from a clean run, not a weaker way of spelling it (exit {EXIT_NO_ROWS}).\n\
\nchecked by {build}\n"
        );
        return (EXIT_NO_ROWS, msg);
    }

    let mut out = String::new();

    // --- machine-checked partition -----------------------------------
    let failing = findings.machine_check_failed();
    let total_failures = findings.grammar_errors.len()
        + findings.subset_failures.len()
        + findings.abutting_failures.len()
        + findings.unsettled_failures.len()
        + findings.cascade_failures.len()
        + findings.ledger_errors.len()
        + findings.verdict_disagreements.len()
        + findings.out_of_proof.len()
        + findings.uncensused_targets.len()
        + findings.unquoted_premises.len()
        + usize::from(findings.acks_unreadable.is_some())
        + usize::from(findings.provenance_failed());
    let scope = join_categories(MACHINE_CHECKED_CATEGORIES);
    if failing {
        out.push_str(&format!(
            "machine-checked: {total_failures} failing — {scope}\n"
        ));
        for e in &findings.grammar_errors {
            out.push_str(&format!("  - [grammar] {e}\n"));
        }
        for e in &findings.subset_failures {
            out.push_str(&format!("  - [subset] {e}\n"));
        }
        for e in &findings.abutting_failures {
            out.push_str(&format!("  - [abutting-literal] {e}\n"));
        }
        for e in &findings.unsettled_failures {
            out.push_str(&format!("  - [unsettled-citation] {e}\n"));
        }
        for e in &findings.cascade_failures {
            out.push_str(&format!("  - [cascade] {e}\n"));
        }
        for e in &findings.ledger_errors {
            out.push_str(&format!("  - [ledger] {e}\n"));
        }
        for e in &findings.verdict_disagreements {
            out.push_str(&format!("  - [verdict-disagreement] {e}\n"));
        }
        for e in &findings.out_of_proof {
            out.push_str(&format!("  - [out-of-proof] {e}\n"));
        }
        for e in &findings.uncensused_targets {
            out.push_str(&format!("  - [uncensused-target] {e}\n"));
        }
        for e in &findings.unquoted_premises {
            out.push_str(&format!("  - [unquoted-premise] {e}\n"));
        }
        if let Some(e) = &findings.acks_unreadable {
            out.push_str(&format!(
                "  - [ack-log-unreadable] a snapshot exists beside this document but its \
acknowledgement log (acks.jsonl) could not be read ({e}) — reported rather than passed over, \
because an unreadable record is not a matching one. Unlike prose.jsonl, this file is never read \
by `render`, so nothing else here has already caught it.\n"
            ));
        }
        match &findings.provenance {
            Provenance::Drifted { first_diff_line, snapshot_lines, memo_lines } => {
                let where_ = match first_diff_line {
                    Some(n) => format!("first difference at line {n}"),
                    None => "identical line-for-line but different lengths".to_string(),
                };
                out.push_str(&format!(
                    "  - [provenance-drift] this document is not what its own snapshot renders \
({where_}; snapshot {snapshot_lines} lines, document {memo_lines}). Either the document was \
edited by hand after rendering, or the workspace moved on without a re-render — a reader \
following a citation would land somewhere this text was never produced from. Re-render, or \
recover the workspace the text really came from.\n"
                ));
            }
            Provenance::Unreadable(e) => {
                out.push_str(&format!(
                    "  - [provenance-drift] a snapshot exists beside this document but could not \
be rendered from ({e}) — reported rather than passed over, because an unreadable record is not \
a matching one.\n"
                ));
            }
            Provenance::Missing | Provenance::Matches => {}
        }
    } else {
        out.push_str(&format!("machine-checked: clean — {scope}\n"));
    }
    if !findings.abutting_candidates.is_empty() {
        out.push_str("  informational, not checked (never a failure, at any distance looser than abutting):\n");
        for c in &findings.abutting_candidates {
            out.push_str(&format!("    - {c}\n"));
        }
    }

    out.push('\n');

    // --- human-owed partition -----------------------------------------
    out.push_str(&format!(
        "human-owed: {} \u{2014} none of this is settled by a passing check\n",
        join_categories(HUMAN_OWED_CATEGORIES)
    ));
    for e in &findings.superseded_evidence {
        out.push_str(&format!("  - superseded evidence: {e}\n"));
    }
    for (id, pass, note) in &findings.qualified_claims {
        out.push_str(&format!(
            "  - {id}: QUALIFIED by pass {pass} — not a plain confirmation. \"{note}\"\n"
        ));
    }
    for line in &findings.grounding_provenance {
        out.push_str(&format!("  - {line}\n"));
    }
    for w in &findings.mint_windows {
        let window = if w.is_first {
            "before this fact, the first minted in its workspace"
        } else {
            "between this fact and the one before it"
        };
        out.push_str(&format!(
            "  - {}: {} refusal(s) recorded {window} — what the author tried and could not do in \
the window that produced it. Frequently innocent; worth a reader's eye when a note reaches past \
its extent, because a refused `look` leaves the pending buffer untouched and the next mint folds \
whatever was already there:\n",
            w.fact_id,
            w.refusals.len()
        ));
        for line in &w.refusals {
            out.push_str(&format!("      {line}\n"));
        }
        if w.straddles_a_boundary {
            out.push_str(
                "      (a refusal here shares a second with a mint, so which side of it the \
refusal fell on cannot be recovered — it is listed under both adjacent facts rather than \
guessed at)\n",
            );
        }
    }
    if !findings.prose_revised_since_proof.is_empty() {
        // One preamble bullet, printed once rather than per entry — the
        // same precedent `ledger_has_no_scope_columns` set: a constant
        // repeated per row buries the findings that are about *this*
        // document. What is constant across every entry below is the
        // cross-process clock bound this whole check rests on, so it is
        // stated here rather than in each block's own bullet.
        out.push_str(
            "  - prose revised after the claims it cites settled (entries below): the ordering \
rests on two clocks nothing here can prove are the same — the authoring workspace's and the \
grounding pass's. A reported ordering can be wrong only if the two differed by more than the \
interval between the prose edit and the pass that first put the cited claim(s) in proof — and \
the same skew in the *other* direction can just as well suppress a listing that should have \
appeared, since this check is silent by default: an entry missing here is not proof the \
ordering was clean, only that no skew large enough to flip it was found\n",
        );
        for p in &findings.prose_revised_since_proof {
            // `p.line` is the offset lookup against `compose::block_offsets`
            // for this same snapshot — it should always resolve, but
            // printing a fabricated `0` on the rare miss would be exactly
            // the overstated-provenance failure this crate exists to
            // avoid. Say so plainly instead.
            let line = match p.line {
                Some(n) => format!("line {n}"),
                None => "line unknown (offset lookup failed)".to_string(),
            };
            // The anchor this block was actually compared against —
            // `p.cited`'s own max, recomputed here rather than carried as
            // a separate field, since it's exactly what "after every
            // claim below had already entered proof" means and every
            // input to it is already printed on the lines beneath. Always
            // `Some`: a listed block has at least one cited entry (the
            // existential gate `prose_after_proof` applies before ever
            // producing one).
            let anchor = p.cited.iter().map(|(_, t, _)| *t).max().unwrap_or(p.block_timestamp);
            let interval = format_interval(p.block_timestamp.saturating_sub(anchor));
            out.push_str(&format!(
                "  - {} ({}): this wording (text and citations) dates from {} (raw {}), {} \
after every claim below had already entered proof:\n",
                p.block_id,
                line,
                format_unix(p.block_timestamp),
                p.block_timestamp,
                interval
            ));
            // The author's own stated reason for the edit that produced
            // this wording — present only when that edit was a `Revise`
            // (a `Create` has none, and none is fabricated here). Never a
            // paraphrase: this is the author's own words, carried
            // verbatim, and the single most actionable thing this entry
            // can hand a reader without quoting the paragraph itself.
            if let Some(why) = &p.why {
                out.push_str(&format!("      revised because: \"{why}\"\n"));
            }
            for (id, first_proof, pass) in &p.cited {
                out.push_str(&format!(
                    "      {id}: first entered proof at {} (raw {}), by pass {pass}\n",
                    format_unix(*first_proof),
                    first_proof
                ));
            }
        }
    }
    if !findings.prose_acknowledged.is_empty() {
        // Same collapsed shape as the preamble above: one constant bullet,
        // then one entry per block. A block reaching this list has left
        // the demanding group above rather than sitting inside it with an
        // annotation stapled on — the human act the group asks for has
        // already been performed, so the standing demand should not stand.
        out.push_str(
            "  - prose acknowledged after the claims it cites settled (entries below): a human \
said, in their own words, that they re-read each block's current text and citations against \
every claim's current wording and found nothing to change. Nothing here verifies that reading or \
judges whether the paragraph is faithful — the tool's silence past this point is the author's \
assertion, not a finding of its own\n",
        );
        for a in &findings.prose_acknowledged {
            let line = match a.line {
                Some(n) => format!("line {n}"),
                None => "line unknown (offset lookup failed)".to_string(),
            };
            out.push_str(&format!(
                "  - {} ({}): acknowledged {} (raw {}) — cites [{}]\n",
                a.block_id,
                line,
                format_unix(a.timestamp),
                a.timestamp,
                a.cited.join(", ")
            ));
            // Same verbatim rule as `revised because:` above: the
            // author's own words, never paraphrased.
            out.push_str(&format!("      acknowledged because: \"{}\"\n", a.why));
        }
    }
    for r in &findings.tree_states {
        out.push_str(&format!(
            "  - {} was observed in {} different working-tree states by this memo's facts. Not a \
defect — a tree moves while a design is written — but it is what tells you whether two facts \
about the same code disagree about the world or about the same world, which their notes alone \
cannot say:\n",
            r.root,
            r.states.len()
        ));
        for (state, ids) in &r.states {
            out.push_str(&format!("      {state}  ←  {}\n", ids.join(", ")));
        }
    }
    if !findings.tree_ungradable.is_empty() {
        out.push_str(&format!(
            "  - [{}]: minted before an observation's tree marker named which tree it described, \
so their recorded state is this tool's own working directory at capture time and not the tree they \
read. Nothing can be compared against it — reported rather than passed over, because an ungradable \
record is not a matching one. Re-observing under the current build is the only repair\n",
            findings.tree_ungradable.join(", ")
        ));
    }
    for o in &findings.notes_outside_extent {
        out.push_str(&format!(
            "  - {}: its note names {}, which this fact's extent does not cover (extent: {}) — \
the extent was captured by the tool, the note was written, so the two disagreeing is worth a \
reader's eye. A note may name a location as context; only you can tell that from a conclusion \
drawn about code this fact never opened\n",
            o.fact_id,
            o.mentioned,
            o.extent_labels.join("; ")
        ));
    }
    for line in &findings.pre_dialect_no_matches {
        out.push_str(&format!("  - {line}\n"));
    }
    if matches!(findings.provenance, Provenance::Missing) && findings.cites_something {
        out.push_str(
            "  - no workspace snapshot beside this document: its citation ids are \
workspace-relative, so without the workspace that minted them every citation here is a pointer \
this repository cannot resolve. Re-render with `tetel render --out <this file>` to write one.\n",
        );
    }
    for (id, kind_status, claim) in &findings.human_owed_rows {
        out.push_str(&format!("  - {id} [{kind_status}]: {claim}\n"));
    }
    for (id, claim) in &findings.coverage_skipped {
        out.push_str(&format!(
            "  - {id}: coverage not machine-checked (domain or extent contains a proc:/external designator, so no coverage claim of any strength is made) — {claim}\n"
        ));
    }
    if !findings.run_row_ids.is_empty() {
        out.push_str(&format!(
            "  - RUN rows [{}]: a matching re-run establishes only that the command reproduces its stored value, never that the value establishes the claim\n",
            findings.run_row_ids.join(", ")
        ));
    }
    if !findings.cited_undefined.is_empty() {
        out.push_str(&format!(
            "  - cited but undefined: [{}]\n",
            findings.cited_undefined.join(", ")
        ));
    }
    for (id, claim) in &findings.defined_uncited {
        out.push_str(&format!(
            "  - {id}: defined but never cited; default disposition is delete, not hunting for a citation — {claim}\n"
        ));
    }
    for (id, proposition) in &findings.ungrounded_claims {
        out.push_str(&format!(
            "  - {id}: ungrounded — no evidence record on file — {proposition}\n"
        ));
    }
    for (id, proposition) in &findings.attested_grounded_claims {
        out.push_str(&format!(
            "  - {id}: grounded only by attested evidence — someone looked, off-instrument; \
distinct from no evidence at all, but never enough on its own to move past vouched — {proposition}\n"
        ));
    }
    for line in &findings.unresolved_evidence_sources {
        out.push_str(&format!("  - {line}\n"));
    }
    for (id, proposition) in &findings.no_scope_claims {
        out.push_str(&format!(
            "  - {id}: no scope declared (tetel's authoring model has no domain/extent field on a claim) — no coverage claim of any strength is made — {proposition}\n"
        ));
    }
    // Once, not once per claim. This is a fact about the authoring model,
    // identical for every claim in the document and dischargeable by
    // nobody; repeating it per row buried the findings that are about
    // *this* document under a constant.
    if findings.ledger_has_no_scope_columns {
        out.push_str(
            "  - no claim in this document declares a scope: `tetel claim` has no such field, \
so no coverage claim of any strength is made by any row. What each claim rests on is in the \
Facts table; whether it rests on enough is yours to judge\n",
        );
    }
    for e in &findings.unverifiable_targets {
        out.push_str(&format!(
            "  - target `{e}` is declared in this document but no snapshot shipped beside it, \
so nothing here can verify the census behind it — reproduce it from the workspace, or re-render with `--out`\n"
        ));
    }
    for e in &findings.unverifiable_transplants {
        out.push_str(&format!(
            "  - transplant {e} is shown in this document but no snapshot shipped beside it, so \
nothing here can verify that its premises are the donor's words or that anything answers them — \
reproduce it from the workspace, or re-render with `--out`\n"
        ));
    }
    for item in NON_COVERAGE {
        out.push_str(&format!("  - tetel does not catch: {item}\n"));
    }

    out.push_str(&format!("\nchecked by {build}\n"));

    let code = if failing { EXIT_CHECK_FAILED } else { EXIT_CLEAN };
    (code, out)
}

#[cfg(test)]
mod tests {
    use super::{format_interval, format_unix, HUMAN_OWED_CATEGORIES, MACHINE_CHECKED_CATEGORIES};

    /// The mint-time verifier belongs to neither partition, and this is
    /// the assertion that keeps it out of both.
    ///
    /// These two arrays are not a taxonomy of findings in general — they
    /// are a statement of what `check` covers, and the module's scope
    /// strings and the `check` tool description are generated from them.
    /// The verifier does not run inside `check`, does not enter the
    /// record, the memo, the snapshot or the ledger, and does not repeat:
    /// the same input can produce a different answer, so a reader holding
    /// the document could not recompute it even in principle. Naming it
    /// in either array would make a scope string promise coverage `check`
    /// does not have, which is the exact failure the enumeration was
    /// introduced to end.
    #[test]
    fn neither_partition_claims_to_cover_the_verifier() {
        for category in MACHINE_CHECKED_CATEGORIES.iter().chain(HUMAN_OWED_CATEGORIES) {
            let lower = category.to_ascii_lowercase();
            assert!(
                !lower.contains("verif") && !lower.contains("model"),
                "`{category}` promises coverage `check` does not have"
            );
        }
    }

    /// Round-2 code review (C): the two epochs the review's own bug
    /// report quoted (`"this wording ... dates from 1754683001 ... C1:
    /// first entered proof at 1754682955"`), spot-checked against this
    /// machine's own `date -u -r <epoch> "+%Y-%m-%d %H:%M:%S UTC"`:
    /// 1754683001 → 2025-08-08 19:56:41 UTC, 1754682955 → 2025-08-08
    /// 19:55:55 UTC. Corroboration only — verified on this machine's own
    /// `date`, not against a calendar library, since this crate pulls in
    /// none.
    #[test]
    fn format_unix_renders_a_known_epoch_as_a_known_calendar_date() {
        assert_eq!(format_unix(1_754_683_001), "2025-08-08 19:56:41 UTC");
        assert_eq!(format_unix(1_754_682_955), "2025-08-08 19:55:55 UTC");
    }

    #[test]
    fn format_unix_handles_the_epoch_itself() {
        assert_eq!(format_unix(0), "1970-01-01 00:00:00 UTC");
    }

    /// The exact case the bug report named: two epochs 46 seconds apart
    /// must read as "46s", not as two raw integers a reader has to
    /// subtract by hand.
    #[test]
    fn format_interval_reports_the_named_46_second_gap() {
        assert_eq!(format_interval(1_754_683_001 - 1_754_682_955), "46s");
    }

    #[test]
    fn format_interval_coarsens_to_the_two_largest_units() {
        assert_eq!(format_interval(0), "0s");
        assert_eq!(format_interval(5), "5s");
        assert_eq!(format_interval(65), "1m 5s");
        assert_eq!(format_interval(3_665), "1h 1m", "seconds are dropped once hours and minutes both show");
        assert_eq!(format_interval(90_061), "1d 1h", "days coarsen away minutes and seconds entirely");
    }
}
