#!/usr/bin/env python3
"""The literal leg judged by what it adds: joined with the check leg and the claim gate.

docs/verify.md says to judge the literal leg by what it adds, not by what it
scores alone. This joins, on the literal eval's 88 claims:

  check     the shipped check leg's own flags — retro_full125x3.json, a claim
            flagged by majority of its three draws
  gate      the Jev claim gate (`pick_clause`, gate_runs/claim_pick_clause_d*.json,
            mean of three draws), skipping a claim below the threshold set 0.05
            under the lowest-scoring adjudicated-correct warning
            (labels_claim_v1.json), exactly as score_gate_variants.py does
  literals  a claim flagged by a literal raised in a majority of its draws, for
            the LLM leg (literals_final_88x3.json) and Jev's
            (literals_runs/jev_x3.json at P(quantity) >= 0.7, P(carried) < 0.5)

and reports, per combination, the claims that later needed work (anything but
supports-only) that are flagged, and the supports-only claims that are.

The README's table of these rows was first computed inline, off-page, and
listed a command that does not print it — this script is that table,
reproducible. Every input is a committed file; no network, no key.

    python3 score_bundle.py
"""

import collections, json, sys
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import literals_jev as LJ  # noqa: E402
import score_gate_variants as S  # noqa: E402

STEERING_LINE = 7 / 62   # retrodict.py's kill condition, as a rate


def majority_literal_flags(records):
    by = collections.defaultdict(list)
    for r in records:
        if r.get("status") == "ok":
            by[(r["memo"][:5], r["id"])].append(r)
    out = {}
    for k, rs in by.items():
        c = collections.Counter(x["literal"] for r in rs for x in r.get("kept", []))
        out[k] = (any(v >= len(rs) // 2 + 1 for v in c.values()), rs[0])
    return out


def main():
    check = S.llm_flagged()
    adj = json.load(open(HERE / "labels_claim_v1.json"))["labels"]
    _, scores, _, _ = S.load("claim", "pick_clause")
    mean = {k: sum(v) / len(v) for k, v in scores.items()}
    lowest = min(mean[(l["memo"], l["id"])] for l in adj if l["label"] == "CORRECT")
    threshold = lowest - S.MARGIN
    gated = {k for k, m in mean.items() if m < threshold}

    jev = majority_literal_flags(
        LJ.apply(json.load(open(HERE / "literals_runs/jev_x3.json"))["records"], 0.7, 0.5))
    llm = majority_literal_flags(json.load(open(HERE / "literals_final_88x3.json"))["records"])
    pop = [k for k in jev if jev[k][1].get("has_evidence", True)]
    worked = [k for k in pop if not jev[k][1]["supports_only"]]
    sound = [k for k in pop if jev[k][1]["supports_only"]]
    lit = lambda F, k: F.get(k, (False,))[0]  # noqa: E731

    rows = [
        ("check alone (ships)", lambda k: check.get(k)),
        ("check + LLM literals", lambda k: check.get(k) or lit(llm, k)),
        ("check + Jev literals", lambda k: check.get(k) or lit(jev, k)),
        ("Jev claim gate + check", lambda k: check.get(k) and k not in gated),
        ("gate + check + Jev literals (literals always run)",
         lambda k: (check.get(k) and k not in gated) or lit(jev, k)),
        ("gate + check + Jev literals (literals gated too)",
         lambda k: k not in gated and (check.get(k) or lit(jev, k))),
    ]
    print(f"{len(pop)} claims: {len(worked)} later needed work, {len(sound)} supports-only; "
          f"gate threshold {threshold:.2f} (lowest correct warning {lowest:.2f} - {S.MARGIN}), "
          f"{sum(1 for k in pop if k in gated)} of them gated")
    print(f"steering-hazard line: {STEERING_LINE:.1%} of supports-only claims flagged\n")
    print(f"  {'':<52} {'needed work flagged':>20} {'sound flagged':>16}")
    for name, flag in rows:
        w = sum(1 for k in worked if flag(k))
        s = sum(1 for k in sound if flag(k))
        over = "  OVER the line" if s / len(sound) > STEERING_LINE else ""
        print(f"  {name:<52} {w:>12} / {len(worked):<5} {s:>6} / {len(sound)} = {s / len(sound):5.1%}{over}")


if __name__ == "__main__":
    main()
