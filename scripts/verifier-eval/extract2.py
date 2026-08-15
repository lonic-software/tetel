#!/usr/bin/env python3
"""Association extraction: pull BINDINGS, not flat lists.

The flat-list version reached a ceiling that no model could lift. Every one of
its six remaining misses had perfect extraction on both sides and failed in
`compare()`, because a set of tokens cannot answer the questions the defects
actually pose:

    claim says   text : String            evidence says  text : Option<String>
    claim says   scope @ 210-213          evidence says  scope @ 194-197
    claim covers "the repository"         extent covers  "src/"
    claim says   "only two refusals"      evidence shows  three

All four are about what a name is BOUND to — a type, a line, a count, a scope.
Flat lists throw the binding away and keep the tokens, so `String`, `Option`
and `serde` all "appear" and nothing fires.

So the model is asked for pairs, and `compare()` asks whether two bindings of
the same name disagree. That is still a decidable question answered in code —
the model reads, the comparator decides — which is what keeps a refusal built
on this format-level rather than an opinion.

    python3 extract2.py                       # gpt-5.6-luna via OpenRouter
    python3 extract2.py --repeat 3
"""

import argparse, json, re, sys, time, urllib.request, urllib.error
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from cases import CASES            # noqa: E402
from extra_cases import EXTRA_CASES  # noqa: E402
from extract_eval import auth_headers  # noqa: E402

DEFAULT_URL = "https://openrouter.ai/api/v1/chat/completions"
DEFAULT_MODEL = "openai/gpt-5.6-luna"

SYSTEM = """Read the text and report what it binds, as one JSON object. Never judge whether anything is true.

{"bindings": [{"name": "", "kind": "", "value": "", "line": ""}],
 "covers": "",
 "quantifiers": [],
 "paths": [],
 "counts": [{"what": "", "n": 0}]}

bindings — every place the text ties a name to something. One entry per binding:
    name   the identifier, field, constant or function
    kind   one of: type, value, line, definition, mention
    value  what it is bound to — the type, the literal, the thing it is said to be
    line   the line number or range this binding sits at, if the text gives one

covers — what the text says it ranges over: a directory, a repository, a file, a
    function. The scope of a search or of a claim. Empty if it names none.

quantifiers — only, every, never, all, none, no, always, nothing.

paths — file paths or file names.

counts — where the text states or shows how many of something there are.

Report only what the text contains. Reply with the JSON object alone."""


def ask(url, model, text, timeout, max_tokens, effort):
    body = {"model": model,
            "messages": [{"role": "system", "content": SYSTEM},
                         {"role": "user", "content": f"TEXT:\n{text}"}],
            "temperature": 0.0, "max_tokens": max_tokens}
    if effort != "none":
        body["reasoning"] = {"effort": effort}
    req = urllib.request.Request(url, data=json.dumps(body).encode(), headers=auth_headers())
    t0 = time.time()
    with urllib.request.urlopen(req, timeout=timeout) as r:
        d = json.load(r)
    return d["choices"][0]["message"].get("content") or "", time.time() - t0, (d.get("usage") or {}).get("cost", 0.0)


def parse(s):
    m = re.search(r"\{.*\}", s, re.S)
    if not m:
        return None
    try:
        d = json.loads(m.group(0))
    except json.JSONDecodeError:
        return None
    out = {"bindings": [], "covers": "", "quantifiers": [], "paths": [], "counts": []}
    for b in d.get("bindings") or []:
        if isinstance(b, dict) and b.get("name"):
            out["bindings"].append({k: str(b.get(k, "")).strip() for k in ("name", "kind", "value", "line")})
    out["covers"] = str(d.get("covers") or "").strip()
    for f in ("quantifiers", "paths"):
        v = d.get(f) or []
        out[f] = [str(x).strip() for x in v] if isinstance(v, list) else []
    for c in d.get("counts") or []:
        if isinstance(c, dict) and c.get("what") is not None:
            out["counts"].append({"what": str(c.get("what")), "n": c.get("n")})
    return out


def norm(s):
    return re.sub(r"[`\s]", "", str(s or "")).strip(".,;:").lower()


def spans(v):
    out = []
    for tok in re.findall(r"\d+\s*[-:]\s*\d+|\d+", str(v or "")):
        p = re.split(r"[-:]", tok)
        out.append((int(p[0]), int(p[-1])))
    return out



WORD_NUM = {"zero":0,"one":1,"two":2,"three":3,"four":4,"five":5,"six":6,"seven":7,
            "eight":8,"nine":9,"ten":10,"eleven":11,"twelve":12}
