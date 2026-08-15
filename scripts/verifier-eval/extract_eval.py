#!/usr/bin/env python3
"""Extraction-plus-compare: ask the model to READ, let code DECIDE.

The verdict experiment showed both 3B models degenerate — one answers
`qualifies` to everything, one answers `supports` to everything. Neither reads
the evidence, because agreeing is cheaper than checking. So stop asking for a
judgement.

Here the model does only what a small model is good at: listing the checkable
atoms it can see in a piece of text — paths, line references, numbers,
identifiers, quantifier words. It is asked the same question twice, once about
the claim and once about the evidence, and it is never told they are related.
It cannot agree, because it is never asked whether anything is true.

The contradiction is then found by `compare()`, which is ordinary code: a number
in the claim that appears nowhere in the evidence is a finding. That makes the
check decidable rather than a model's opinion — the same reason tetel's own
refusals are format-level.

    python3 extract_eval.py --model <id>
    python3 extract_eval.py --model <id> --only refutes-wrong-citation

Stdlib only.
"""

import argparse, json, re, sys, time, urllib.request, urllib.error
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from cases import CASES  # noqa: E402
from extra_cases import EXTRA_CASES  # noqa: E402

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

EXTRACT_SYSTEM = """List what the text mentions, as one JSON object:

{"paths": [], "line_refs": [], "numbers": [], "identifiers": [], "quantifiers": []}

paths — file paths or file names
line_refs — line numbers or line ranges
numbers — any other number
identifiers — code names: functions, types, fields, constants
quantifiers — words like only, every, never, all, none

Copy each item exactly as it appears in the text. Reply with the JSON object alone."""

FIELDS = ("paths", "line_refs", "numbers", "identifiers", "quantifiers")


def ask(url, model, text, timeout, max_tokens, no_think=False, effort="high"):
    payload = {
        "model": model,
        "messages": [{"role": "system", "content": EXTRACT_SYSTEM},
                     {"role": "user", "content": f"TEXT:\n{text}"}],
        "temperature": 0.0, "max_tokens": max_tokens,
    }
    # A reasoning model with no terminating thought emits nothing into `content`
    # at all: Gemma 4 12B produced 16,362 characters of `reasoning` and an empty
    # `content` at a 6000-token cap, on every one of 14 cases. Turning thinking
    # off took the same call from 110s and unparseable to 2s and clean JSON.
    # Extraction is a reading task; there is nothing here to think about.
    if no_think:
        payload["chat_template_kwargs"] = {"enable_thinking": False}
    # A local mlx-lm server rejects unknown top-level fields, so only send this
    # where it means something. Extraction needs little of it; the verdict step
    # is where reasoning measurably paid.
    if effort != "none" and "127.0.0.1" not in url and "localhost" not in url:
        payload["reasoning"] = {"effort": effort}
    body = json.dumps(payload).encode()
    req = urllib.request.Request(url, data=body, headers=auth_headers())
    t0 = time.time()
    with urllib.request.urlopen(req, timeout=timeout) as r:
        payload = json.load(r)
    msg = payload["choices"][0]["message"]
    return (msg.get("content") or ""), time.time() - t0


def parse(content):
    m = re.search(r"\{.*\}", content, re.S)
    if not m:
        return None
    try:
        d = json.loads(m.group(0))
    except json.JSONDecodeError:
        return None
    out = {}
    for f in FIELDS:
        v = d.get(f, [])
        out[f] = [str(x).strip() for x in v] if isinstance(v, list) else []
    return out


# ---------------------------------------------------------------- comparison

def norm_num(s):
    """13.0M -> 13000000; 1,234 -> 1234; keeps plain ints as ints."""
    t = str(s).strip().lower().replace(",", "").replace("_", "")
    m = re.fullmatch(r"(\d+(?:\.\d+)?)\s*([km])?", t)
    if not m:
        return None
    val = float(m.group(1))
    if m.group(2) == "k":
        val *= 1_000
    elif m.group(2) == "m":
        val *= 1_000_000
    return val


