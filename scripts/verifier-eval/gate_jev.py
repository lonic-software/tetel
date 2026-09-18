#!/usr/bin/env python3
"""Would a cheap typed question have told us not to make the expensive call?

The refuter runs per *finding* and findings are rare. The check leg
(`verify.rs:1008`) runs per *mint*, carries the evidence, and is the call that
costs. Its correct answer is usually nothing at all — `CHECK_SYSTEM` says so:
"An empty list is the common and correct answer." So the question worth asking
first is whether there is anything here to look for.

This asks that, of every subject in a results file, and records the answer
beside what the LLM actually found for that subject. No LLM call is made: the
expensive side already ran and is in the file.

Three nouls in one call, because they evaluate in parallel and cost the same:
`any` is the gate, and `contradicts`/`overreaches` say which kind fired, which
is what a gate would hand the LLM to narrow its search.

    python3 gate_jev.py fact_v1.json --out gate_fact.json
"""

import argparse, json, sys, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import verbs_eval as V  # noqa: E402
import jev  # noqa: E402

# The two kinds, worded from CHECK_SYSTEM rather than freshly invented, and
# asked NEUTRALLY: the gate's threshold is applied when scoring. CHECK_SYSTEM's
# own rules about what is NOT a disagreement go in the `false` criteria,
# because they are the population this has to stay silent on.
NOT_A_DISAGREEMENT = (
    "Also false when: the evidence merely fails to establish the text; the capture does not touch "
    "what the text is about; the text says less than the evidence shows; the text describes what a "
    "design PROPOSES to build, which evidence captured beforehand cannot contradict; or the reader "
    "is simply uncertain. Insufficiency is not disagreement.")

QUESTIONS = {
    "any": {"type": "noul",
            "instructions": "The author's text disagrees with the captured evidence: it either "
                            "asserts something the evidence shows to be otherwise, or it ranges "
                            "wider than what was captured.",
            "criteria": {"true": "there is a disagreement of either kind", "false":
                         "there is no disagreement. " + NOT_A_DISAGREEMENT}},
    "contradicts": {"type": "noul",
                    "instructions": "The captured evidence shows something incompatible with an "
                                    "assertion in the text: a different number, name, type, line "
                                    "or behaviour.",
                    "criteria": {"true": "the evidence contradicts the text",
                                 "false": "it does not. " + NOT_A_DISAGREEMENT}},
    "overreaches": {"type": "noul",
                    "instructions": "An assertion in the text ranges wider than what was captured "
                                    "— it says every, never, only, no, always, any or cannot about "
                                    "a population the evidence samples rather than covers.",
                    "criteria": {"true": "the text reaches past its evidence",
                                 "false": "it does not. " + NOT_A_DISAGREEMENT}},
}


def gate_one(key, case, timeout):
    state = f"AUTHOR'S TEXT:\n{case['text']}\n\n{V.evidence_text(case)}"
    t0 = time.time()
    try:
        answers, cost, usage = jev.ask(state, QUESTIONS, key=key, timeout=timeout)
    except Exception as e:
        # A gate that errors must not skip the call. Fail open, loudly.
        return {"error": f"{type(e).__name__}: {e}"}, 0.0, time.time() - t0
    return ({k: answers.get(k, {}).get("noul") for k in QUESTIONS} | {"usage": usage},
            cost, time.time() - t0)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--out", required=True)
    ap.add_argument("--timeout", type=int, default=60)
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--limit", type=int, default=0)
    a = ap.parse_args()

    key = jev.api_key()
    d = json.load(open(a.results))
    verb = d["verb"]

    subjects = []
    for m in sorted({r["memo"] for r in d["records"]}):
        for c in V.subjects(m, verb):
            subjects.append((m, c))
    found = {(r["memo"], r["id"]): r for r in d["records"]}
    if a.limit:
        subjects = subjects[:a.limit]
    print(f"{len(subjects)} subjects to gate", file=sys.stderr)

    rows, cost, lat = [], 0.0, []
    with ThreadPoolExecutor(max_workers=a.workers) as ex:
        futs = {ex.submit(gate_one, key, c, a.timeout): (m, c) for m, c in subjects}
        for n, fut in enumerate(futs, 1):
            m, c = futs[fut]
            g, cst, el = fut.result()
            cost += cst
            lat.append(el)
            r = found.get((m, c["id"]))
            rows.append({"memo": m, "id": c["id"], "gate": g,
                         "llm_status": r["status"] if r else "absent",
                         "llm_findings": [{"kind": f["kind"], "clause": f["clause"]}
                                          for f in (r["findings"] if r else [])]})
            if n % 25 == 0 or n == len(futs):
                print(f"  {n}/{len(futs)}  ${cost:.5f}", file=sys.stderr)

    lat.sort()
    json.dump({"verb": verb, "source": a.results, "model": jev.MODEL,
               "cost": cost, "n": len(rows),
               "latency_s": {"median": lat[len(lat)//2], "max": lat[-1]},
               "rows": rows}, open(a.out, "w"), indent=1)
    print(f"\ncost ${cost:.5f} over {len(rows)} subjects "
          f"(${cost/len(rows):.6f} each), median {lat[len(lat)//2]:.2f}s")


if __name__ == "__main__":
    main()
