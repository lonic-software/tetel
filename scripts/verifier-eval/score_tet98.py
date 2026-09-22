#!/usr/bin/env python3
"""Score TET-98's runs: the shipped verifier, through `examples/verify_corpus.rs`.

Four arms, three draws each, over `tet98_tasks.py`'s subjects:

  claim_candidate  typed_model=typesafe/jev-latest, literals on  (what would ship)
  claim_default    typed_model unset, literals off               (what ships today)
  fact_gated       typed_model set, literals on   — the fact row's gate on
  fact_ungated     typed_model unset, literals on — the same, gate off

Rules, the retrodiction's: a subject is flagged by majority of its non-error
draws (`ok` with a finding). `gated` is an answer, not an error; every other
status is an error column that enters no denominator. When the driver re-ran
a draw, its last line is the one read.

Claims are graded by `score_classify_ab.grade` in-sample; out-of-sample
claims, which no earlier adjudication saw, by OUT_CLAIM below. Facts: the
fitted positives are `defects_v1.json`'s twelve; the held-out ones are
OUT_FACT below, graded from the ungated arm's findings in ANY draw — a gate
must not skip a real defect whichever draw happened to find it.

    python3 score_tet98.py [run dir]           # tables, then everything left to read
"""

import collections, json, sys
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import score_classify_ab as SCA  # noqa: E402

RUN = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE.parents[2] / "tetel-eval-runs" / "tet98"
LINE = 0.113  # design C19: sound claims flagged, as a rate
FIT_DEFECTS = {tuple(s) for s in json.load(open(HERE / "defects_v1.json"))["fact"]["subjects"]}

