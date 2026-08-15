#!/usr/bin/env python3
"""Approach C — a three-stage pipeline. Extract, extract, then judge the two extracts.

Stage 1 and 2 are exactly `extract2`'s: the model reads the claim, then reads the
evidence, and never learns the two are related. Stage 3 is new — instead of the
Python comparator deciding, the model is handed both JSON objects and asked what
disagrees.

Why this might beat both siblings. Against approach A (one call), the judge never
sees the prose, so it cannot be talked round by a fluent claim — it compares two
structured records, which is a narrower and more mechanical question. Against
approach B (code compares), it can recognise that "an explicit array of ten
entries" and "[&str; 10]" are the same fact, which cost B six false positives and
took a hand-written shape rule to fix.

What it gives up is decidability. B's findings are re-derivable by anyone with the
two extracts; C's are a model's opinion about them, and tetel's refusals are
format-level for a reason. That trade is the point of measuring it.

    python3 judge_eval.py --repeat 3
"""

import argparse, json, re, sys, time, urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from cases import CASES              # noqa: E402
from extra_cases import EXTRA_CASES  # noqa: E402
from extract_eval import auth_headers  # noqa: E402
from extract2 import SYSTEM as EXTRACT_SYSTEM, parse as parse_extract  # noqa: E402

DEFAULT_URL = "https://openrouter.ai/api/v1/chat/completions"
DEFAULT_MODEL = "openai/gpt-5.6-luna"

JUDGE_SYSTEM = """You are given two structured records, extracted independently from two texts.

CLAIM holds what an author asserted. EVIDENCE holds what was actually captured — a file that was
opened, a command that was run, and what it returned.

Report only DISAGREEMENTS between them. A disagreement is one of:

  a name bound to two things that cannot both be true — a type, a value, a line
  a number the claim states that the evidence contradicts
  a scope the claim ranges over that is wider than what the evidence covers
  a count the claim states that the evidence shows differently
  a path or name the claim reasons about that the evidence never contains

The same fact stated two ways is NOT a disagreement. "an explicit array of ten entries" and
"[&str; 10]" are the same fact. A claim that says less than the evidence shows is not a
disagreement. Prose describing code is not a disagreement with the code.

Reply with one JSON object and nothing else:
{"disagreements": [{"kind": "", "detail": ""}]}

Give an empty list when the two records are consistent. An empty list is the common, correct answer."""


def ask(url, model, system, user, timeout, max_tokens, effort):
    """One call, retried with a larger budget if reasoning ate the whole cap.

    All four of this harness's failures on the 2026-08-10 run were this and
    not the pipeline: measured on identical input at temperature 0, reasoning
    swings 516..2000 tokens, and on the draws that hit the cap the response
    carries `finish_reason=length` with content of length ZERO. Retry upward;
    cost is per token generated, so the headroom is free when unused.
    """
    spent, elapsed, cap = 0.0, 0.0, max_tokens
    for attempt in range(3):
        body = {"model": model,
                "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
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


def parse_judgement(s):
    m = re.search(r"\{.*\}", s, re.S)
    if not m:
        return None
    try:
        d = json.loads(m.group(0))
    except json.JSONDecodeError:
        return None
    out = []
    for x in d.get("disagreements") or []:
        if isinstance(x, dict):
            out.append((str(x.get("kind", "")), str(x.get("detail", ""))))
        elif isinstance(x, str):
            out.append(("disagreement", x))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default=DEFAULT_URL)
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--reasoning-effort", default="high")
    ap.add_argument("--repeat", type=int, default=1)
    ap.add_argument("--only", action="append")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--max-tokens", type=int, default=4000)
    ap.add_argument("--out", default="judge_report.md")
    a = ap.parse_args()

    cases = [c for c in CASES if c.get("scope", "in") == "in"] + EXTRA_CASES
    if a.only:
        cases = [c for c in cases if c["id"] in a.only]

    rows, spend = [], 0.0
    for n, case in enumerate(cases, 1):
        for rep in range(a.repeat):
            ext = "\n".join(case["extent"])
            try:
                cr, t1, k1 = ask(a.url, a.model, EXTRACT_SYSTEM, "TEXT:\n" + case["proposition"],
                                 a.timeout, a.max_tokens, a.reasoning_effort)
                er, t2, k2 = ask(a.url, a.model, EXTRACT_SYSTEM, "TEXT:\n" + ext + "\n" + case["output"],
                                 a.timeout, a.max_tokens, a.reasoning_effort)
                cx, ex = parse_extract(cr), parse_extract(er)
                spend += (k1 or 0) + (k2 or 0)
                if not (cx and ex):
                    raise ValueError("extraction unparsed")
                jr, t3, k3 = ask(a.url, a.model, JUDGE_SYSTEM,
                                 "CLAIM:\n" + json.dumps(cx, indent=2) +
                                 "\n\nEVIDENCE:\n" + json.dumps(ex, indent=2),
                                 a.timeout, a.max_tokens, a.reasoning_effort)
                spend += k3 or 0
                findings = parse_judgement(jr)
                el = t1 + t2 + t3
                err = None if findings is not None else "judgement unparsed"
            except Exception as ex_:
                cx = ex = None; findings = None; el = 0; err = f"{type(ex_).__name__}: {ex_}"

            flagged = bool(findings)
            should = case["flaw"] is not None
            ok = flagged == should
            print(f"[{n}/{len(cases)}{'' if a.repeat == 1 else f' r{rep+1}'}] {case['id']}  "
                  f"{'ok  ' if ok else 'MISS'}  {len(findings or [])} disagreement(s)  {el:.0f}s", flush=True)
            rows.append(dict(case_id=case["id"], should_flag=should, flagged=flagged, ok=ok,
                             findings=findings or [], flaw=case["flaw"], err=err,
                             claim=cx, evidence=ex, elapsed=el))

    Path(a.out.replace(".md", ".json")).write_text(json.dumps(rows, indent=2))
    # Errors are their own column and never enter the flagged/quiet
    # denominators. The 2026-08-10 run shows why it misreports in *both*
    # directions: of four failures, two landed on flawed cases and read as
    # misses this pipeline never made (28/30 was really 28/28), and two
    # landed on a clean case where `flagged=False` scored as a correct
    # silence it never earned.
    answered = [r for r in rows if not r["err"]]
    flawed = [r for r in answered if r["should_flag"]]
    clean = [r for r in answered if not r["should_flag"]]
    print(f"\napproach              C — extract, extract, model judges")
    print(f"model                 {a.model} (reasoning={a.reasoning_effort}, repeat={a.repeat})")
    print(f"flawed cases flagged  {len([r for r in flawed if r['flagged']])} of {len(flawed)}")
    print(f"clean left alone      {len([r for r in clean if not r['flagged']])} of {len(clean)}")
    print(f"failed (excluded)     {len(rows) - len(answered)} of {len(rows)}")
    print(f"cost                  ${spend:.4f}")


if __name__ == "__main__":
    main()
