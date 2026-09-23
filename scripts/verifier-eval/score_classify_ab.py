#!/usr/bin/env python3
"""Settle classify: the check leg fed the LLM's labels against the same check fed Jev's.

Both arms are `retrodict.py --question split --arm union --repeat 3` over the
same 125 claims, run the same day on the corrected reconstruction, differing
only in `--classifier`. A claim is flagged by majority of its three draws, the
retrodiction's rule.

Each flagged claim's findings are matched against the claim adjudication:

  CORRECT  a finding lands on the clause of an adjudicated-correct warning
           (classify_jev.DECISIVE, `must`) — the warning the author should get
  WRONG    every finding lands on a clause already adjudicated WRONG on that
           claim (labels_claim_v1.json)
  READ     anything else: a claim or a clause no adjudication has graded. Printed
           in full, to be graded by hand and added to ADJUDICATED below —
           never counted by assumption.

    python3 score_classify_ab.py retro_classify_llm_x3.json retro_classify_jev_x3.json
"""

import collections, json, sys
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import classify_jev as CJ  # noqa: E402
from data import DATA  # noqa: E402

LABELS = {(l["memo"], l["id"]): l for l in
          json.load(open(DATA / "labels_claim_v1.json"))["labels"]}
GOOD = {(m, i): c for m, i, c, w in CJ.DECISIVE if w == "must"}

# Findings on claims or clauses the first adjudication never saw, graded by
# hand after reading them against the evidence: (memo, id, clause prefix) ->
# (CORRECT|WRONG, why). Filled in from the READ list this script prints.
ADJUDICATED = {
    # 2026-09-18, the classify A/B's 18 unread claims. Same standard as
    # claim_flagged_adjudicated.md; grounding notes read as a second opinion.
    ("tet30", "C13", "The measured weight"): ("CORRECT", "405019 characters exceeds the capture's 373271 bytes — impossible in UTF-8; a grader flagged it and the author revised to 373271"),
    ("tet30", "C13", "`render` 486 matching lines"): ("CORRECT", "same figure"),
    ("tet30", "C13", "`NON_COVERAGE` 35 lines"): ("CORRECT", "98828 characters against 95637 bytes, same impossibility"),
    ("tet30", "C13", "`Findings` 23 lines"): ("CORRECT", "59.2 KB against 57418 bytes"),
    ("tet30", "C13", "`check_file` 5 lines"): ("WRONG", "5 lines / 285 bytes reproduce exactly; the finding compared a different measurement"),
    ("tet30", "C13", "and measured a cost"): ("WRONG", "insufficiency"),
    ("tet30", "C13", "every memo added"): ("WRONG", "insufficiency"),
    ("tet56", "C17", "which is how `look --lines`"): ("CORRECT", "format! labels every range; prose P21 graded CORRECT on the same sentence and code"),
    ("tet56", "C17", "`look --lines` already annotates"): ("CORRECT", "same"),
    ("tet28", "C5", ""): ("WRONG", "insufficiency: one module's CLI surface against 'no surface'"),
    ("tet29", "C3", ""): ("WRONG", "insufficiency, same shape"),
    ("tet46", "C17", ""): ("WRONG", "proposal read as current: the exclusions are what the design adds"),
    ("tet56", "C15", ""): ("WRONG", "insufficiency: a disclosed exclusion"),
    ("tet61", "C8", ""): ("WRONG", "insufficiency: one event constructor against 'every workspace event'"),
    ("tet61", "C14", ""): ("WRONG", "proposal read as current: today's parser rejecting the Ack the design adds"),
    ("tet28", "C11", ""): ("WRONG", "proposal: the predicate the design specifies names both exclusions; graders support"),
    ("tet29", "C13", ""): ("WRONG", "misread referent: the refusal set of a different verb"),
    ("tet46", "C5", ""): ("WRONG", "insufficiency: 'every reader'"),
    ("tet46", "C9", ""): ("WRONG", "insufficiency: 'whichever symbol'; graders qualified on drifted numbers, not this"),
    ("tet47", "C11", ""): ("WRONG", "the inputs are derived from the path; an absent snapshot is a derived absence"),
    ("tet47", "C24", ""): ("WRONG", "scope fixed by its own sentence ('and its snapshot'); the owed definition is the design's"),
}