WRAPPERS = {"option", "vec", "result", "box", "arc", "rc", "cow", "hashmap", "btreemap"}
NEGATORS = ("neither", "nor", "not ", "no ", "never", "without", "lacks", "missing", "plain", "required")


def numbers_in(v):
    """Numeric content, magnitude suffixes and number-words resolved."""
    t = str(v).lower().replace(",", "")
    out = []
    for m in re.finditer(r"(\d+(?:\.\d+)?)\s*([kmb])?", t):
        n = float(m.group(1))
        n *= {"k": 1e3, "m": 1e6, "b": 1e9}.get(m.group(2) or "", 1)
        out.append(n)
    for w, n in WORD_NUM.items():
        if re.search(rf"\b{w}\b", t):
            out.append(float(n))
    return out


def is_codeish(v):
    """Looks like a type or expression rather than a sentence about one."""
    t = str(v)
    return bool(re.search(r"[<>(){}\[\];:&|]|::|\w+\.\w+", t)) or (
        len(t.split()) <= 2 and any(c.isupper() for c in t[1:]))


def type_tokens(v):
    return {w.lower() for w in re.findall(r"[A-Za-z_][A-Za-z0-9_]*", str(v))}


def values_conflict(a, b):
    """Do two bindings of one name actually disagree?

    Designed against the pairs the string-equality version produced, where 6 of 6
    false positives were a prose description compared against a code detail —
    "an explicit array of ten entries" against "[&str; 10]" is the same fact in
    two registers, not a contradiction. Comparing registers is the bug; the fix
    is to only compare things that are comparable.
    """
    na, nb = norm(a), norm(b)
    if not na or not nb or na == nb:
        return None
    if na in nb or nb in na:
        return None                      # one is a fuller statement of the other

    # numbers are comparable across registers: "13.0M" vs "13,003,879" agree
    xa, xb = numbers_in(a), numbers_in(b)
    if xa and xb:
        if any(abs(p - q) <= max(abs(p), abs(q)) * 0.01 for p in xa for q in xb):
            return None                  # some number matches within 1%
        return f"claim says {xa[0]:g}; evidence shows {xb[0]:g}"

    ca, cb = is_codeish(a), is_codeish(b)

    # prose on one side, code on the other: only a NEGATION bridges the registers.
    # "plain required String, with neither Option nor a serde default" denies the
    # very wrapper the evidence shows, and that is a real contradiction.
    if ca != cb:
        prose, code = (b, a) if ca else (a, b)
        pl = str(prose).lower()
        if any(g in pl for g in NEGATORS):
            denied = {t for t in type_tokens(code) if t in WRAPPERS and t in pl} or \
                     {t for t in type_tokens(code) if t in WRAPPERS}
            if denied and any(g in pl for g in ("neither", "nor", "not ", "no ", "without", "plain", "required")):
                return (f"claim describes it as '{str(prose)[:60]}' while the evidence shows "
                        f"{sorted(denied)} in '{str(code)[:40]}'")
        return None                      # otherwise: two registers, not comparable

    if not (ca and cb):
        return None                      # two prose descriptions: paraphrase, not conflict

    # both code-shaped: a wrapper on one side and not the other is a real difference
    ta, tb = type_tokens(a), type_tokens(b)
    wa, wb = ta & WRAPPERS, tb & WRAPPERS
    if wa != wb:
        return f"claim's type is '{a}'; the evidence's is '{b}'"
    return None


UNIVERSAL = {"only", "every", "never", "all", "none", "no", "always", "nothing"}


