#!/usr/bin/env python3
"""The literal leg, asked of Jev: code proposes every candidate, Jev only judges.

`LITERALS_SYSTEM` already leaves the model one job: "is this literal an
assertion of current fact, and could a capture have carried it" — every
factual part of a finding is decided in code (docs/verify.md). The one thing
the LLM still does that code does not is *propose* the literals. Here code
does that too: every number, cardinal word and path in the claim, with the
word it counts ("918 seconds", "one glob", `acks.jsonl`), which is the shape
the LLM's own surviving literals take in literals_final_88x3.json.

Then the shipped filters, unchanged and in the shipped order — verbatim in
the text (true by construction), in no observation shown (`containing`), a
quantity or path (`is_quantity`) — and Jev answers two nouls per survivor, all
of a claim's candidates in one call:

  quantity  it is a quantity stated as current fact, or a file the text says
            it read — LITERALS_SYSTEM's definition and its never-report list,
            ported.
  carried   the capture carries the value in another form — LITERALS_SYSTEM's
            own examples (`14_000` backs "14,000 bytes"; a contradicted value
            is NOT carried).

Kept when P(quantity) >= Q and P(carried) < C. Every candidate's two
probabilities are stored, so thresholds re-score offline with no call.

Records use literals_eval.run_case's shape, and `literals_eval.summarise` —
the scorer that produced the LLM's 80% / 16% — scores them unchanged.

    python3 literals_jev.py --draws 1 --out literals_jev_d1.json
    python3 literals_jev.py --summarise literals_jev_d1.json --q 0.5 --c 0.5
"""

import argparse, json, re, sys, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import literals_eval as L  # noqa: E402
import jev  # noqa: E402
import gate_variants as GV  # noqa: E402
from data import DATA  # noqa: E402

BASELINE = DATA / "literals_final_88x3.json"

# A figure starts a token and ends on a word boundary, so a commit hash's
# digits and a `file.rs:117` location do not qualify as figures of their own.
FIGURE = re.compile(r"(?<![\w.:/#-])\d[\d,_]*(?:\.\d+)?%?(?![\w])"
                    r"|(?<![\w-])(?:" + "|".join(L.NUMBER_WORDS) + r")(?![\w-])", re.I)
PATH = re.compile(r"`?(?:[\w.-]+/)*[\w.-]+\.(?:" + "|".join(s.lstrip(".") for s in L.PATH_SUFFIXES)
                  + r")\b`?|`?(?:[\w.-]+/)+[\w.-]*`?")
NOUN = re.compile(r"\s+(\(?[A-Za-z][\w'-]*\)?)")


def candidates(text):
    """Every quantity- or path-shaped literal in `text`, with the word it counts."""
    out = []
    for m in FIGURE.finditer(text):
        lit = m.group(0)
        n = NOUN.match(text, m.end())
        if n and not n.group(1).startswith("("):
            lit = text[m.start():n.end()].strip()
        out.append(lit)
    for m in PATH.finditer(text):
        out.append(m.group(0).strip("`"))
    seen, uniq = set(), []
    for c in out:
        if c and c in text and c not in seen:
            seen.add(c)
            uniq.append(c)
    return uniq


def clause_of(text, lit):
    i = text.find(lit)
    for s in GV.sentences(text):
        j = text.find(s)
        if j <= i < j + len(s):
            for a, b in GV.clauses(s):
                if a <= i - j < b:
                    return s[a:b].strip()
            return s
    return text


QUANTITY_TRUE = ("it is a QUANTITY the text states as current fact — a value that could be wrong by "
                 "counting or arithmetic: a count, a size, a byte or line count, a duration, a "
                 "percentage or proportion, a threshold, an index range used as a measurement — or a "
                 "FILE the text says it read or that carries something")
QUANTITY_FALSE = ("it is not: a symbol, function, type, module or field name; a flag, option or setting "
                  "name; a version string; an identifier for a ticket, section, check or numbered "
                  "item; a line or byte range saying WHERE something is rather than HOW MUCH; a quoted "
                  "phrase the text discusses; a quantifier (any, every, no, only, always); a quantity "
                  "in what this design WILL build; a quantity inside a reason, a decision or an "
                  "entailment; or a number that measures nothing (\"two reasons\", \"one call\")")