# Out-of-sample findings, graded by hand against the snapshot's evidence:
# (memo, id, clause prefix) -> (CORRECT|WRONG, why). "" matches every clause.
# 2026-09-22, claim_flagged_adjudicated.md's standard (default WRONG), the
# graders' notes in each memo's .evidence.jsonl read as a second opinion.
# A claim's specific CORRECT clause is listed before its "" catch-all.
OUT_CLAIM = {
    ("tet-verifier-mint-warning.md", "C10", ""): ("WRONG", "misread referent: the verifier's finding taken for the note-outside-extent category"),
    ("tet-verifier-mint-warning.md", "C11", "a new workspace file is withheld"): ("CORRECT", "entries carry no justifying comment; the grader qualified on the same clause"),
    ("tet-verifier-mint-warning.md", "C11", ""): ("WRONG", "an absent file skipped is not a render that fails to ship it"),
    ("tet-verifier-mint-warning.md", "C14", ""): ("WRONG", "insufficiency: the search skipped only tetel's own output"),
    ("tet-verifier-mint-warning.md", "C17", ""): ("WRONG", "insufficiency: a sampled head read as not covering"),
    ("tet-verifier-mint-warning.md", "C18", ""): ("CORRECT", "minor: 'the largest bucket at 185' against 578 supports in the same capture"),
    ("tet-verifier-mint-warning.md", "C8", ""): ("WRONG", "proposal read as current: the verify.* keys are what the design adds"),
    ("tet42-committed-artifact-paths.md", "C1", ""): ("CORRECT", "minor: porcelain lists untracked entries; three graders qualified on it"),
    ("tet42-committed-artifact-paths.md", "C12", ""): ("WRONG", "all six figures are in F9's output: a Jev literal with its counted word attached"),
    ("tet42-committed-artifact-paths.md", "C14", ""): ("CORRECT", "look never refuses a relative path; graders qualified on the same sentence"),
    ("tet42-committed-artifact-paths.md", "C16", ""): ("WRONG", "insufficiency, and the no-git marker misread as a different repository"),
    ("tet42-committed-artifact-paths.md", "C17", ""): ("WRONG", "the pin claim is the memo's C1, not a fact's pin; the rest insufficiency"),
    ("tet42-committed-artifact-paths.md", "C18", ""): ("WRONG", "insufficiency: a universal over constructed cases"),
    ("tet42-committed-artifact-paths.md", "C2", ""): ("WRONG", "275 and 350 are in F3's output: a Jev literal with its counted word attached"),
    ("tet42-committed-artifact-paths.md", "C20", ""): ("CORRECT", "scripts/grep-dialect-census.py names both types; graders qualified on the same sentence"),
    ("tet42-committed-artifact-paths.md", "C21", ""): ("CORRECT", "minor: 'exactly two places' beside three counted fields"),
    ("tet42-committed-artifact-paths.md", "C22", ""): ("WRONG", "insufficiency: graders confirm both render sites print the label verbatim"),
    ("tet42-committed-artifact-paths.md", "C23", ""): ("WRONG", "the 29 resolving paths were resolved in this worktree, not in the clone the sentence names"),
    ("tet42-committed-artifact-paths.md", "C27", ""): ("CORRECT", "minor: search_key falls back when canonicalize fails, the fallback C4's graders qualified on"),
    ("tet42-committed-artifact-paths.md", "C4", ""): ("CORRECT", "resolve_key falls back to the caller's spelling; graders qualified on the same parenthetical"),
    ("tet42-committed-artifact-paths.md", "C7", ""): ("CORRECT", "world_root's default carries a positive meaning, and pattern is a fifth; graders qualified on it"),
    # An `unevidenced` finding is CORRECT only when the value is absent from
    # the cited capture AND a grader found it wrong or could not establish it;
    # a figure the graders reproduced is not a warning the author wanted
    # (tet-verifier-jev C4, C5).
    ("tet-verifier-jev.md", "C1", ""): ("CORRECT", "README.md modified and five untracked entries; graders qualified on both clauses"),
    ("tet-verifier-jev.md", "C13", ""): ("WRONG", "F12 carries '20 / 50' and '4 / 38'; the ordering clause is the proposal"),
    ("tet-verifier-jev.md", "C14", ""): ("WRONG", "insufficiency: a universal negative over runs"),
    ("tet-verifier-jev.md", "C15", ""): ("CORRECT", "the captured CLAUSE regex cuts only before whitespace and only at spaced em/en dashes; graders supported"),
    ("tet-verifier-jev.md", "C16", ""): ("CORRECT", "the capture scores a Jev refuter on prose too, not fact alone"),
    ("tet-verifier-jev.md", "C17", ""): ("CORRECT", "minor: the endpoint returned jev-1.13.0, not typesafe/jev-1.13.0, the constant a drift flag would compare"),
    ("tet-verifier-jev.md", "C18", ""): ("WRONG", "proposal read as current: the TYPESAFE_API_KEY arm is what the design adds"),
    ("tet-verifier-jev.md", "C19", ""): ("WRONG", "the counts matched; the proposal's reasoning is not a capture"),
    ("tet-verifier-jev.md", "C2", ""): ("WRONG", "the captured check leg is the LLM's, which the claim says"),
    ("tet-verifier-jev.md", "C4", ""): ("WRONG", "the flagged clause is true; the defect graders found is the numbers attached to it"),
    ("tet-verifier-jev.md", "C5", ""): ("WRONG", "the counts are stated beside 'equal quality'; the August figure is right"),
    ("tet-verifier-jev.md", "C7", ""): ("CORRECT", "the captured error response has no model field; graders qualified on 'each response'"),
}
# In-sample claims only this run flagged: the earlier adjudications never saw
# these findings. Same standard, same rule for `unevidenced`.
IN_CLAIM = {
    ("tet30-prose-revised-since-grounding.md", "C11", ""): ("WRONG", "a count of what the design adds; graders qualified elsewhere"),
    ("tet30-prose-revised-since-grounding.md", "C2", ""): ("WRONG", "'Two things' measures nothing"),
    ("tet30-prose-revised-since-grounding.md", "C4", ""): ("CORRECT", "uncaptured, and a grader found a tet28 post-pass revision that is a paragraph, not a heading"),
    ("tet46-look-never-returns-tetel-content.md", "C10", ""): ("CORRECT", "uncaptured; graders could not establish the 707,751-character figure"),
    ("tet46-look-never-returns-tetel-content.md", "C12", ""): ("CORRECT", "uncaptured; graders qualified on exactly this counter-instance"),
    ("tet46-look-never-returns-tetel-content.md", "C6", ""): ("WRONG", "insufficiency; graders qualified on counts the finding did not name"),
    ("tet47-ground-what-is-owed.md", "C13", ""): ("CORRECT", "scan_cites_trailer returns unbracketed ids first; a grader qualified on the same clause"),
    ("tet47-ground-what-is-owed.md", "C19", ""): ("CORRECT", "graders narrowed 'owed_claims ... no occurrence in the worktree', the clause flagged"),
    ("tet56-bounded-grep-return.md", "C11", ""): ("WRONG", "insufficiency over tetel's own excluded output; graders supported"),
}
OUT_FACT = {
    ("tet-verifier-jev.md", "F19", ""): ("CORRECT", "minor: 'eight source files' where the capture shows six .rs files; the finding's own seven is also off"),
    ("tet42-committed-artifact-paths.md", "F5", ""): ("WRONG", "only key and label are copied as-is, which is the note's subject"),
    ("tet42-committed-artifact-paths.md", "F6", ""): ("CORRECT", "'canonical-absolute by construction' where resolve_key falls back to the caller's spelling"),
    ("tet42-committed-artifact-paths.md", "F7", ""): ("WRONG", "the negative is true: no fact-pin equality comparison in src"),
    ("tet42-committed-artifact-paths.md", "F8", ""): ("CORRECT", "minor: 'one byte' where printf 'x\\n' appends two"),
    ("tet42-committed-artifact-paths.md", "F10", ""): ("WRONG", "`file(s)` paraphrases the capture's plural logic"),
    ("tet42-committed-artifact-paths.md", "F11", ""): ("CORRECT", "minor: 'all share one' root beside the note's own no-git-worktree entry"),
    ("tet42-committed-artifact-paths.md", "F12", ""): ("CORRECT", "minor: the symlink is made after the commit, not in it"),
    ("tet42-committed-artifact-paths.md", "F13", ""): ("CORRECT", "minor: the matches include twelve.html, not only *.json"),
    ("tet42-committed-artifact-paths.md", "F14", ""): ("WRONG", "the five construction sites and the census.py mention are in the capture"),
    ("tet42-committed-artifact-paths.md", "F15", ""): ("WRONG", "the skipped paths are tetel's own output, disclosed"),
    ("tet42-committed-artifact-paths.md", "F20", ""): ("CORRECT", "'no other file under src/ names it' where the capture matches src/facts.rs"),
    ("tet42-committed-artifact-paths.md", "F21", ""): ("WRONG", "loose wording, not a false fact"),
    ("tet42-committed-artifact-paths.md", "F23", ""): ("WRONG", "true restatement cross-referenced from measured facts"),
    ("tet42-committed-artifact-paths.md", "F27", ""): ("WRONG", "an edge-case fallback left out; the conclusion holds"),
    ("tet42-committed-artifact-paths.md", "F29", ""): ("WRONG", "the percentages are right, unrounded"),
    ("tet42-committed-artifact-paths.md", "F30", ""): ("WRONG", "../outer/x.txt is the constructed case; the corpus figures match"),
    ("tet42-committed-artifact-paths.md", "F31", ""): ("WRONG", "the hypothetical under the normalizing reading, as stated"),
    ("tet42-committed-artifact-paths.md", "F34", ""): ("WRONG", "true restatement; the sums follow"),
    ("tet42-committed-artifact-paths.md", "F35", ""): ("CORRECT", "minor: 'all recorded model outputs' where the matches include cases.py"),
    ("tet42-committed-artifact-paths.md", "F40", ""): ("CORRECT", "minor: 'one-file repositories' where each commits x.txt and sub/f.txt"),
    ("tet-verifier-jev.md", "F2", ""): ("WRONG", "the note itself says the variant continues past line 140"),
    ("tet-verifier-jev.md", "F3", ""): ("CORRECT", "json! emits refuter_model as null, not absent, when unset"),
    ("tet-verifier-jev.md", "F5", ""): ("WRONG", "true restatement: labels_fact_v1.json grades fact_v1.json's findings"),
    ("tet-verifier-jev.md", "F6", ""): ("WRONG", "true restatement of score_gate_variants.py's MARGIN, runs dir and inputs"),
    ("tet-verifier-jev.md", "F9", ""): ("WRONG", "the quoted object sits verbatim in the captured detail array, abridged"),
    ("tet-verifier-jev.md", "F12", ""): ("WRONG", "true restatement: score_bundle.py reads retro_full125x3.json; 7/62 is its line"),
    ("tet-verifier-jev.md", "F14", ""): ("WRONG", "ask_chunked makes one call under 40 units; 'A/B' misreads the note"),
    ("tet-verifier-jev.md", "F15", ""): ("WRONG", "the regex's blank-line alternative is top-level; 'no labels' is not the header"),
    ("tet-verifier-jev.md", "F16", ""): ("WRONG", "216 and 50 are in the stored output the checker saw truncated"),
    ("tet-verifier-jev.md", "F18", ""): ("WRONG", "every uncaptured count is true in the named files"),
    ("tet-verifier-jev.md", "F20", ""): ("CORRECT", "the captured listing has 35 files summing to 306, not 34 and 331; ten test call sites, not eight"),
    ("tet-verifier-jev.md", "F21", ""): ("WRONG", "-I skips binaries, not an extension or directory restriction"),
    ("tet-verifier-jev.md", "F22", ""): ("WRONG", "true restatements of files the capture does not open"),
    ("tet-verifier-jev.md", "F23", ""): ("WRONG", "two flags for one entry is still one exclusion per entry"),
    ("tet-verifier-jev.md", "F24", ""): ("WRONG", "points to an earlier observation; claims nothing current"),
    ("tet-verifier-jev.md", "F26", ""): ("WRONG", "'whole-worktree' is tetel's defined census with its standard exclusions"),
    ("tet-verifier-jev.md", "F27", ""): ("WRONG", "the note itself says the keys are witnessed by the live call, not this capture"),
    ("tet-verifier-mint-warning.md", "F9", ""): ("CORRECT", "three allowlist entries carry no comment, against 'each with a comment justifying its inclusion'"),
    ("tet-verifier-mint-warning.md", "F10", ""): ("WRONG", "'one authoring tool call each' describes the append-only log; the capture fits it"),
    ("tet-verifier-mint-warning.md", "F13", ""): ("WRONG", "every row carries exactly one verdict; the count is right"),
    ("tet-verifier-mint-warning.md", "F15", ""): ("WRONG", "uncaptured but true: mcp.rs defines fact_result for `fact` only"),
    ("tet-verifier-mint-warning.md", "F16", ""): ("WRONG", "urlopen raises on non-2xx; the four states are the design's proposal"),
    ("tet-verifier-mint-warning.md", "F18", ""): ("CORRECT", "minor: 'the body is four lines' where the captured body is five"),
    ("tet-verifier-mint-warning.md", "F20", ""): ("WRONG", "uncaptured but true: README carries 32 / 33 and 12 / 12"),
    ("tet-verifier-mint-warning.md", "F21", ""): ("WRONG", "the 10% line is a proposed gate; every computed figure matches"),
    ("tet-verifier-mint-warning.md", "F22", ""): ("CORRECT", "minor: 'five with no justification' where transplants.jsonl's comment covers one, leaving four"),
    ("tet-verifier-mint-warning.md", "F30", ""): ("WRONG", "uncaptured but true: retrodict.py records an err per errored draw"),
    ("tet-verifier-mint-warning.md", "F31", ""): ("WRONG", "375 is in the F30 and F34 captures"),
    ("tet-verifier-mint-warning.md", "F32", ""): ("WRONG", "one-call / three-call are labels the README confirms"),
    ("tet-verifier-mint-warning.md", "F33", ""): ("WRONG", "the note's own organisation; retrodict.py imports direct_eval's SYSTEM"),
    ("tet-verifier-mint-warning.md", "F34", ""): ("WRONG", "the scope arms make one request per draw; figures match"),
    ("tet-verifier-mint-warning.md", "F35", ""): ("WRONG", "all four arms cover 125 claims"),
}


