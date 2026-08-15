#!/usr/bin/env python3
"""Approach A — one call. Claim and evidence together, verdict straight out.

The simplest thing that could work, and the one that failed on every local
model: both 3Bs collapsed to a constant verdict, because agreeing is cheaper
than checking. That was never a fair test of the *approach* though — only of
those models. Luna reasons, and this is the shape that benefits most from
reasoning, so it deserves its own harness rather than an inference from Qwen.

Scored on the same axis as the other two harnesses so the three are comparable:
did it flag a defective claim, and did it stay quiet on a sound one. A verdict
of `supports` counts as not-flagged; `refutes` or `qualifies` counts as flagged.

    python3 direct_eval.py --repeat 3
"""

import argparse, json, re, sys, time, urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from cases import CASES              # noqa: E402
from extra_cases import EXTRA_CASES  # noqa: E402
from extract_eval import auth_headers  # noqa: E402

DEFAULT_URL = "https://openrouter.ai/api/v1/chat/completions"
DEFAULT_MODEL = "openai/gpt-5.6-luna"

SYSTEM = """You grade one claim against the evidence captured for it, and nothing else.

supports  — you read evidence that STATES the claim. Evidence merely consistent with it is not enough.
            A claim that asserts less than the evidence shows is still supports.
refutes   — what the evidence shows contradicts the claim.
qualifies — the claim holds only under a condition it does not state, or is right in substance and
            wrong in one clause, or the evidence given cannot settle it.

Grade the claim AS WRITTEN. If it names a specific line, count or type, check that specific thing.
If it says "only", "never", "every" or "no", the quantifier is part of the claim and a single
exception makes it refutes or qualifies even when the surrounding point is correct. Naming the
clause that fails is the entire value of your answer.

Reply with one JSON object and nothing else:
{"verdict": "supports" | "refutes" | "qualifies", "note": "<one or two sentences>"}"""


def ask(url, model, case, timeout, max_tokens, effort):
    """One call, retried with a larger budget if reasoning ate the whole cap.

    Measured 2026-08-10: on identical input at temperature 0, reasoning length
    swings 516..2000 tokens run to run. When it reaches the cap the response
    comes back `finish_reason=length` with content of length ZERO — the model
    never reached the answer. Raising the cap on that draw is the fix; lowering
    it (an earlier version of this harness) is exactly backwards for a
    reasoning model. Cost is charged per token generated, so a bigger ceiling
    costs nothing on the runs that do not need it.
    """
    user = (f"CLAIM:\n{case['proposition']}\n\n"
            f"EVIDENCE — what was opened or run:\n" + "\n".join(f"  - {x}" for x in case["extent"]) +
            f"\n\nEVIDENCE — captured output:\n{case['output']}")
    spent, elapsed, cap = 0.0, 0.0, max_tokens
    for attempt in range(3):
        body = {"model": model,
                "messages": [{"role": "system", "content": SYSTEM}, {"role": "user", "content": user}],
                "temperature": 0.0, "max_tokens": cap}
        if effort != "none":
            body["reasoning"] = {"effort": effort}
        req = urllib.request.Request(url, data=json.dumps(body).encode(), headers=auth_headers())
        t0 = time.time()
        with urllib.request.urlopen(req, timeout=timeout) as r:
            d = json.load(r)
        elapsed += time.time() - t0
        spent += (d.get("usage") or {}).get("cost", 0.0) or 0.0
        choice = d["choices"][0]
        content = choice["message"].get("content") or ""
        if choice.get("finish_reason") != "length" and content.strip():
            return content, elapsed, spent
        cap *= 3
    return content, elapsed, spent


def parse(s):
    m = re.search(r"\{.*\}", s, re.S)
    if not m:
        return None, ""
    try:
        d = json.loads(m.group(0))
    except json.JSONDecodeError:
        return None, ""
    v = str(d.get("verdict", "")).strip().lower()
    return (v if v in ("supports", "refutes", "qualifies") else None), str(d.get("note", ""))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default=DEFAULT_URL)
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--reasoning-effort", default="high")
    ap.add_argument("--repeat", type=int, default=1)
    ap.add_argument("--only", action="append")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--max-tokens", type=int, default=4000)
    ap.add_argument("--out", default="direct_report.md")
    a = ap.parse_args()

    cases = [c for c in CASES if c.get("scope", "in") == "in"] + EXTRA_CASES
    if a.only:
        cases = [c for c in cases if c["id"] in a.only]

    rows, spend = [], 0.0
    for n, case in enumerate(cases, 1):
        for rep in range(a.repeat):
            try:
                raw, el, cost = ask(a.url, a.model, case, a.timeout, a.max_tokens, a.reasoning_effort)
                verdict, note = parse(raw)
                spend += cost or 0
                err = None if verdict else "unparsed"
            except Exception as ex:
                verdict = None; note = ""; el = 0; err = f"{type(ex).__name__}: {ex}"
            flagged = verdict in ("refutes", "qualifies")
            should = case["flaw"] is not None
            ok = flagged == should
            print(f"[{n}/{len(cases)}{'' if a.repeat == 1 else f' r{rep+1}'}] {case['id']}  "
                  f"{'ok  ' if ok else 'MISS'}  verdict={verdict}  {el:.0f}s", flush=True)
            rows.append(dict(case_id=case["id"], should_flag=should, flagged=flagged, ok=ok,
                             verdict=verdict, note=note, flaw=case["flaw"], err=err, elapsed=el))

    Path(a.out.replace(".md", ".json")).write_text(json.dumps(rows, indent=2))
    # Errors are their own column and never enter the flagged/quiet
    # denominators. Scoring an errored row as `flagged=False` (the previous
    # behaviour) misreports in both directions at once: on a flawed case it
    # reads as a miss the model never made, and on a clean one it reads as a
    # correct silence it never earned.
    answered = [r for r in rows if not r["err"]]
    flawed = [r for r in answered if r["should_flag"]]
    clean = [r for r in answered if not r["should_flag"]]
    print(f"\napproach              A — direct, one call")
    print(f"model                 {a.model} (reasoning={a.reasoning_effort}, repeat={a.repeat})")
    print(f"flawed cases flagged  {len([r for r in flawed if r['flagged']])} of {len(flawed)}")
    print(f"clean left alone      {len([r for r in clean if not r['flagged']])} of {len(clean)}")
    print(f"unparsed (excluded)   {len(rows) - len(answered)} of {len(rows)}")
    print(f"cost                  ${spend:.4f}")


if __name__ == "__main__":
    main()