def line_span(s):
    m = re.fullmatch(r"\s*(\d+)\s*[-:]\s*(\d+)\s*", str(s))
    if m:
        return int(m.group(1)), int(m.group(2))
    m = re.fullmatch(r"\s*(\d+)\s*", str(s))
    return (int(m.group(1)), int(m.group(1))) if m else None


def compare(claim, ev, evidence_text, extent_text):
    """Findings a deterministic checker can make from two extractions.

    Every rule answers a question with a yes or a no. None of them is a
    judgement, and none of them needs to know what the claim means.
    """
    f = []
    hay = (evidence_text + " " + extent_text).lower()

    # a number asserted by the claim that the evidence never shows
    ev_nums = {n for n in (norm_num(x) for x in ev["numbers"] + ev["line_refs"]) if n is not None}
    for raw in claim["numbers"]:
        n = norm_num(raw)
        if n is None or n in ev_nums:
            continue
        # a sum of evidence numbers is not an unsupported number
        if any(abs(sum(p) - n) < 1e-6 for p in _pairs(ev_nums)):
            continue
        f.append(("number-not-in-evidence", f"claim says {raw}; evidence shows {sorted(ev_nums)}"))

    # a line reference the evidence does not cover
    ev_spans = [s for s in (line_span(x) for x in ev["line_refs"] + ev["numbers"]) if s]
    for raw in claim["line_refs"]:
        cs = line_span(raw)
        if not cs:
            continue
        if not any(a <= cs[0] <= b or a <= cs[1] <= b for a, b in ev_spans):
            f.append(("line-ref-not-in-evidence", f"claim cites {raw}; evidence covers {ev_spans[:6]}"))

    # a path the claim names that the extent never opened  <-- the scope check
    for p in claim["paths"]:
        base = p.split("/")[-1].strip("`.,;:")
        if base and base.lower() not in hay:
            f.append(("path-outside-extent", f"claim names {p}; the extent does not open it"))

    # A universal quantifier is NOT a finding on its own — nearly every careful claim
    # carries one, and firing on all of them was 3 of this harness's 3 false positives.
    # It only matters as an amplifier: a quantifier plus a concrete mismatch is the
    # shape of "right in substance, wrong in one clause".
    quant = next((q for q in claim["quantifiers"]
                  if q.lower() in ("only", "every", "never", "no", "none", "always", "nothing", "all")), None)
    if quant and f:
        f.append(("quantifier-amplifies", f"claim says “{quant}”, and a mismatch above breaks it"))

    # An identifier the claim leans on that appears nowhere in the evidence.
    #
    # Gated on the token LOOKING like code, because a 3B extractor returns prose
    # phrases here — "findings fields", "table", "proposition" — and every false
    # positive this harness produced on a clean case came from that. A path a
    # claim names is worth flagging on sight; a bare English word is not.
    for i in claim["identifiers"]:
        tok = i.strip("`()<>[]&:.,")
        if " " in tok or len(tok) <= 2:
            continue
        codeish = ("::" in tok) or ("_" in tok) or (tok != tok.lower() and tok != tok.upper())
        if not codeish:
            continue
        leaf = tok.split("::")[-1]
        if leaf.lower() not in hay and tok.lower() not in hay:
            f.append(("identifier-not-in-evidence", f"claim names `{i}`; evidence never shows it"))
    return f