def load(name):
    last = {}
    for l in open(RUN / f"{name}.jsonl"):
        v = json.loads(l)
        last[(v["memo"], v["id"], v["draw"])] = v
    subj = collections.defaultdict(list)
    for (m, i, _), v in last.items():
        subj[(m, i)].append(v)
    return subj


def answered(vs):
    return [v for v in vs if v["record"]["status"] in ("ok", "gated")]


def flagged(v):
    r = v["record"]
    return r["status"] == "ok" and bool(r.get("findings"))


def flagged_by_check(v):
    # The candidate with its literal leg's findings set aside: `unevidenced`
    # is the only kind that leg emits and the check never does, so this is
    # `typed_model` on with `literals` off, less the literal calls' spend.
    r = v["record"]
    return r["status"] == "ok" and any(f.get("kind") != "unevidenced" for f in r.get("findings") or [])


def majority(vs, pred):
    a = answered(vs)
    return bool(a) and sum(pred(v) for v in a) * 2 > len(a)


def findings(vs, check_only=False):
    out = {}
    for v in vs:
        if flagged(v):
            for f in v["record"]["findings"]:
                if not (check_only and f.get("kind") == "unevidenced"):
                    out.setdefault((f.get("kind"), f.get("clause")), f)
    return list(out.values())


