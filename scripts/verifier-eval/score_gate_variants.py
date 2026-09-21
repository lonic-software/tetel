#!/usr/bin/env python3
"""Score gate variants against the subject-level defects, all on one footing.

A subject is positive if `defects_v1.json` lists it: some adjudicated run found
a real defect there. Everything else is negative — including subjects an LLM
flagged wrongly, which a gate skipping is a gain, not a loss.

Per variant, over the mean score of its draws:

  AUC        positives vs negatives. 0.5 is a coin.
  spread     the largest max-min across draws for one subject: how much a
             single draw can move a subject.
  skip @0    the threshold just under the lowest positive, i.e. no defect lost.
             FITTED IN-SAMPLE to 12 fact / 5 prose points, so it flatters.
  skip @0-m  the same threshold lowered by a 0.05 margin — the number to
             believe more, because a new defect only has to score a little
             lower than the worst one seen to be lost at @0.
  net        the check leg's saving at @0-m, after paying the gate on every
             subject: skipped x check cost - all x gate cost, over all x check cost.

    python3 score_gate_variants.py fact whole split pick atoms
    python3 score_gate_variants.py claim whole pick --positives caught   # refuted|caught|flagged|adjudicated
"""

import collections, json, sys
from pathlib import Path

HERE = Path(__file__).parent
RUNS = HERE / "gate_runs"
MARGIN = 0.05
LABEL = {}   # claim rows carry their own label; filled by load()


def auc(pos, neg):
    if not pos or not neg:
        return float("nan")
    return sum((p > n) + 0.5 * (p == n) for p in pos for n in neg) / (len(pos) * len(neg))


# The claim verb has no `claim_v1.json`: its check leg was measured by
# retrodict over 125 claims x 3 draws of the full classify+check pipeline.
CLAIM_CHECK_COST = "retro_full125x3.json"


def check_cost(verb):
    if verb == "claim":
        rows = json.load(open(HERE / CLAIM_CHECK_COST))
        c = [r["cost"] for r in rows if r.get("cost")]
        return sum(c) / len(c)
    d = json.load(open(HERE / f"{verb}_v1.json"))
    return sum(r.get("cost", 0) for r in d["records"]) / len(d["records"])


def load(verb, variant):
    draws = sorted(RUNS.glob(f"{verb}_{variant}_d*.json"))
    scores, cost, errors = collections.defaultdict(list), 0.0, 0
    for p in draws:
        d = json.load(open(p))
        cost += d["cost"]
        for r in d["rows"]:
            LABEL[(r["memo"][:5], r["id"])] = r.get("label")
            s = r["gate"].get("score")
            if s is None:
                errors += 1
                continue
            scores[(r["memo"][:5], r["id"])].append(s)
    return draws, scores, cost, errors


def llm_flagged():
    """Claims the shipped check leg flags by majority of retro_full125x3's three draws."""
    votes = collections.defaultdict(list)
    for r in json.load(open(HERE / CLAIM_CHECK_COST)):
        votes[(r["memo"][:5], r["id"])].append(bool(r.get("flagged")))
    return {k: sum(v) >= 2 for k, v in votes.items()}


