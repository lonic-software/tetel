#!/usr/bin/env python3
"""The refutation pass, asked of Jev instead of an LLM.

Same job assembly as `refute.py`, same subjects, same findings, same output
file shape — so `score_refuter.py` grades this against the adjudicated labels
exactly as it grades the Sonnet and Gemini runs. Only the model changes, which
is what makes the comparison about the model.

TWO QUESTIONS, ONE CALL. Jev evaluates a question map in parallel and the
published latency barely moves with more questions, so asking both costs one
call:

  verdict — a `choice` over CORRECT/WRONG/UNCLEAR. The shipped refuter's own
            three options, already enumerated in REFUTE_SYSTEM. Directly
            comparable to what Sonnet and Gemini were asked.
  correct — a `noul`, P(the disagreement is correct). This is what the shipped
            refuter has no way to return, and it is the whole reason to
            measure this model class here.

The `noul` is deliberately asked NEUTRALLY. REFUTE_SYSTEM ends "Default to
WRONG when you are not convinced ... the burden of proof is on the assertion"
— a decision threshold written in prose because there was no number to set.
With a probability the threshold IS the number, applied afterwards by
`score_refuter.py --prob`. Writing the bias into the question too would apply
it twice and make the sweep meaningless. The `choice` keeps the instruction,
because that arm is imitating the shipped refuter rather than replacing it.

    python3 refute_jev.py fact_v1.json --out fact_refuted_jev.json
"""

import argparse, json, sys, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import verbs_eval as V  # noqa: E402
import jev  # noqa: E402

# Ported from REFUTE_SYSTEM in src/verify.rs, near enough verbatim: the
# criteria descriptions carry the nine observed WRONG shapes that prompt
# lists. A new brief invented for the occasion would measure the brief.
INSTRUCTIONS = (
    "An author wrote a note. A tool captured evidence. Someone then asserted that a specific "
    "clause of the note disagrees with that evidence. Decide whether that assertion is correct.")

WRONG_DESC = (
    "The assertion does not hold. Includes: the evidence supports the clause; the reason misreads "
    "the evidence; it objects to a clause other than the one quoted, or restates it; the clause's "
    "scope is fixed by its own sentence and the objection re-reads it more broadly; the captured "
    "text states the general property itself, so the author is reporting the capture; the capture "
    "disclosed an exclusion or truncation and the objection's whole content is that something was "
    "not covered, which is insufficiency, not disagreement; word-level pedantry that does not "
    "change what the text tells a reader; the clause describes what the design PROPOSES to build, "
    "which evidence captured beforehand cannot contradict; the text already states the limitation "
    "being reported back to it.")

CORRECT_DESC = (
    "The clause really does disagree with the captured evidence, in the way and of the kind "
    "claimed, such that the author would have wanted to know. Default to WRONG when not "
    "convinced: this is shown to an author as a warning, and a wrong warning costs more than a "
    "missed one, so the burden of proof is on the assertion and not on the author's text.")

UNCLEAR_DESC = "The evidence is genuinely insufficient to settle it either way."


def questions():
    return {
        "verdict": {"type": "choice", "instructions": INSTRUCTIONS,
                    "criteria": {"CORRECT": CORRECT_DESC, "WRONG": WRONG_DESC,
                                 "UNCLEAR": UNCLEAR_DESC}},
        # Neutral on purpose — the threshold is applied when scoring, not here.
        "correct": {"type": "noul",
                    "instructions": "The proposed disagreement is correct: the quoted clause "
                                    "really does disagree with the captured evidence, in the "
                                    "way and of the kind claimed.",
                    "criteria": {"true": "the clause disagrees with the evidence as claimed",
                                 "false": "it does not"}},
    }


def state_for(case, finding):
    return (f"AUTHOR'S TEXT:\n{case['text']}\n\n{V.evidence_text(case)}\n\n"
            f"PROPOSED DISAGREEMENT:\n"
            f"  kind: {finding['kind']}\n"
            f"  clause: {finding['clause']}\n"
            f"  reason: {finding['why']}\n"
            f"  quotation offered: {finding.get('span') or finding.get('rejected') or '(none)'}")