def _pairs(nums):
    ns = sorted(nums)
    return [(a, b) for i, a in enumerate(ns) for b in ns[i + 1:]]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default=DEFAULT_URL)
    ap.add_argument("--reasoning-effort", default="high",
                    choices=["none", "minimal", "low", "medium", "high", "xhigh", "max"],
                    help="OpenRouter reasoning effort; 'none' omits the parameter")
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--only", action="append")
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("--max-tokens", type=int, default=1200)
    ap.add_argument("--no-think", action="store_true",
                    help="disable a reasoning model's thinking mode; required for Gemma 4")
    ap.add_argument("--out", default="extract_report.md")
    args = ap.parse_args()

    cases = [c for c in CASES if c.get("scope", "in") == "in"] + EXTRA_CASES
    if args.only:
        cases = [c for c in cases if c["id"] in args.only]

    rows = []
    for n, case in enumerate(cases, 1):
        print(f"[{n}/{len(cases)}] {case['id']}", end="", flush=True)
        extent_text = "\n".join(case["extent"])
        try:
            c_raw, t1 = ask(args.url, args.model, case["proposition"], args.timeout, args.max_tokens, args.no_think, args.reasoning_effort)
            e_raw, t2 = ask(args.url, args.model, extent_text + "\n" + case["output"],
                            args.timeout, args.max_tokens, args.no_think, args.reasoning_effort)
            c, e = parse(c_raw), parse(e_raw)
            if not (c and e):  # rambling past the JSON is the usual cause; a tighter cap stops it
                if not c:
                    c_raw, dt = ask(args.url, args.model, case["proposition"], args.timeout, args.max_tokens * 2, args.no_think, args.reasoning_effort)
                    c, t1 = parse(c_raw), t1 + dt
                if not e:
                    e_raw, dt = ask(args.url, args.model, extent_text + "\n" + case["output"],
                                    args.timeout, args.max_tokens * 2, args.no_think, args.reasoning_effort)
                    e, t2 = parse(e_raw), t2 + dt
            err = None if (c and e) else "unparsed extraction"
        except (urllib.error.URLError, OSError, KeyError) as ex:
            c = e = None; t1 = t2 = 0.0; err = f"{type(ex).__name__}: {ex}"

        findings = compare(c, e, case["output"], extent_text) if not err else []
        flagged = bool(findings)   # an unparsed extraction flags nothing, and that counts as a miss
        should = case["flaw"] is not None
        ok = flagged == should
        print(f"  {'ok  ' if ok else 'MISS'}  {len(findings)} finding(s)  {t1+t2:.0f}s")
        rows.append(dict(case_id=case["id"], should_flag=should, flagged=flagged, ok=ok,
                         findings=findings, flaw=case["flaw"], err=err,
                         claim_extract=c, evidence_extract=e, elapsed=t1 + t2))

    report(Path(args.out), args.model, rows)
    Path(args.out.replace(".md", ".json")).write_text(json.dumps(rows, indent=2))

    flawed = [r for r in rows if r["should_flag"]]      # every case counts, errors included
    clean = [r for r in rows if not r["should_flag"]]
    graded = [r for r in rows if not r["err"]]
    errs = [r for r in rows if r["err"]]
    print()
    print(f"model                  {args.model}")
    print(f"flawed cases flagged   {len([r for r in flawed if r['flagged']])} of {len(flawed)}"
          f"   (a miss here is a claim the loop would ship)")
    print(f"clean cases left alone {len([r for r in clean if not r['flagged']])} of {len(clean)}"
          f"   (a miss here costs a revision round)")
    print(f"extraction failed on   {len(errs)} of {len(rows)}"
          + (f"   ({', '.join(r['case_id'] for r in errs)})" if errs else ""))
    if graded:
        print(f"median latency         {sorted(r['elapsed'] for r in graded)[len(graded)//2]:.0f}s")


def report(path, model, rows):
    L = [f"# Extraction-plus-compare — `{model}`", "",
         "The model only lists what a text mentions. Every finding below was produced by",
         "`compare()`, which is code — so each one is decidable and re-derivable, not an opinion.", "",
         "| case | should flag | flagged | findings |", "|---|---|---|---|"]
    for r in rows:
        L.append(f"| `{r['case_id']}` | {'yes' if r['should_flag'] else 'no'} | "
                 f"{'yes' if r['flagged'] else 'no'} | {len(r['findings'])} |")
    L += ["", "## What the comparator found, per case", ""]
    for r in rows:
        L += [f"### `{r['case_id']}`", ""]
        if r["flaw"]:
            L += [f"*The real defect:* {r['flaw']}", ""]
        if r["err"]:
            L += [f"> extraction failed: {r['err']}", ""]
        elif not r["findings"]:
            L += ["> (no findings)", ""]
        else:
            L += [f"- **{k}** — {v}" for k, v in r["findings"]] + [""]
    path.write_text("\n".join(L))


if __name__ == "__main__":
    main()