def _seen():
    """Every clause the August run raised on an adjudicated claim.

    The claim adjudication graded all of them: on a WRONG claim every finding
    is wrong, and on a CORRECT claim every finding but the one on the good
    clause is (tet30 C12's second and fourth, for instance).
    """
    seen = collections.defaultdict(set)
    for r in json.load(open(DATA / "retro_full125x3.json")):
        k = (r["memo"][:5], r["id"])
        if k in LABELS:
            for f in r.get("findings") or []:
                seen[k].add(f.get("clause") or "")
    return seen


SEEN = _seen()


def overlaps(a, b):
    a, b = a.strip().rstrip("."), b.strip().rstrip(".")
    return bool(a) and bool(b) and (a in b or b in a)


def grade(key, findings):
    lab = LABELS.get(key)
    good = GOOD.get(key)
    verdicts = []
    for f in findings:
        cl = f.get("clause") or ""
        hand = next((v for (m, i, p), v in ADJUDICATED.items()
                     if (m, i) == key and cl.startswith(p)), None)
        if hand:
            verdicts.append(hand[0])
        elif good and overlaps(cl, good):
            verdicts.append("CORRECT")
        elif lab and any(overlaps(cl, x) for x in SEEN.get(key, ())):
            verdicts.append("WRONG")       # a clause the claim adjudication already graded
        else:
            verdicts.append("READ")
    return verdicts


def arm(path):
    rows = json.load(open(path))
    by = collections.defaultdict(list)
    for r in rows:
        by[(r["memo"][:5], r["id"])].append(r)
    errors = sum(1 for r in rows if r.get("err"))
    out = {}
    for k, rs in by.items():
        if sum(bool(r["flagged"]) for r in rs) * 2 <= len(rs):
            continue
        fs = {}
        for r in rs:
            if r["flagged"]:
                for f in r.get("findings") or []:
                    fs.setdefault((f.get("kind"), f.get("clause")), f)
        out[k] = (rs[0], list(fs.values()))
    cost = sum(r.get("cost") or 0 for r in rows)
    return out, errors, cost, len(by), rows


def main():
    arms = {Path(p).stem: arm(p) for p in sys.argv[1:]}
    to_read = []
    print(f"  {'arm':<26} {'flagged':>7} {'correct':>8} {'wrong':>6} {'unread':>7} "
          f"{'sound flagged':>14} {'refuted flagged':>16} {'errors':>7} {'cost':>8}")
    detail = {}
    for name, (flag, errors, cost, n, rows) in arms.items():
        c = w = u = 0
        per = {}
        for k, (r0, fs) in flag.items():
            v = grade(k, fs)
            if "CORRECT" in v:
                c += 1; per[k] = "CORRECT"
            elif v and all(x == "WRONG" for x in v):
                w += 1; per[k] = "WRONG"
            else:
                u += 1; per[k] = "READ"
                to_read.append((name, k, r0, fs, v))
        so = sum(1 for k, (r0, _) in flag.items() if r0["supports_only"])
        rf = sum(1 for k, (r0, _) in flag.items() if r0["refuted"])
        detail[name] = per
        print(f"  {name:<26} {len(flag):>7} {c:>8} {w:>6} {u:>7} {so:>14} {rf:>16} "
              f"{errors:>7} ${cost:>7.3f}")
    names = list(detail)
    if len(names) == 2:
        a, b = names
        good_a = {k for k, v in detail[a].items() if v == "CORRECT"}
        good_b = {k for k, v in detail[b].items() if v == "CORRECT"}
        print(f"\n  correct warnings in {a} only: {sorted(good_a - good_b)}")
        print(f"  correct warnings in {b} only: {sorted(good_b - good_a)}")
        print(f"  flagged in {a} only: {sorted(set(detail[a]) - set(detail[b]))}")
        print(f"  flagged in {b} only: {sorted(set(detail[b]) - set(detail[a]))}")
    if to_read:
        print(f"\n==== {len(to_read)} flagged claims need reading")
        for name, k, r0, fs, v in to_read:
            print(f"\n--- [{name}] {k[0]} {k[1]}  prior label: "
                  f"{LABELS.get(k, {}).get('label', 'none')}  later: "
                  f"{'refuted' if r0['refuted'] else 'supports-only' if r0['supports_only'] else 'qualified'}")
            print(f"CLAIM: {r0['prop']}")
            for f, g in zip(fs, v):
                print(f"  [{g}] [{f.get('kind')}] CLAUSE: {f.get('clause')}\n"
                      f"        WHY: {f.get('why')}\n        QUOTE ok={f.get('quote_verified')}: "
                      f"{(f.get('evidence') or '')[:300]}")


if __name__ == "__main__":
    main()