def hand(table, memo, id_, fs):
    vs = []
    for f in fs:
        cl = f.get("clause") or ""
        vs.append(next((v[0] for (m, i, p), v in table.items()
                        if (m, i) == (memo, id_) and cl.startswith(p)), "READ"))
    return vs


def grade_claim(key, vs, check_only=False):
    fs = findings(vs, check_only)
    memo, id_ = key
    if vs[0]["sample"] == "in":
        v = [h if g == "READ" else g
             for g, h in zip(SCA.grade((memo[:5], id_), fs), hand(IN_CLAIM, memo, id_, fs))]
    else:
        v = hand(OUT_CLAIM, memo, id_, fs)
    return "CORRECT" if "CORRECT" in v else "WRONG" if v and all(x == "WRONG" for x in v) else "READ"


def spend(subj):
    rs = [v["record"] for vs in subj.values() for v in vs]
    return sum(r.get("cost") or 0 for r in rs), len(rs)


def pct(k, n):
    return f"{k}/{n} {100 * k / n:4.1f}%" if n else "-"


def claims():
    loaded = {a: load(f"claim_{a}") for a in ("candidate", "default")}
    rows = {"candidate": (loaded["candidate"], flagged),
            "cand-lit": (loaded["candidate"], flagged_by_check),
            "default": (loaded["default"], flagged)}
    graded = {}
    print("CLAIM — flagged by majority of three draws\n")
    print(f"  {'arm':<10} {'sample':<7} {'claims':>6} {'errors':>6} {'flagged':>7} {'correct':>7} "
          f"{'wrong':>5} {'unread':>6} {'sound flagged':>16} {'refuted flagged':>15} {'gated draws':>12}")
    for name, (subj, pred) in rows.items():
        for sample in ("in", "out", "all"):
            ks = [k for k, vs in subj.items() if sample in ("all", vs[0]["sample"])]
            err = sum(len(subj[k]) - len(answered(subj[k])) for k in ks)
            fl = [k for k in ks if majority(subj[k], pred)]
            # cand-lit is graded on the findings it was flagged on: with the
            # literal leg's put back, a WRONG check flag could pass as CORRECT.
            grades = {k: grade_claim(k, subj[k], check_only=pred is flagged_by_check) for k in fl}
            g = collections.Counter(grades.values())
            graded.update({(name, k): v for k, v in grades.items()})
            sound = [k for k in ks if subj[k][0]["supports_only"]]
            sf = sum(k in fl for k in sound)
            rf = sum(1 for k in fl if subj[k][0]["refuted"])
            nref = sum(1 for k in ks if subj[k][0]["refuted"])
            gd = sum(v["record"]["status"] == "gated" for k in ks for v in subj[k])
            nd = sum(len(answered(subj[k])) for k in ks)
            print(f"  {name:<10} {sample:<7} {len(ks):>6} {err:>6} {len(fl):>7} {g['CORRECT']:>7} "
                  f"{g['WRONG']:>5} {g['READ']:>6} {pct(sf, len(sound)):>16} {rf:>7}/{nref:<7} "
                  f"{pct(gd, nd):>12}")
        if name == "cand-lit":
            continue
        cost, n = spend(subj)
        vers = collections.Counter(x for vs in subj.values() for v in vs
                                   for x in v["record"].get("typed_versions") or [])
        print(f"  {'':<10} ${cost:.3f} over {n} draws (${cost / max(n, 1):.4f}/draw)"
              f"{'; Jev ' + ', '.join(f'{k}×{c}' for k, c in vers.items()) if vers else ''}")
    print(f"\n  line: sound claims flagged under {LINE:.1%}")

    cand, dflt = loaded["candidate"], loaded["default"]
    print("\n  Correct warnings the default arm raises and the candidate does not"
          " (not flagged, or flagged only on a wrong clause):")
    lost = 0
    for k, vs in sorted(dflt.items()):
        if (graded.get(("default", k)) == "CORRECT"
                and graded.get(("candidate", k)) != "CORRECT"):
            lost += 1
            gated = sum(v["record"]["status"] == "gated" for v in cand[k])
            print(f"    {k[0]} {k[1]}  candidate: {gated}/{len(cand[k])} draws gated,"
                  f" {sum(map(flagged, cand[k]))} flagged, graded {graded.get(('candidate', k), 'not flagged')}")
    print(f"    {lost} lost")
    # The flip argued for is typed_model without literals, so compare that
    # reading claim by claim too: its net figure hides what it trades.
    for verb, want, other in (("loses", "default", "cand-lit"), ("gains", "cand-lit", "default")):
        ks = sorted(k for k in dflt if graded.get((want, k)) == "CORRECT" and graded.get((other, k)) != "CORRECT")
        print(f"  cand-lit {verb} {len(ks)}: " + ", ".join(f"{k[0][:-3]} {k[1]}" for k in ks))

    print(f"\n  The {len(SCA.GOOD)} correct warnings the claim gate was fitted to keep, still raised:")
    for name, (subj, _) in rows.items():
        kept = [k for k in subj if (k[0][:5], k[1]) in SCA.GOOD and graded.get((name, k)) == "CORRECT"]
        print(f"    {name:<10} {len(kept)} of {len(SCA.GOOD)}")
    return loaded, graded


