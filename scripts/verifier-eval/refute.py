#!/usr/bin/env python3
"""Ask a second call to refute each finding, and keep only the survivors.

Two rounds of prompt work on the `fact` verifier moved precision from 21% to
21%. The clusters changed — the self-defeating ones went, a new
"census-skipped-something" one took their place — which is the same
displacement the literal check showed: instructing a model away from one
wrong move relocates it rather than removing it.

What the same measurements show, three times over, is that a model asked the
*other* question gets it right. Handed the note, the evidence and the finding,
an independent pass rejected 11 of 14 baseline findings, 11 of 14 after the
prompt round, and 12 of 12 on prose — and in every case the ones it kept were
the ones a careful reader would keep. The judgement is in the model. A single
pass asked to FIND disagreements does not exercise it, because finding and
checking are different questions and only one of them was ever asked.

So this asks the second one. The prompt below is the adjudication brief that
produced those three results, near enough verbatim, rather than a new one
invented for the occasion — it is the only prompt here with evidence behind
it.

The economics work because findings are rare against subjects: 68 findings
over 123 facts means a refutation pass costs about a third more than the run
it filters, not double. And it runs offline against a results file, so it
reuses what the base pass already paid for.

    python3 refute.py fact_123.json --out fact_123_refuted.json
    python3 refute.py prose_112.json --out prose_112_refuted.json
"""

import argparse, json, sys, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from retrodict import DEFAULT_URL, DEFAULT_MODEL, one_call  # noqa: E402
import verbs_eval as V  # noqa: E402
from extract_eval import auth_headers  # noqa: E402

REFUTE_SYSTEM = """You are given a note or paragraph an author wrote, the evidence a tool captured,
and someone's assertion that a specific clause disagrees with that evidence. Decide whether the
assertion is correct.

There are two kinds it may claim:

  contradicts — the evidence shows something incompatible with the clause: a different number, name,
                type, line or behaviour.
  overreaches — the clause ranges wider than what was captured; it says "every", "never", "only",
                "no", "always", "any" or "cannot" about a population the evidence samples rather
                than covers.

Answer CORRECT only if the clause really does disagree with the captured evidence, in the way and of
the kind claimed, such that the author would have wanted to know. Answer WRONG if it does not. That
includes all of these, each of which has been observed:

  * the evidence supports the clause
  * the reason misreads the evidence
  * the reason objects to a clause other than the one quoted, or restates the clause
  * the clause's scope is fixed by its own sentence, and the objection re-reads it more broadly
  * the captured text — often a doc comment or a declaration — states the general property itself,
    so the author is reporting the capture rather than generalising past it
  * the capture disclosed an exclusion or a truncation, and the objection's whole content is that
    something was not covered; insufficiency is not disagreement
  * the objection is word-level pedantry that does not change what the text tells a reader
  * the clause describes what the design PROPOSES to build; evidence captured beforehand cannot
    contradict it
  * the text already states the limitation being reported back to it

Answer UNCLEAR only if the evidence is genuinely insufficient to settle it either way.

**Default to WRONG when you are not convinced.** This will be shown to an author as a warning, and a
wrong warning costs more than a missed one, so the burden of proof is on the assertion and not on the
author's text. Do not be generous to it.

Judge only against the evidence you were given. Do not assume facts about any codebase beyond it.

Reply with one JSON object and nothing else:
{"verdict": "CORRECT"|"WRONG"|"UNCLEAR", "why": ""}"""


def refute(url, model, case, finding, timeout, max_tokens, effort):
    user = (f"AUTHOR'S TEXT:\n{case['text']}\n\n{V.evidence_text(case)}\n\n"
            f"PROPOSED DISAGREEMENT:\n"
            f"  kind: {finding['kind']}\n"
            f"  clause: {finding['clause']}\n"
            f"  reason: {finding['why']}\n"
            f"  quotation offered: {finding.get('span') or finding.get('rejected') or '(none)'}")
    try:
        body, cost = one_call(url, model, REFUTE_SYSTEM, user, timeout, max_tokens, effort)
    except Exception as e:
        # An errored refutation is not a refutation. The finding survives, so
        # a transport failure can never silently delete a real warning.
        return "ERROR", f"{type(e).__name__}: {e}", 0.0
    v = V.json_object(body) or {}
    verdict = str(v.get("verdict", "")).upper()
    if verdict not in ("CORRECT", "WRONG", "UNCLEAR"):
        return "ERROR", (body or "")[:160], cost
    return verdict, v.get("why", ""), cost


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--out", required=True)
    ap.add_argument("--keep", default="CORRECT,UNCLEAR,ERROR",
                    help="verdicts that survive (default: everything but WRONG)")
    ap.add_argument("--url", default=DEFAULT_URL)
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--reasoning-effort", default="high")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--max-tokens", type=int, default=4000)
    ap.add_argument("--workers", type=int, default=6)
    a = ap.parse_args()

    d = json.load(open(a.results))
    verb = d["verb"]
    keep = set(a.keep.split(","))
    byid = {}
    for m in {r["memo"] for r in d["records"]}:
        for c in V.subjects(m, verb):
            byid[(m, c["id"])] = c

    jobs = []
    for r in d["records"]:
        if r["status"] != "ok":
            continue
        for i, f in enumerate(r["findings"]):
            jobs.append((r, i, f, byid[(r["memo"], r["id"])]))
    print(f"{len(jobs)} findings to put to a refutation pass", file=sys.stderr)

    results, cost = {}, 0.0
    with ThreadPoolExecutor(max_workers=a.workers) as ex:
        futs = {ex.submit(refute, a.url, a.model, c, f, a.timeout, a.max_tokens,
                          a.reasoning_effort): (r["memo"], r["id"], i)
                for r, i, f, c in jobs}
        for n, fut in enumerate(futs, 1):
            verdict, why, c = fut.result()
            results[futs[fut]] = (verdict, why)
            cost += c
            if n % 10 == 0 or n == len(futs):
                print(f"  {n}/{len(futs)}  ${cost:.4f}", file=sys.stderr)

    tally = {}
    for r in d["records"]:
        if r["status"] != "ok":
            continue
        kept = []
        for i, f in enumerate(r["findings"]):
            verdict, why = results.get((r["memo"], r["id"], i), ("ERROR", ""))
            tally[verdict] = tally.get(verdict, 0) + 1
            f["refutation"] = {"verdict": verdict, "why": why}
            if verdict in keep:
                kept.append(f)
        r["findings_before_refutation"] = len(r["findings"])
        r["findings"] = kept
    d["refutation"] = {"cost": cost, "tally": tally, "keep": sorted(keep)}
    json.dump(d, open(a.out, "w"), indent=1)

    print(f"\nrefutation cost ${cost:.4f}")
    for k in sorted(tally):
        print(f"  {k:<9} {tally[k]}")
    print()
    print(V.summarise(d["records"], verb))


if __name__ == "__main__":
    main()