CARRIED_TRUE = ("the captured evidence carries this value, possibly in another form: `14_000` backs "
                "\"14,000 bytes\"; a capture of lines 1-40 backs \"40 lines\"; `MAX_ATTEMPTS: u32 = "
                "3` backs \"retries three times\"; 5 of 6 visible backs \"83%\"")
CARRIED_FALSE = ("it does not: the value appears nowhere in what was captured, or the capture shows a "
                 "DIFFERENT value (two timestamps 910 apart do not carry \"918 seconds\"), or the "
                 "value may lie in material that was truncated and not shown")


# `--split` asks the three parts of QUANTITY as three questions. The single
# noul bundles nine exclusions into one `false`, and on the first screen the
# one it let through most was "a number that measures nothing" — "one call",
# "two event", "Two things" — which is ten of the 27 literals it kept.
SPLIT_Q = {
    "is_q": ("«{lit}» is a quantity (a count, size, byte or line count, duration, percentage, "
             "threshold, or a range used as a measurement) or a file path — not a name, flag, "
             "version, identifier, quantifier, or a range saying WHERE something is.",
             "it is a quantity or a file path", "it is not"),
    "measures": ("«{lit}», in the clause «{cl}», measures something in the code, the files, a tool's "
                 "output or the data — how many or how much of something observable. It is not "
                 "counting parts of the author's own text or argument (\"two things\", \"one "
                 "call\", \"the first of three\", \"four pieces\" of a design).",
                 "it measures something observable", "it counts the author's own discourse, or measures nothing"),
    "current": ("The clause «{cl}» states «{lit}» as a fact about how things are TODAY — not about "
                "what this design will build, and not inside a reason, decision or entailment.",
                "it is stated as current fact", "it is part of a proposal, a reason, a decision or an entailment"),
}


def questions_split(prop, lits):
    qs = {}
    for i, lit in enumerate(lits):
        cl = clause_of(prop, lit)
        for k, (ins, t, f) in SPLIT_Q.items():
            qs[f"{k}{i}"] = {"type": "noul", "criteria": {"true": t, "false": f},
                             "instructions": ins.format(lit=lit, cl=cl)}
        qs[f"c{i}"] = {"type": "noul", "criteria": {"true": CARRIED_TRUE, "false": CARRIED_FALSE},
                       "instructions": f"The captured evidence carries the value of «{lit}», as the "
                                       f"author's clause «{cl}» uses it."}
    return qs


def questions(prop, lits):
    qs = {}
    for i, lit in enumerate(lits):
        cl = clause_of(prop, lit)
        qs[f"q{i}"] = {"type": "noul", "criteria": {"true": QUANTITY_TRUE, "false": QUANTITY_FALSE},
                       "instructions": f"In the author's text, the literal «{lit}», in the clause "
                                       f"«{cl}», is a quantity stated as current fact or a file the "
                                       f"text says it read."}
        qs[f"c{i}"] = {"type": "noul", "criteria": {"true": CARRIED_TRUE, "false": CARRIED_FALSE},
                       "instructions": f"The captured evidence carries the value of «{lit}», as the "
                                       f"author's clause «{cl}» uses it."}
    return qs


def run_case(key, case, split=False):
    """literals_eval.run_case, with the LLM call replaced by code + Jev."""
    t = time.time()
    prop = case["prop"]
    cands = candidates(prop)
    judged, refuted, naq = [], 0, 0
    for lit in cands:
        if L.containing(case, lit):          # filter 2, shipped
            refuted += 1
            continue
        if not L.is_quantity(lit):           # filter 3, shipped
            naq += 1
            continue
        judged.append(lit)
    probs, cost = {}, 0.0
    if judged:
        state = f"TEXT:\n{prop}\n\n{L.evidence_text(case)}"
        try:
            a, cost, _ = GV.ask_chunked(state, (questions_split if split else questions)(prop, judged), key)
        except Exception as e:
            return dict(memo=case["memo"], id=case["id"], status="error",
                        detail=f"{type(e).__name__}: {e}", cost=0.0)
        for i, lit in enumerate(judged):
            if split:
                parts = {k: a.get(f"{k}{i}", {}).get("noul") for k in SPLIT_Q}
                # quantity = all three hold; min is the conjunction's weakest link
                probs[lit] = dict(parts, quantity=min((v for v in parts.values() if v is not None),
                                                      default=None),
                                  carried=a.get(f"c{i}", {}).get("noul"))
            else:
                probs[lit] = {"quantity": a.get(f"q{i}", {}).get("noul"),
                              "carried": a.get(f"c{i}", {}).get("noul")}
    return dict(memo=case["memo"], id=case["id"], status="ok", prop=prop,
                verdicts=case["verdicts"], supports_only=case["supports_only"],
                refuted_later=case["refuted"], has_evidence=L.has_evidence(case),
                raised=len(cands), candidates=probs, not_verbatim=0,
                machine_refuted=refuted, not_a_quantity=naq, kept=[],
                cost=cost, elapsed=round(time.time() - t, 2))


