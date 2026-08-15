#!/usr/bin/env python3
"""Ask a local model to grade tetel-shaped claims, and report what it caught.

    python3 run_eval.py                          # default model, all cases
    python3 run_eval.py --model <id>             # e.g. the 3B, for contrast
    python3 run_eval.py --only qualifies-count-off-by-one
    python3 run_eval.py --repeat 3               # same case N times, to see stability

The grader is given exactly what a tetel grounding pass gets: the proposition,
and the captured evidence behind it. It is NOT told there is anything wrong, is
not told the expected verdict, and is not told how many cases are flawed —
those are the hints that would make a pass agree by construction.

Two numbers matter and they are different:

  verdict    did it reach the right verdict at all
  clause     for a flawed case, did it name the actual defect

A grader that says `qualifies` for the wrong reason is not a grader that works.
Clause detection here is keyword matching over the note, which is coarse — it
sorts the notes for reading, it does not decide anything. Read them.

Stdlib only; no dependencies.
"""

import argparse, json, re, sys, time, urllib.request, urllib.error
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from cases import CASES  # noqa: E402

DEFAULT_URL = "https://openrouter.ai/api/v1/chat/completions"
DEFAULT_MODEL = "openai/gpt-5.6-luna"


def auth_headers():
    """Bearer token from the environment, never from a flag or a file.

    A key on the command line lands in shell history and in any transcript of
    this run; a key in the source lands in git. Set one of these instead:

        export OPENROUTER_API_KEY=...      # or OPENAI_API_KEY

    Absent, requests go out unauthenticated, which is what a local mlx-lm
    server wants.
    """
    import os
    h = {"Content-Type": "application/json"}
    key = os.environ.get("OPENROUTER_API_KEY") or os.environ.get("OPENAI_API_KEY")
    if key:
        h["Authorization"] = f"Bearer {key}"
    return h

SYSTEM = """You grade one claim against the evidence captured for it, and nothing else.

You are given a PROPOSITION and the EVIDENCE behind it: the extent (what was opened or run) and the
output that produced. Decide whether the evidence establishes the proposition.

Three verdicts, and the boundaries between them are the whole job:

  supports  — you read evidence that STATES the proposition. Evidence merely consistent with it is
              not enough. A proposition that claims less than the evidence shows is still supports.
  refutes   — what the evidence shows contradicts the proposition.
  qualifies — the proposition holds only under a condition it does not state, OR it is right in
              substance and wrong in one clause, OR the evidence given cannot settle it.

Grade the proposition AS WRITTEN. If it names a specific line, count, or range, check that specific
thing. If it says "only", "never", "every", "no", or "always", the quantifier is part of the claim
and a single exception makes it qualifies or refutes, even when the surrounding point is correct.
A proposition right in substance and wrong in one clause is qualifies, not supports — and naming
that clause is the entire value of your answer.

Watch for evidence that is the wrong SHAPE for the claim: a search for a word does not settle a
claim about a behaviour, and a search rooted at one directory does not settle a claim quantified
over a whole repository.

Answer with a single JSON object and nothing else:

{"verdict": "supports" | "refutes" | "qualifies",
 "note": "<one or two sentences; if not supports, name the exact clause that fails and why>"}"""

USER = """PROPOSITION:
{proposition}

EVIDENCE — extent (what was opened or run):
{extent}

EVIDENCE — captured output:
{output}"""


def ask(url, model, case, timeout, max_tokens, effort="high"):
    body = {
        "model": model,
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": USER.format(
                proposition=case["proposition"],
                extent="\n".join(f"  - {e}" for e in case["extent"]),
                output=case["output"],
            )},
        ],
        "temperature": 0.0,
        "max_tokens": max_tokens,
    }
    if effort != "none" and "127.0.0.1" not in url and "localhost" not in url:
        body["reasoning"] = {"effort": effort}
    body = json.dumps(body).encode()
    req = urllib.request.Request(url, data=body, headers=auth_headers())
    t0 = time.time()
    with urllib.request.urlopen(req, timeout=timeout) as r:
        payload = json.load(r)
    elapsed = time.time() - t0
    msg = payload["choices"][0]["message"]
    return msg.get("content") or "", msg.get("reasoning") or "", elapsed, payload.get("usage", {})


def parse(content):
    """Pull the JSON object out of a reply that may carry prose around it."""
    m = re.search(r"\{.*\}", content, re.S)
    if not m:
        return None, None
    try:
        d = json.loads(m.group(0))
    except json.JSONDecodeError:
        return None, None
    v = str(d.get("verdict", "")).strip().lower()
    return (v if v in ("supports", "refutes", "qualifies") else None), str(d.get("note", ""))


