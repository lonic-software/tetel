#!/usr/bin/env python3
"""Screen: gpt-6-luna at one draw against gpt-5.6-luna's draw 0, same arms.

Grades with score_tet98's tables, so a finding those never saw is listed as
still to read rather than guessed at.

    python3 screen_gpt6.py   # prints the comparison, then every finding still to read
"""
import sys
from pathlib import Path

HERE = Path(__file__).parent
ROOT = HERE.parents[2]
sys.argv = [sys.argv[0], str(ROOT / "tetel-eval-runs/tet98")]
sys.path.insert(0, str(HERE))
import score_tet98 as S  # noqa: E402

OLD, NEW = ROOT / "tetel-eval-runs/tet98", ROOT / "tetel-eval-runs/tet98-gpt6"


def load(run, name, draw0=False):
    S.RUN = run
    subj = S.load(name)
    if draw0:
        subj = {k: [v for v in vs if v["draw"] == 0] for k, vs in subj.items()}
    return {k: vs for k, vs in subj.items() if vs}


def med(xs):
    xs = sorted(xs)
    return xs[len(xs) // 2] if xs else 0


def lat(subj):
    return med([v["record"].get("elapsed_ms") or v["record"].get("latency_ms") or 0
                for vs in subj.values() for v in vs if v["record"]["status"] == "ok"]) / 1000


to_read = []
print("CLAIM, one draw, typed_model + literals\n")
print(f"  {'model':<8} {'reading':<9} {'sample':<6} {'claims':>6} {'err':>4} {'flagged':>7} "
      f"{'correct':>7} {'wrong':>5} {'read':>4} {'sound flagged':>15} {'cost':>7}")
for label, run, d0 in (("5.6", OLD, True), ("6", NEW, False)):
    subj = load(run, "claim_candidate", d0)
    for reading, pred in (("all", S.flagged), ("no-lit", S.flagged_by_check)):
        for sample in ("in", "out", "all"):
            ks = [k for k, vs in subj.items() if sample in ("all", vs[0]["sample"])]
            err = sum(len(subj[k]) - len(S.answered(subj[k])) for k in ks)
            fl = [k for k in ks if S.majority(subj[k], pred)]
            g = {k: S.grade_claim(k, subj[k], check_only=pred is S.flagged_by_check) for k in fl}
            if label == "6" and reading == "all" and sample == "all":
                to_read += [("claim", k) for k, v in g.items() if v == "READ"]
            n = lambda x: sum(v == x for v in g.values())
            sound = [k for k in ks if subj[k][0]["supports_only"]]
            cost = sum(v["record"].get("cost") or 0 for k in ks for v in subj[k])
            print(f"  {label:<8} {reading:<9} {sample:<6} {len(ks):>6} {err:>4} {len(fl):>7} {n('CORRECT'):>7} "
                  f"{n('WRONG'):>5} {n('READ'):>4} {S.pct(sum(k in fl for k in sound), len(sound)):>15} ${cost:>6.3f}")
    print(f"  {label:<8} median ok draw {lat(subj):.0f}s\n")

print("FACT, one draw, gated\n")
held = {(m, i) for (m, i, _), (v, _) in S.OUT_FACT.items() if v == "CORRECT"}
for label, run, d0 in (("5.6", OLD, True), ("6", NEW, False)):
    subj = load(run, "fact_gated", d0)
    fit = {k for k in subj if (k[0][:5], k[1]) in S.FIT_DEFECTS}
    fl = {k for k in subj if S.majority(subj[k], S.flagged)}
    err = sum(len(vs) - len(S.answered(vs)) for vs in subj.values())
    gd = sum(v["record"]["status"] == "gated" for vs in subj.values() for v in vs)
    cost = sum(v["record"].get("cost") or 0 for vs in subj.values() for v in vs)
    graded_out = {(m, i) for (m, i, _) in S.OUT_FACT}
    other_in = {k for k in fl if subj[k][0]["sample"] == "in" and k not in fit}
    unread_out = {k for k in fl if subj[k][0]["sample"] == "out" and k not in graded_out}
    wrong_out = {k for k in fl if subj[k][0]["sample"] == "out" and k in graded_out and k not in held}
    if label == "6":
        to_read += [("fact", k) for k in sorted(unread_out)]
    print(f"  {label:<4} facts {len(subj)}, errors {err}, gated {gd}, flagged {len(fl)}, cost ${cost:.3f}, "
          f"median ok draw {lat(subj):.0f}s")
    print(f"       fitted defects raised {len(fl & fit)}/{len(fit)}; held-out positives raised {len(fl & held)}/{len(held)}")
    print(f"       flags elsewhere: in-sample not a fitted defect {len(other_in)}; held-out on a fact graded"
          f" only WRONG {len(wrong_out)}; held-out never graded {len(unread_out)}\n")

print(f"To read: {len(to_read)}")
for kind, k in to_read:
    print(f"  {kind} {k[0]} {k[1]}")