VALUE = re.compile(r"\d[\d,_]*(?:\.\d+)?")
_CASES = {}


def bare_value_in_capture(memo, cid, lit):
    """The literal's leading figure occurs as a whole token in some observation.

    Jev cannot do the arithmetic LITERALS_SYSTEM asks for ("two timestamps 910
    apart back '910 seconds'"), so "1690 seconds" survived against a capture
    that prints the 1690. Code can do the cheap half: if the bare value is in
    the capture as a token, drop it. It errs toward silence, as the shipped
    substring filter already does by design (docs/verify.md: "a `40` inside a
    line range will suppress a real finding about a different `40`").
    """
    m = VALUE.match(lit)
    if not m:
        return False
    if not _CASES:
        for mm in {x for x, _ in _CASES_WANT}:
            for c in L.load_memo(mm):
                _CASES[(c["memo"], c["id"])] = c
    case = _CASES.get((memo, cid))
    if not case:
        return False
    v = re.escape(m.group(0))
    pat = re.compile(rf"(?<![\w.]){v}(?![\w])")
    return any(pat.search(o) for f in L.subject_ids(case)
               for o in case["facts"].get(f, {}).get("obs", []))


_CASES_WANT = set()


def apply(records, q, c, value_filter=False):
    """Fill `kept` at thresholds (q, c) — offline, from stored probabilities."""
    if value_filter and not _CASES_WANT:
        _CASES_WANT.update((r["memo"], r["id"]) for r in records)
    out = []
    for r in records:
        r = dict(r)
        if r["status"] == "ok":
            r["kept"] = [dict(literal=lit, clause=clause_of(r["prop"], lit), why="",
                              clause_quoted=True)
                         for lit, p in r.get("candidates", {}).items()
                         if (p["quantity"] or 0) >= q and (p["carried"] if p["carried"]
                                                           is not None else 1) < c
                         and not (value_filter and bare_value_in_capture(r["memo"], r["id"], lit))]
        out.append(r)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--draws", type=int, default=1)
    ap.add_argument("--out")
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--summarise", nargs="+", help="results files (draws) to score together")
    ap.add_argument("--q", type=float, default=0.5)
    ap.add_argument("--c", type=float, default=0.5)
    ap.add_argument("--split", action="store_true", help="three judgement nouls per literal")
    ap.add_argument("--value-filter", action="store_true",
                    help="also drop a numeric literal whose bare value is a token in the capture")
    a = ap.parse_args()

    if a.summarise:
        recs = [r for p in a.summarise for r in json.load(open(p))["records"]]
        print(f"thresholds: P(quantity) >= {a.q}, P(carried) < {a.c}\n")
        print(L.summarise(apply(recs, a.q, a.c, a.value_filter)))
        return

    key = jev.api_key()
    # The baseline's own 88 claims, so the denominators are the same claims.
    want = {(r["memo"], r["id"]) for r in json.load(open(BASELINE))["records"]}
    memos = sorted({m for m, _ in want})
    cases = [c for m in memos for c in L.load_memo(m) if (c["memo"], c["id"]) in want]
    print(f"{len(cases)} of the baseline's {len(want)} claims", file=sys.stderr)
    records, cost = [], 0.0
    for d in range(a.draws):
        with ThreadPoolExecutor(max_workers=a.workers) as ex:
            for r in ex.map(lambda c: run_case(key, c, a.split), cases):
                r["rep"] = d
                records.append(r)
                cost += r.get("cost", 0.0)
    json.dump({"model": jev.MODEL, "repeat": a.draws, "records": records},
              open(a.out, "w"), indent=1)
    print(f"{a.out}: ${cost:.4f}, {len(records)} records, "
          f"{sum(r['status'] != 'ok' for r in records)} not ok", file=sys.stderr)


if __name__ == "__main__":
    main()