def facts():
    arms = {a: load(f"fact_{a}") for a in ("gated", "ungated")}
    print("\nFACT — flagged by majority of three draws\n")
    print(f"  {'arm':<8} {'sample':<7} {'facts':>6} {'errors':>6} {'flagged':>7} {'gated draws':>12} {'cost':>8}")
    for name, subj in arms.items():
        for sample in ("in", "out", "all"):
            ks = [k for k, vs in subj.items() if sample in ("all", vs[0]["sample"])]
            err = sum(len(subj[k]) - len(answered(subj[k])) for k in ks)
            fl = sum(majority(subj[k], flagged) for k in ks)
            gd = sum(v["record"]["status"] == "gated" for k in ks for v in subj[k])
            nd = sum(len(answered(subj[k])) for k in ks)
            cost = sum(v["record"].get("cost") or 0 for k in ks for v in subj[k])
            print(f"  {name:<8} {sample:<7} {len(ks):>6} {err:>6} {fl:>7} {pct(gd, nd):>12} ${cost:>7.3f}")

    g, u = arms["gated"], arms["ungated"]
    held = {(m, i) for (m, i, _), (verdict, _) in OUT_FACT.items() if verdict == "CORRECT"}
    print(f"\n  Positives the gate skipped (any draw gated):")
    for label, keys in (("fitted, defects_v1", [k for k in g if (k[0][:5], k[1]) in FIT_DEFECTS]),
                        ("held out, OUT_FACT", sorted(held))):
        miss = [(k, sum(v["record"]["status"] == "gated" for v in g[k])) for k in keys if k in g]
        miss = [(k, n) for k, n in miss if n]
        print(f"    {label}: {len(miss)} of {len(keys)}"
              + "".join(f"\n      {k[0]} {k[1]} gated {n}/{len(g[k])}" for k, n in miss))

    read = {k for k, vs in u.items() if vs[0]["sample"] == "out" and any(map(flagged, vs))}
    minor = {k for (m, i, _), (v, why) in OUT_FACT.items()
             if v == "CORRECT" and why.startswith("minor") for k in [(m, i)]}
    print(f"\n  Held out: {len(read)} facts with a finding in some ungated draw, {len(held)} with a"
          f" CORRECT one, {len(held & minor)} of them minor. Raised by majority:")
    for label, keys in (("all held-out positives", held), ("not minor", held - minor),
                        ("fitted, defects_v1", {k for k in g if (k[0][:5], k[1]) in FIT_DEFECTS})):
        print(f"    {label:<24} gated arm {sum(majority(g[k], flagged) for k in keys)},"
              f" ungated arm {sum(majority(u[k], flagged) for k in keys)}, of {len(keys)}")

    # Why the saving is below the skip rate: what the gate skips is small.
    size = {(v["memo"], v["id"]): len(v["text"]) + sum(len(o) for _, _, outs in v["evidence"] for o in outs)
            for v in map(json.loads, open(RUN / "subjects_fact.jsonl"))}
    by = {st: [(k, v) for k, vs in g.items() for v in vs if v["record"]["status"] == st] for st in ("gated", "ok")}
    for st, rows in by.items():
        sizes = sorted(size[k] for k, _ in rows)
        cost_u = sum(w["record"].get("cost") or 0 for k, v in rows for w in u[k] if w["draw"] == v["draw"])
        print(f"    draws the gate {'skipped' if st == 'gated' else 'passed':<7}: median subject {sizes[len(sizes) // 2]:,}"
              f" characters; the same draws cost ${cost_u:.2f} ungated")
    return arms


