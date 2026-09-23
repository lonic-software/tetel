#!/usr/bin/env python3
"""The subjects TET-98's two runs verify, one JSON line each, for `examples/verify_corpus.rs`.

Python does only what the shipped code cannot: replay a claim log to a claim's
wording and cites *at its memo's first render* (`retrodict.load_memo`). The
overlap set, the evidence and its formatting are left to the driver, which
calls tetel's own `claims::overlap_for` and `verify::claim_subject` on the
committed snapshot. A fact's subject is built by `verify::fact_subject` from
the snapshot alone and needs nothing from here but its id: that reads a
revised fact's latest note, which is also the wording fact_v1.json was
measured on (all ten of its revised facts).

`sample` says which side of the fitted sets a subject is on:

  claim  in   the 125 claims every earlier claim-verb row was measured on
         out  graded first-render claims of the memos written since
              (tet42, tet-verifier-mint-warning, tet-verifier-jev)
  fact   in   the 123 facts of fact_v1.json, which the fact gate was fitted on
         out  every other fact created in the ten snapshots (126)

Claims of the seven old memos that are outside the 125 are left out: they
never had a check result, so they sit in neither population.

    python3 tet98_tasks.py --verb claim > claims.jsonl
    python3 tet98_tasks.py --verb fact  > facts.jsonl
"""

import argparse, json, os, sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from retrodict import CORPUS, load_memo  # noqa: E402

HERE = Path(__file__).parent
OLD_CLAIM_MEMOS = {c["memo"] for c in json.load(open(HERE / "claims125.json"))}


def memos():
    return sorted(p.name[: -len(".evidence.jsonl")] for p in Path(CORPUS).glob("*.evidence.jsonl"))


def claim_tasks():
    fitted = {(c["memo"], c["id"]) for c in json.load(open(HERE / "claims125.json"))}
    for memo in memos():
        for c in load_memo(memo):
            if (memo, c["id"]) in fitted:
                sample = "in"
            elif memo in OLD_CLAIM_MEMOS:
                continue
            else:
                sample = "out"
            # The defect `load_memo` was fixed for: a null carried forward as
            # text or as no evidence. Fail loudly rather than verify "None".
            assert isinstance(c["prop"], str) and c["prop"].strip(), (memo, c["id"], c["prop"])
            assert c["cites"], (memo, c["id"], "no cites at first render")
            yield dict(verb="claim", memo=memo, dir=os.path.join(CORPUS, memo + ".tetel"),
                       id=c["id"], prop=c["prop"], cites=c["cites"], sample=sample,
                       supports_only=c["supports_only"], refuted=c["refuted"],
                       qualified=c["qualified"])


def fact_tasks():
    fv = json.load(open(HERE / "fact_v1.json"))
    fitted = {(r["memo"], r["id"]) for r in fv["records"]}
    for memo in memos():
        snap = os.path.join(CORPUS, memo + ".tetel")
        for line in open(os.path.join(snap, "facts.jsonl")):
            d = json.loads(line)
            if d["event"] != "Create":
                continue
            yield dict(verb="fact", memo=memo, dir=snap, id=d["id"],
                       sample="in" if (memo, d["id"]) in fitted else "out")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--verb", choices=["claim", "fact"], required=True)
    a = ap.parse_args()
    tasks = list(claim_tasks() if a.verb == "claim" else fact_tasks())
    for t in tasks:
        print(json.dumps(t))
    n = Counter(t["sample"] for t in tasks)
    extra = ""
    if a.verb == "claim":
        sound = Counter(t["sample"] for t in tasks if t["supports_only"])
        extra = f", supports-only in={sound['in']} out={sound['out']}"
    print(f"{a.verb}: in={n['in']} out={n['out']}{extra}", file=sys.stderr)


if __name__ == "__main__":
    main()
