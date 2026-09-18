#!/usr/bin/env python3
"""What a gate before the check leg would have cost and saved.

Each subject falls in one of four buckets, decided by what the LLM found for it
and how that was adjudicated:

  must_keep      carries at least one adjudicated-CORRECT finding. Skipping one
                 of these loses a true warning. This is the only bucket where a
                 skip is a loss.
  noise          carries findings, every one adjudicated WRONG. Skipping is a
                 GAIN: it removes a warning that should never have printed.
  unadjudicated  carries findings none of which the adjudication covers.
                 Reported separately rather than assumed either way.
  silent         the LLM found nothing. Skipping is pure saving.

A skip rule is `p_any < t`. The sweep reports what each t costs and buys.

ONE LIMIT TO READ THIS THROUGH. The adjudications cover only findings that were
RAISED, so a subject the LLM was silent on was never graded. This measures the
gate against "would the LLM have found it", never against "was something there".
A disagreement both models miss is invisible to this, exactly as it is to the
retrodiction on this page.

    python3 score_gate.py gate_fact1.json gate_fact2.json gate_fact3.json \
        --labels labels_fact_v1.json
"""

import argparse, json, collections


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("draws", nargs="+")
    ap.add_argument("--labels", required=True)
    ap.add_argument("--field", default="any")
    a = ap.parse_args()

    labels = {(l["memo"], l["id"], l["clause"]): l["label"]
              for l in json.load(open(a.labels))["labels"]}

    ps, meta = collections.defaultdict(list), {}
    for path in a.draws:
        for row in json.load(open(path))["rows"]:
            k = (row["memo"], row["id"])
            v = row["gate"].get(a.field)
            if v is not None:
                ps[k].append(v)
            meta[k] = row["llm_findings"]

    bucket, spread = {}, []
    for k, fs in meta.items():
        graded = [labels.get((k[0], k[1], f["clause"])) for f in fs]
        graded = [g for g in graded if g]
        if not fs:
            bucket[k] = "silent"
        elif "CORRECT" in graded:
            bucket[k] = "must_keep"
        elif graded:
            bucket[k] = "noise"
        else:
            bucket[k] = "unadjudicated"
    for k, v in ps.items():
        spread.append(max(v) - min(v))

    counts = collections.Counter(bucket.values())
    n = len(meta)
    print(f"{n} subjects  ({a.draws[0].split('/')[-1]} …, {len(a.draws)} draws, "
          f"max p spread {max(spread):.2f})")
    for b in ("must_keep", "noise", "unadjudicated", "silent"):
        print(f"  {b:<14} {counts.get(b,0):4}")

    avg = {k: sum(v) / len(v) for k, v in ps.items()}
    keep_all = [k for k in meta if bucket[k] == "must_keep"]
    print(f"\n  p_any on the {len(keep_all)} must_keep subjects: "
          f"{', '.join(f'{avg[k]:.2f}' for k in sorted(keep_all, key=lambda k: -avg[k]))}")

    print(f"\n  skip if     skipped   of which        catches   noise")
    print(f"  p_any <     total     silent  noise     LOST      removed")
    for t in [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]:
        sk = [k for k in meta if avg.get(k, 1.0) < t]
        c = collections.Counter(bucket[k] for k in sk)
        lost = c.get("must_keep", 0)
        flag = "   <-- first loss" if lost and not any(
            bucket[k] == "must_keep" for k in meta if avg.get(k, 1.0) < t - 0.1) else ""
        print(f"  {t:.1f}        {len(sk):4} ({len(sk)/n:3.0%})  {c.get('silent',0):4}   "
              f"{c.get('noise',0):4}      {lost:4}      {c.get('noise',0)}{flag}")


if __name__ == "__main__":
    main()