def main():
    args = sys.argv[1:]
    positives = "refuted"
    if "--positives" in args:
        i = args.index("--positives")
        positives = args[i + 1]
        del args[i:i + 2]
    verb, variants = args[0], args[1:]
    only, adj = None, []
    if verb == "claim":
        # Which claims a gate must not skip is itself a choice, so it is named:
        #   refuted  every claim a later grounding pass refuted (9).
        #   caught   refuted AND flagged by the check leg — the only refuted
        #            claims a gate can lose, since the check misses the rest
        #            with or without it.
        #   flagged  everything the check leg flags, right or wrong: a gate
        #            that skips none of these changes no warning anyone sees.
        #   adjudicated  the flagged claims whose warning labels_claim_v1.json
        #            grades CORRECT — the warnings worth keeping. A flagged
        #            claim graded WRONG is a false alarm, and skipping it is a gain.
        # The last two need the check leg's own results, which cover 125 of
        # the 152 claims; the population is restricted to those.
        load(verb, variants[0])
        if positives == "refuted":
            defects = {k for k, l in LABEL.items() if l == "refuted"}
        else:
            fl = llm_flagged()
            only = set(fl)
            if positives == "caught":
                defects = {k for k in fl if fl[k] and LABEL.get(k) == "refuted"}
            elif positives == "adjudicated":
                adj = json.load(open(HERE / "labels_claim_v1.json"))["labels"]
                defects = {(l["memo"], l["id"]) for l in adj if l["label"] == "CORRECT"}
            else:
                defects = {k for k in fl if fl[k]}
        print(f"positives: {positives}" + (f", over the {len(only)} claims the check leg ran on"
                                           if only else ""))
    else:
        defects = {tuple(x) for x in json.load(open(HERE / "defects_v1.json"))[verb]["subjects"]}
    cc = check_cost(verb)
    print(f"{verb}: {len(defects)} defect subjects, check leg ${cc:.5f}/subject\n")
    print(f"  {'variant':<12} {'draws':>5} {'AUC':>5} {'spread':>6}  {'skip @0':>8}  "
          f"{'skip @0-m':>9}  {'net':>5}  {'gate $/subj':>11}  errors")
    detail = {}
    for v in variants:
        draws, scores, cost, errors = load(verb, v)
        if not draws:
            print(f"  {v:<12} (no draws)")
            continue
        mean = {k: sum(x) / len(x) for k, x in scores.items() if only is None or k in only}
        pos = [mean[k] for k in mean if k in defects]
        neg = [mean[k] for k in mean if k not in defects]
        if verb == "claim":
            # AUC against the population a gate SHOULD skip; skip rates stay
            # over all traffic, qualified claims included.
            auc_neg = ([mean[k] for k in mean if LABEL.get(k) == "supports_only"]
                       if positives not in ("flagged", "adjudicated") else neg)
            qual_skipped = sum(1 for k in mean if LABEL.get(k) == "qualified"
                               and mean[k] < min(pos) - MARGIN)
        else:
            auc_neg, qual_skipped = neg, None
        spread = max(max(x) - min(x) for x in scores.values())
        lo = min(pos)
        skip0 = sum(1 for s in neg if s < lo)
        skipm = sum(1 for s in neg if s < lo - MARGIN)
        gate = cost / len(draws) / len(mean)
        net = (skipm * cc - len(mean) * gate) / (len(mean) * cc)
        print(f"  {v:<12} {len(draws):5} {auc(pos, auc_neg):5.2f} {spread:6.2f}  "
              f"{skip0 / len(mean):7.0%}  {skipm / len(mean):8.0%}  {net:5.0%}  "
              f"{gate:11.6f}  {errors}"
              + (f"   qualified skipped @0-m: {qual_skipped}" if qual_skipped is not None else ""))
        if positives == "adjudicated":
            # The flagged claims graded WRONG are false alarms a gate skipping
            # REMOVES from what the author is shown.
            wrong = {(l["memo"], l["id"]) for l in adj if l["label"] == "WRONG"}
            gone = sum(1 for k in wrong if k in mean and mean[k] < lo - MARGIN)
            print(f"  {'':<12} false alarms skipped @0-m: {gone} of {len(wrong)} -> warning "
                  f"precision {len(pos)}/{len(pos) + len(wrong) - gone} "
                  f"= {len(pos) / (len(pos) + len(wrong) - gone):.0%} (ungated "
                  f"{len(pos) / (len(pos) + len(wrong)):.0%})")
        detail[v] = {k: mean[k] for k in mean if k in defects}, sorted(neg)
    print(f"\n  defect subjects, mean score (and the negatives' median / 90th pct)")
    print(f"  {'':<10}" + "".join(f"{v:>12}" for v in detail))
    for k in sorted(defects):
        print(f"  {k[0]} {k[1]:<4}" + "".join(f"{detail[v][0].get(k, float('nan')):12.2f}"
                                           for v in detail))
    print(f"  {'neg p50':<10}" + "".join(f"{detail[v][1][len(detail[v][1]) // 2]:12.2f}"
                                        for v in detail))
    print(f"  {'neg p90':<10}" + "".join(f"{detail[v][1][int(len(detail[v][1]) * .9)]:12.2f}"
                                        for v in detail))


if __name__ == "__main__":
    main()