# Each arm's budget had `verify.timeout_ms` been left unset, for the
# configuration as run (refuter off, so not docs/verify.md's 240 s), from
# `verify::default_budget_ms`: 60 s an LLM call, 10 s a TypeSafe call, counted
# by `expected_calls`. The run itself used 300 s everywhere.
#   candidate  check (LLM) + gate, classify, literals (Jev)   60 + 3 x 10
#   default    check + classify (LLM)                          2 x 60
#   fact rows  check + split + literals (LLM), gate if set     3 x 60 (+ 10)
DEFAULT_BUDGET_MS = {"claim_candidate": 90_000, "claim_default": 120_000,
                     "fact_gated": 190_000, "fact_ungated": 180_000}


def budgets():
    """What the default budget would have cost: an `ok` draw slower than it
    becomes an error. Latency was measured at 20-40 concurrent requests, so
    this is an estimate of the direction, not a replay."""
    print("\nBUDGET — had `verify.timeout_ms` been left at its default (refuter off, as run)\n")
    for name, b in DEFAULT_BUDGET_MS.items():
        subj = load(name)
        ok = [v["record"] for vs in subj.values() for v in vs if v["record"]["status"] == "ok"]
        el = sorted(r.get("elapsed_ms") or 0 for r in ok)
        over = [r for r in ok if (r.get("elapsed_ms") or 0) > b]
        within = [r for r in ok if (r.get("elapsed_ms") or 0) <= b]
        rate = lambda rs: sum(bool(r.get("findings")) for r in rs) / max(len(rs), 1)
        print(f"  {name:<16} default {b // 1000}s: {len(over)} of {len(ok)} draws that ran the check over it"
              f" ({len(over) / len(ok):.0%}), {sum(bool(r.get('findings')) for r in over)} of them with findings"
              f" ({rate(over):.0%}, against {rate(within):.0%} within); median {el[len(el) // 2] / 1e3:.0f}s")
    for name in ("candidate", "default"):
        subj, b = load(f"claim_{name}"), DEFAULT_BUDGET_MS[f"claim_{name}"]
        for cut in (None, b):
            def kept(v):
                r = v["record"]
                return r["status"] == "gated" or (r["status"] == "ok" and not (cut and (r.get("elapsed_ms") or 0) > cut))
            sound = sf = good = lost = 0
            for k, vs in subj.items():
                a = [v for v in vs if kept(v)]
                if not a:
                    lost += 1
                    continue
                f = majority(a, flagged)
                if vs[0]["supports_only"]:
                    sound += 1
                    sf += f
                good += f and grade_claim(k, a) == "CORRECT"
            print(f"  {name:<10} at {'300s' if cut is None else f'{cut // 1000}s':>5}: correct warnings {good:>2},"
                  f" sound flagged {pct(sf, sound)}, {lost} claims with every draw timed out")


