#!/usr/bin/env python3
"""Resolve `retrodict.py` output onto the gate declared in the design memo.

The memo's gate is stated in claims over claims, not rows over rows, and it
names two kill conditions:

  * steering hazard  -- >= 7 of the 62 claims carrying supports and nothing
    else are flagged at their first-render wording.
  * recall           -- <= 2 of the 9 claims that were ever refuted are
    surfaced with a finding naming the defect a later pass named.

A claim is "flagged" when a majority of its repeats flagged it, so a single
non-deterministic draw cannot decide a population.

Errored draws are NOT excluded, and saying otherwise would be the mistake the
earlier eval made and fixed. `retrodict.py` records an errored draw as
`flagged: false` with an `err` string, so a claim whose draws all errored
enters its population's denominator as an unflagged claim -- scored as a
correct silence it was never asked for. The `unanswered` rows below name
those claims so the reader can see which denominators contain one. In the run
this was written against there are three, and because all of a claim's draws
errored together rather than one of three, no majority anywhere was flipped:
the effect is that the flagged counts are marginally conservative, not that
any of them is wrong.

The "named the defect" column cannot be computed -- it is a reading of the
verifier's finding against the later pass's refutation note. The ids are
listed by hand below and `--show-refuted` prints both texts so the reading is
checkable rather than asserted.

usage:  python3 gate_result.py retro_full125x3.json [--show-refuted]
"""

import collections
import json
import sys

# Claims where the flag's finding names the same defect the later refutation
# named. Established by reading both texts; `--show-refuted` prints them.
NAMED_THE_DEFECT = {
    ("tet47-ground-what-is-owed.md", "C6"),
    ("tet56-bounded-grep-return.md", "C9"),
}


def population(rows):
    """The memo's three populations, from the later verdicts on a claim."""
    r = rows[0]
    if r["supports_only"]:
        return "supports_only"
    if r["refuted"]:
        return "refuted"
    if r["qualified"]:
        return "qualified_not_refuted"
    return "other"


def main():
    path = sys.argv[1]
    show = "--show-refuted" in sys.argv
    rows = json.load(open(path))

    by = collections.defaultdict(list)
    for row in rows:
        by[(row["memo"], row["id"])].append(row)

    pop = collections.Counter()
    flagged = collections.Counter()
    unanswered = collections.defaultdict(list)
    for key, reps in by.items():
        p = population(reps)
        pop[p] += 1
        if sum(1 for r in reps if r["flagged"]) * 2 > len(reps):
            flagged[p] += 1
        if all(r["err"] for r in reps):
            unanswered[p].append(key)

    def unanswered_note(p):
        rows = unanswered.get(p, [])
        if not rows:
            return ""
        names = ", ".join(f"{m.split('-')[0]} {i}" for m, i in rows)
        return f"   [{len(rows)} unanswered: {names}]"

    # The third population as the memo states it: every claim carrying at
    # least one qualifies, including those also refuted.
    any_qualifies = [k for k, reps in by.items() if reps[0]["qualified"]]
    q_flagged = sum(
        1
        for k in any_qualifies
        if sum(1 for r in by[k] if r["flagged"]) * 2 > len(by[k])
    )

    refuted = [k for k, reps in by.items() if reps[0]["refuted"]]
    refuted_flagged = [
        k for k in refuted if sum(1 for r in by[k] if r["flagged"]) * 2 > len(by[k])
    ]
    named = [k for k in refuted_flagged if k in NAMED_THE_DEFECT]

    print(f"claims                     {len(by)}")
    print(f"draws                      {len(rows)}")
    print(f"errored draws              {sum(1 for r in rows if r['err'])}")
    print(
        f"  on claims never answered {sum(len(v) for v in unanswered.values())}"
        "   (every draw errored; counted as unflagged in the rows below)"
    )
    print()
    print("GATE 1  steering hazard      kill at >= 7")
    print(
        f"  supports-only claims     {pop['supports_only']}"
        f"   flagged {flagged['supports_only']}"
        f"   -> {'KILLS' if flagged['supports_only'] >= 7 else 'PASSES'}"
        + unanswered_note("supports_only")
    )
    print()
    print("GATE 2  recall               kill at <= 2")
    print(
        f"  ever-refuted claims      {len(refuted)}   flagged {len(refuted_flagged)}"
        + unanswered_note("refuted")
    )
    print(
        f"  naming the same defect   {len(named)}"
        f"   -> {'KILLS' if len(named) <= 2 else 'PASSES'}"
    )
    print()
    print("NOT A KILL CONDITION")
    q_also_refuted = [k for k in any_qualifies if by[k][0]["refuted"]]
    q_flagged_also_refuted = [
        k
        for k in q_also_refuted
        if sum(1 for r in by[k] if r["flagged"]) * 2 > len(by[k])
    ]
    print(f"  claims with a qualifies  {len(any_qualifies)}   flagged {q_flagged}")
    print(
        f"  never refuted            {pop['qualified_not_refuted']}"
        f"   flagged {flagged['qualified_not_refuted']}"
        + unanswered_note("qualified_not_refuted")
    )
    # This bucket is the one the gate deliberately left without a
    # threshold, so it is easy to describe as wholly unresolved. It is not
    # quite: the claims in it that a later pass ALSO refuted are scored by
    # gate 2, and a flag on one of those has a verdict after all.
    print(
        f"  of the flagged, resolved {len(q_flagged_also_refuted)}"
        "   (also refuted, so scored by gate 2 above)"
    )
    print()
    # Over distinct claims: the three populations partition the corpus, but
    # "claims with a qualifies" overlaps "ever refuted", so summing the
    # printed rows above would count some claims twice.
    all_flagged = [
        k for k, reps in by.items() if sum(1 for r in reps if r["flagged"]) * 2 > len(reps)
    ]
    on_sound = sum(1 for k in all_flagged if by[k][0]["supports_only"])
    on_worked = len(all_flagged) - on_sound
    ever_worked = len(by) - pop["supports_only"]
    print("DERIVED, over distinct claims")
    print(f"  claims flagged           {len(all_flagged)}")
    print(f"  on later non-supporting  {on_worked}")
    print(f"  on supports-only         {on_sound}")
    print(f"  precision                {on_worked / len(all_flagged):.0%}")
    print(f"  recall                   {on_worked / ever_worked:.0%}"
          f"  ({on_worked}/{ever_worked} claims a later pass did not simply support)")

    if show:
        for key in refuted:
            reps = by[key]
            n = sum(1 for r in reps if r["flagged"])
            print()
            print("=" * 72)
            print(f"{key[0]}  {key[1]}   flagged {n}/{len(reps)}"
                  f"   named-the-defect: {key in NAMED_THE_DEFECT}")
            print(f"  proposition: {reps[0]['prop'][:300]}")
            for later in reps[0]["later"]:
                if later["verdict"] == "refutes":
                    print(f"  REFUTES: {later['note'][:400]}")
            for f in reps[0]["findings"]:
                print(f"  FINDING: {f.get('kind')} / {f.get('clause')}")
                print(f"           {f.get('why', '')[:300]}")


if __name__ == "__main__":
    main()