def compare(c, e, evidence_text, extent_text):
    f = []
    hay = (evidence_text + " " + extent_text).lower()

    # 1. the same name bound to two different things
    ev_by_name = {}
    for b in e["bindings"]:
        ev_by_name.setdefault(norm(b["name"]), []).append(b)
    for cb in c["bindings"]:
        matches = ev_by_name.get(norm(cb["name"]))
        if not matches:
            continue
        if cb["value"]:
            # conflict only if EVERY evidence binding of this name conflicts —
            # one compatible reading is enough to stay silent
            reasons = [values_conflict(cb["value"], m["value"]) for m in matches if m["value"]]
            if reasons and all(r is not None for r in reasons):
                f.append(("binding-mismatch", f"{cb['name']}: {reasons[0]}"))
        # a name pinned to a line the evidence puts elsewhere
        cl, el = spans(cb["line"]), [s for m in matches for s in spans(m["line"])]
        if cl and el and not any(a <= x <= b or a <= y <= b for x, y in cl for a, b in el):
            f.append(("binding-at-wrong-line",
                      f"claim puts {cb['name']} at {cb['line']}; evidence puts it at "
                      f"{', '.join(m['line'] for m in matches if m['line'])}"))

    # 2. the claim ranges wider than the evidence does
    if c["covers"] and e["covers"] and norm(c["covers"]) != norm(e["covers"]):
        cw, ew = c["covers"].lower(), e["covers"].lower()
        wider = any(w in cw for w in ("repo", "crate", "project", "worktree", "codebase", "anywhere", "whole"))
        narrower = any(w in ew for w in ("src", "/", "file", "directory", "dir"))
        if wider and narrower:
            f.append(("claim-ranges-wider-than-evidence",
                      f"claim covers '{c['covers']}'; the evidence covers only '{e['covers']}'"))

    # 3. a stated count the evidence contradicts
    for cc in c["counts"]:
        for ec in e["counts"]:
            if norm(cc["what"]) and norm(cc["what"]) in norm(ec["what"]) or norm(ec["what"]) in norm(cc["what"]):
                if cc["n"] is not None and ec["n"] is not None and cc["n"] != ec["n"]:
                    f.append(("count-mismatch",
                              f"claim says {cc['n']} {cc['what']}; evidence shows {ec['n']}"))

    # 4. a path the claim reasons about that the extent never opened
    for p in c["paths"]:
        base = p.split("/")[-1].strip("`.,;:")
        if base and base.lower() not in hay:
            f.append(("path-outside-extent", f"claim names {p}; the extent does not open it"))

    # 5. a universal claim standing over evidence that shows an exception branch.
    #    Only ever an amplifier — on its own it fired on every careful claim.
    quant = next((q for q in c["quantifiers"] if q.lower() in UNIVERSAL), None)
    if quant and f:
        f.append(("quantifier-amplifies", f"claim says '{quant}', and a mismatch above breaks it"))
    return f


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default=DEFAULT_URL)
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--reasoning-effort", default="high")
    ap.add_argument("--repeat", type=int, default=1)
    ap.add_argument("--only", action="append")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--max-tokens", type=int, default=4000)
    ap.add_argument("--out", default="extract2_report.md")
    a = ap.parse_args()

    cases = [c for c in CASES if c.get("scope", "in") == "in"] + EXTRA_CASES
    if a.only:
        cases = [c for c in cases if c["id"] in a.only]

    rows, spend = [], 0.0
    for n, case in enumerate(cases, 1):
        for rep in range(a.repeat):
            ext = "\n".join(case["extent"])
            try:
                cr, t1, k1 = ask(a.url, a.model, case["proposition"], a.timeout, a.max_tokens, a.reasoning_effort)
                er, t2, k2 = ask(a.url, a.model, ext + "\n" + case["output"], a.timeout, a.max_tokens, a.reasoning_effort)
                cx, ex = parse(cr), parse(er)
                err = None if (cx and ex) else "unparsed"
                spend += (k1 or 0) + (k2 or 0)
            except Exception as ex_:
                cx = ex_2 = None; ex = None; t1 = t2 = 0; err = f"{type(ex_).__name__}: {ex_}"
            findings = compare(cx, ex, case["output"], ext) if not err else []
            should, flagged = case["flaw"] is not None, bool(findings)
            ok = should == flagged
            print(f"[{n}/{len(cases)}{'' if a.repeat == 1 else f' r{rep+1}'}] {case['id']}  "
                  f"{'ok  ' if ok else 'MISS'}  {len(findings)} finding(s)  {t1+t2:.0f}s", flush=True)
            rows.append(dict(case_id=case["id"], should_flag=should, flagged=flagged, ok=ok,
                             findings=findings, flaw=case["flaw"], err=err,
                             claim=cx, evidence=ex, elapsed=t1 + t2))

    Path(a.out.replace(".md", ".json")).write_text(json.dumps(rows, indent=2))
    flawed = [r for r in rows if r["should_flag"]]
    clean = [r for r in rows if not r["should_flag"]]
    errs = [r for r in rows if r["err"]]
    print(f"\nmodel                  {a.model}   (reasoning={a.reasoning_effort}, repeat={a.repeat})")
    print(f"flawed cases flagged   {len([r for r in flawed if r['flagged']])} of {len(flawed)}")
    print(f"clean cases left alone {len([r for r in clean if not r['flagged']])} of {len(clean)}")
    print(f"extraction failed on   {len(errs)} of {len(rows)}")
    print(f"cost                   ${spend:.4f}")


if __name__ == "__main__":
    main()
