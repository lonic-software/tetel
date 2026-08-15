#!/usr/bin/env python3
"""Compare prompt variants on the claims the shipped prompt already gets wrong.

`screen_cases.json` holds three buckets, each labelled with what the shipped
prompt did on it in `literals_full125x3.json`:

  false-positive   flagged, and a later pass only ever supported the claim.
                   A variant should stop flagging these.
  refuted-missed   a later pass refuted the claim and nothing was flagged.
                   A variant that catches any of these gains real recall.
  control-silent   correctly silent. A variant that starts flagging these has
                   not become more precise, it has moved its noise.

This is a screening tool and its numbers are not accuracy. The population was
chosen *because* the shipped prompt fails on two thirds of it, so every rate
here is against a stacked deck — which is what makes it cheap and sensitive,
and what makes quoting it as a result dishonest. A variant that looks good
here earns a run on the whole corpus, nothing more.

    python3 screen_compare.py baseline.json v1.json v2.json
"""

import json, sys
from collections import defaultdict


def flagged_by_claim(path):
    d = json.load(open(path))
    by = defaultdict(list)
    for r in d["records"]:
        if r["status"] == "ok":
            by[(r["memo"], r["id"])].append(r)
    out = {}
    for k, rs in by.items():
        need = len(rs) // 2 + 1
        tally = defaultdict(int)
        for r in rs:
            for f in {f["literal"]: f for f in r["kept"]}.values():
                tally[f["literal"]] += 1
        out[k] = sorted(l for l, n in tally.items() if n >= need)
    return d, out


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    cases = json.load(open("screen_cases.json"))
    buckets = defaultdict(list)
    for c in cases:
        buckets[c["bucket"]].append((c["memo"], c["id"]))

    runs = []
    for path in sys.argv[1:]:
        d, flags = flagged_by_claim(path)
        runs.append((path, d, flags))

    print(f"{'':34}" + "".join(f"{p.replace('.json','')[:16]:>18}" for p, _, _ in runs))
    order = [("false-positive", "should go to 0"),
             ("refuted-missed", "any catch is a gain"),
             ("control-silent", "must stay 0")]
    for bucket, aim in order:
        ks = buckets[bucket]
        cells = []
        for _, _, flags in runs:
            n = sum(1 for k in ks if flags.get(k))
            cells.append(f"{n}/{len(ks)}")
        print(f"  {bucket:<20} {aim:<11}" + "".join(f"{c:>18}" for c in cells))

    tot = []
    for path, d, flags in runs:
        cost = sum(r.get("cost", 0.0) for r in d["records"])
        tot.append(f"${cost:.3f}")
    print(f"\n  {'cost':<32}" + "".join(f"{c:>18}" for c in tot))

    # What changed, claim by claim — the part that says whether a variant got
    # the right answer for the right reason.
    base = runs[0][2]
    for path, _, flags in runs[1:]:
        print(f"\n--- {path} against {sys.argv[1]} ---")
        for bucket, _ in order:
            for k in buckets[bucket]:
                a, b = base.get(k, []), flags.get(k, [])
                if a != b:
                    print(f"  [{bucket}] {k[1]:5} {k[0][:30]:32} {a} -> {b}")


if __name__ == "__main__":
    main()