def found_clause(case, note):
    if not case["flaw_markers"] or not note:
        return None
    low = note.lower()
    return any(m.lower() in low for m in case["flaw_markers"])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default=DEFAULT_URL)
    ap.add_argument("--reasoning-effort", default="high",
                    choices=["none", "minimal", "low", "medium", "high", "xhigh", "max"],
                    help="OpenRouter reasoning effort; 'none' omits the parameter")
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--only", action="append", help="case id; repeatable")
    ap.add_argument("--scope", choices=["in","out","all"], default="all")
    ap.add_argument("--repeat", type=int, default=1)
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("--max-tokens", type=int, default=2000)
    ap.add_argument("--out", default="report.md")
    ap.add_argument("--raw", default="raw.json")
    args = ap.parse_args()

    cases = [c for c in CASES if not args.only or c["id"] in args.only]
    if args.scope != "all":
        cases = [c for c in cases if c.get("scope", "in") == args.scope]
    if not cases:
        sys.exit(f"no cases matched {args.only}")

    rows = []
    total = len(cases) * args.repeat
    n = 0
    for case in cases:
        for rep in range(args.repeat):
            n += 1
            print(f"[{n}/{total}] {case['id']}" + (f" (rep {rep+1})" if args.repeat > 1 else ""),
                  end="", flush=True)
            try:
                content, reasoning, elapsed, usage = ask(
                    args.url, args.model, case, args.timeout, args.max_tokens, args.reasoning_effort)
                verdict, note = parse(content)
                err = None
            except (urllib.error.URLError, OSError, KeyError, json.JSONDecodeError) as e:
                content = reasoning = ""
                verdict = note = None
                elapsed, usage, err = 0.0, {}, f"{type(e).__name__}: {e}"
            ok = (verdict == case["expect"])
            clause = found_clause(case, note) if case["flaw"] else None  # triage only
            mark = "err " if err else ("PASS" if ok else "MISS")
            extra = "" if clause is None else (" +clause" if clause else " -clause")
            print(f"  {mark}{extra}  {elapsed:.0f}s")
            rows.append(dict(case_id=case["id"], expect=case["expect"], got=verdict, ok=ok,
                             clause=clause, note=note, flaw=case["flaw"], err=err,
                             raw=content, reasoning=reasoning, elapsed=elapsed,
                             completion_tokens=usage.get("completion_tokens")))

    write_report(Path(args.out), args.model, rows)
    Path(args.raw).write_text(json.dumps(rows, indent=2))
    summarise(args.model, rows)


def summarise(model, rows):
    graded = [r for r in rows if not r["err"]]
    right = [r for r in graded if r["ok"]]
    flawed = [r for r in graded if r["flaw"]]
    caught = [r for r in flawed if r["ok"]]
    named = [r for r in caught if r["clause"]]
    clean = [r for r in graded if not r["flaw"]]
    print()
    print(f"model                     {model}")
    print(f"cases graded              {len(graded)} of {len(rows)}")
    print(f"verdict correct           {len(right)} of {len(graded)}")
    print(f"  flawed cases caught     {len(caught)} of {len(flawed)}")
    print(f"  clean cases not flagged {len([r for r in clean if r['ok']])} of {len(clean)}")
    print(f"defect named correctly    {len(named)} of {len(caught)}   (keyword match — read the notes)")
    if graded:
        print(f"median latency            {sorted(r['elapsed'] for r in graded)[len(graded)//2]:.0f}s")


def write_report(path, model, rows):
    L = [f"# Local verifier eval — `{model}`", ""]
    L += ["A false negative on a flawed case is the expensive error: it is a claim the loop would",
          "have shipped. A false positive on a clean case costs a revision round.", "",
          "| case | expected | got | verdict | defect named |", "|---|---|---|---|---|"]
    for r in rows:
        got = r["err"] and "error" or (r["got"] or "unparsed")
        v = "ok" if r["ok"] else "**MISS**"
        c = "—" if r["clause"] is None else ("yes" if r["clause"] else "**no**")
        L.append(f"| `{r['case_id']}` | {r['expect']} | {got} | {v} | {c} |")
    L += ["", "## Every answer, in the model's own words", ""]
    for r in rows:
        L += [f"### `{r['case_id']}` — expected **{r['expect']}**, got **{r['got'] or 'unparsed'}**", ""]
        if r["flaw"]:
            L += [f"*The defect a correct grader must notice:* {r['flaw']}", ""]
        L += [f"> {r['note'] or r['err'] or r['raw'][:400] or '(empty)'}", ""]
    path.write_text("\n".join(L))
    print(f"\nwrote {path} and raw.json")


if __name__ == "__main__":
    main()