def to_read(claim_arms, fact_arms):
    print("\nTO READ — findings no adjudication covers\n")
    seen = set()
    for name, subj in claim_arms.items():
        for k, vs in sorted(subj.items()):
            if k in seen or not majority(vs, flagged) or grade_claim(k, vs) != "READ":
                continue
            seen.add(k)
            print(f"  claim {k[0]} {k[1]} [{vs[0]['sample']}, first flagged in {name}]  {vs[0]['prop'][:300]}")
            for f in findings(vs):
                print(f"      {f.get('kind')}: «{(f.get('clause') or '')[:160]}» — {(f.get('why') or '')[:300]}")
    for k, vs in sorted(fact_arms["ungated"].items()):
        if vs[0]["sample"] != "out":
            continue
        fs = findings(vs)
        if not fs or all(x != "READ" for x in hand(OUT_FACT, k[0], k[1], fs)):
            continue
        n = sum(map(flagged, vs))
        print(f"  fact {k[0]} {k[1]} [flagged {n}/{len(vs)}]")
        for f in fs:
            print(f"      {f.get('kind')}: «{(f.get('clause') or '')[:160]}» — {(f.get('why') or '')[:300]}")


if __name__ == "__main__":
    ca, _ = claims()
    fa = facts()
    budgets()
    # Every line, not the last per draw: a re-run draw was paid for twice.
    paid = sum(json.loads(l)["record"].get("cost") or 0 for a in
               ("claim_candidate", "claim_default", "fact_gated", "fact_ungated") for l in open(RUN / f"{a}.jsonl"))
    print(f"\nSPEND — every draw paid for, re-runs included: ${paid:.2f}")
    to_read(ca, fa)