def refute_one(key, case, finding, timeout):
    t0 = time.time()
    try:
        answers, cost, usage = jev.ask(state_for(case, finding), questions(),
                                       key=key, timeout=timeout)
    except Exception as e:
        # An errored refutation is not a refutation — refute.py's rule. The
        # finding survives, so a transport failure can never delete a warning.
        return {"verdict": "ERROR", "why": f"{type(e).__name__}: {e}"}, 0.0, time.time() - t0
    v = answers.get("verdict", {})
    n = answers.get("correct", {})
    out = {"verdict": str(v.get("choice", "ERROR")).upper(),
           "p_correct": n.get("noul"),
           "choice_confidence": v.get("confidence"),
           "choice_probabilities": v.get("probabilities"),
           "usage": usage}
    if out["verdict"] not in ("CORRECT", "WRONG", "UNCLEAR"):
        out["verdict"] = "ERROR"
    return out, cost, time.time() - t0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--out", required=True)
    ap.add_argument("--keep", default="CORRECT,UNCLEAR,ERROR")
    ap.add_argument("--kind", default="contradicts",
                    help="which finding kinds to refute (default: the adjudicated partition)")
    ap.add_argument("--timeout", type=int, default=60)
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--limit", type=int, default=0, help="stop after N findings (smoke test)")
    a = ap.parse_args()

    key = jev.api_key()
    d = json.load(open(a.results))
    verb = d["verb"]
    keep = set(a.keep.split(","))
    kinds = set(a.kind.split(",")) if a.kind else None

    byid = {}
    for m in {r["memo"] for r in d["records"]}:
        for c in V.subjects(m, verb):
            byid[(m, c["id"])] = c

    jobs = []
    for r in d["records"]:
        if r["status"] != "ok":
            continue
        for i, f in enumerate(r["findings"]):
            if kinds and f["kind"] not in kinds:
                continue
            jobs.append((r, i, f, byid[(r["memo"], r["id"])]))
    if a.limit:
        jobs = jobs[:a.limit]
    print(f"{len(jobs)} findings to put to Jev", file=sys.stderr)

    results, cost, lat = {}, 0.0, []
    with ThreadPoolExecutor(max_workers=a.workers) as ex:
        futs = {ex.submit(refute_one, key, c, f, a.timeout): (r["memo"], r["id"], i)
                for r, i, f, c in jobs}
        for n, fut in enumerate(futs, 1):
            out, c, el = fut.result()
            results[futs[fut]] = out
            cost += c
            lat.append(el)
            if n % 5 == 0 or n == len(futs):
                print(f"  {n}/{len(futs)}  ${cost:.6f}", file=sys.stderr)

    tally = {}
    for r in d["records"]:
        if r["status"] != "ok":
            continue
        kept = []
        for i, f in enumerate(r["findings"]):
            out = results.get((r["memo"], r["id"], i))
            if out is None:           # not refuted (other kind, or --limit)
                kept.append(f)
                continue
            tally[out["verdict"]] = tally.get(out["verdict"], 0) + 1
            f["refutation"] = out
            if out["verdict"] in keep:
                kept.append(f)
        r["findings_before_refutation"] = len(r["findings"])
        r["findings"] = kept
    lat.sort()
    d["refutation"] = {"model": jev.MODEL, "cost": cost, "tally": tally,
                       "keep": sorted(keep), "n": len(lat),
                       "latency_s": {"median": lat[len(lat)//2] if lat else None,
                                     "max": lat[-1] if lat else None}}
    json.dump(d, open(a.out, "w"), indent=1)

    print(f"\ncost ${cost:.6f} over {len(lat)} calls")
    if lat:
        print(f"latency median {lat[len(lat)//2]:.2f}s  max {lat[-1]:.2f}s")
    for k in sorted(tally):
        print(f"  {k:<9} {tally[k]}")


if __name__ == "__main__":
    main()
