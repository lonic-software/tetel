#!/usr/bin/env python3
"""Score a refutation pass against the adjudicated labels, mechanically.

The README's Sonnet-vs-Gemini table was joined by hand. This does the same
join in code, so a new refuter — a different model, or a different model
*class* — is scored the way those two were rather than by a fresh reading.

Ground truth is `labels_fact_v1.json`, derived from
`fact_v1_contradicts_adjudicated.md`: the 16 `contradicts` findings of
`fact_v1.json`, 10 CORRECT and 6 WRONG, one adjudicator.

A refuter is graded on what it KEEPS:

  kept_true   of the 10 adjudicated-correct findings, how many survived.
              This is recall, and it is the number a warning costs.
  precision   of everything it kept, the share that is adjudicated-correct.

    python3 score_refuter.py fact_refuted.json fact_refuted_gemini.json

A probabilistic refuter (`--prob p_correct`) is also swept across thresholds,
because a calibrated probability makes the keep/drop line a parameter rather
than a property of the model. `--reliability` bins the probabilities against
outcomes: the bins are what say whether the threshold means anything.
"""

import argparse, json, sys
from collections import defaultdict
from data import DATA  # noqa: E402

LABELS = str(DATA / "labels_fact_v1.json")


def load_labels(path=LABELS):
    d = json.load(open(path))
    return {(l["memo"], l["id"], l["clause"]): l["label"] for l in d["labels"]}


def kept_keys(results_path):
    """Every contradicts finding surviving in a refuted results file."""
    d = json.load(open(results_path))
    out = {}
    for r in d["records"]:
        if r["status"] != "ok":
            continue
        for f in r["findings"]:
            # No kind filter: the adjudicated label set defines the population,
            # and it differs per corpus — the fact adjudication covers only
            # `contradicts`, the prose one covers all 44 findings of both kinds.
            # Filtering by kind here silently dropped a true `overreaches` catch
            # and reported it as lost by the refuter.
            out[(r["memo"], r["id"], f["clause"])] = f.get("refutation", {})
    return d, out


# A refuter keeps everything but WRONG. `refute.py` enforced that by deleting
# the dropped findings, which also deletes what they scored — so a lossless
# results file has to be read by verdict instead of by presence. Both shapes
# are graded the same way here: a file that dropped its WRONGs has none left
# to exclude, so the two rules agree on it.
KEEP_VERDICTS = {"CORRECT", "UNCLEAR", "ERROR"}


def survivors(kept):
    return {k: v for k, v in kept.items()
            if not v or v.get("verdict") in KEEP_VERDICTS}


def score(labels, kept, label_name):
    truth_true = {k for k, v in labels.items() if v == "CORRECT"}
    unknown = [k for k in kept if k not in labels]
    kept_known = {k for k in kept if k in labels}
    kept_true = kept_known & truth_true
    prec = len(kept_true) / len(kept_known) if kept_known else float("nan")
    print(f"{label_name}")
    print(f"  kept          {len(kept_known)} of {len(labels)} adjudicated findings")
    print(f"  kept_true     {len(kept_true)} / {len(truth_true)}   (true catches retained)")
    print(f"  precision     {prec:.0%}   ({len(kept_true)}/{len(kept_known)})")
    if unknown:
        print(f"  ignored       {len(unknown)} kept findings outside the adjudicated set")
    lost = truth_true - kept_true
    for k in sorted(lost):
        print(f"  lost catch    {k[0].split('-')[0]} {k[1]}  {k[2][:56]!r}")
    return {"kept": len(kept_known), "kept_true": len(kept_true), "precision": prec}


def sweep(labels, kept, prob_field):
    """A calibrated refuter's keep/drop line is a parameter. Show the curve."""
    truth_true = {k for k, v in labels.items() if v == "CORRECT"}
    rows = [(k, kept[k].get(prob_field)) for k in kept if k in labels]
    if any(p is None for _, p in rows):
        print("  (no probability on every finding — skipping sweep)")
        return
    print(f"\n  threshold   kept  kept_true  precision   LLM calls saved")
    for t in [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]:
        keep = {k for k, p in rows if p >= t}
        kt = keep & truth_true
        prec = len(kt) / len(keep) if keep else float("nan")
        print(f"  p >= {t:.1f}    {len(keep):4}  {len(kt):9}  {prec:9.0%}   "
              f"{len(rows) - len(keep):4} of {len(rows)}")


def reliability(labels, kept, prob_field, bins=5):
    """Does p mean what it says? Bin it against the adjudicated outcome."""
    rows = [(kept[k].get(prob_field), labels[k] == "CORRECT") for k in kept if k in labels]
    rows = [(p, t) for p, t in rows if p is not None]
    if not rows:
        return
    print(f"\n  reliability (n={len(rows)}) — a calibrated p tracks the observed rate")
    buckets = defaultdict(list)
    for p, t in rows:
        buckets[min(int(p * bins), bins - 1)].append(t)
    print(f"  {'bin':<12} {'n':>3}  {'mean p':>7}  {'observed':>9}")
    for b in range(bins):
        v = buckets.get(b, [])
        if not v:
            continue
        ps = [p for p, _ in rows if min(int(p * bins), bins - 1) == b]
        print(f"  {b/bins:.1f}-{(b+1)/bins:.1f}      {len(v):3}  {sum(ps)/len(ps):7.2f}  "
              f"{sum(v)/len(v):9.0%}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results", nargs="+")
    ap.add_argument("--labels", default=LABELS)
    ap.add_argument("--prob", default=None,
                    help="field inside `refutation` holding P(CORRECT), e.g. p_correct")
    a = ap.parse_args()

    labels = load_labels(a.labels)
    print(f"ground truth: {len(labels)} adjudicated findings, "
          f"{sum(1 for v in labels.values() if v == 'CORRECT')} CORRECT\n")
    for path in a.results:
        d, kept = kept_keys(path)
        name = f"{path}  (refuter: {d.get('refutation', {}).get('model', 'see file')})"
        score(labels, survivors(kept), name)
        if a.prob:
            # The sweep runs over EVERY finding scored, not just the ones the
            # verdict kept — a threshold applied to the survivors of another
            # threshold measures the first one.
            sweep(labels, kept, a.prob)
            reliability(labels, kept, a.prob)
        print()


if __name__ == "__main__":
    main()
