#!/usr/bin/env python3
"""The classify leg, asked of Jev: can a model that emits no strings sort a claim?

`CLASSIFY_SYSTEM` asks an LLM to do two things: split a claim into assertions,
quoting each VERBATIM, and label each current / proposed / argument. The first
is string generation, which Jev cannot do — but the prompt's own rule ("you
are only sorting the author's own words") means the split never needs a model.
Here it is mechanical (`gate_variants.clauses` / `sentences`) and Jev does only
the labelling, one `choice` per unit, all units of a claim in one call.

There is no adjudicated ground truth for classify, so two measures:

  agreement   per character of claim text, Jev's label against the LLM
              classifier's majority label over retro_full125x3.json's three
              draws. Measures whether Jev can stand in for it — not whether
              either is right.

  decisive    the clauses where a label changes an outcome, from the
              adjudications. A clause carrying a CORRECT warning must reach
              the check labelled current, or the check is told it cannot
              speak to it and the warning dies. A clause that drew a
              proposal-read-as-current false alarm should not be current.
              The LLM classifier is scored by the same rule on the same
              clauses, so the comparison is like for like.

A clause counts as current when ANY unit overlapping it is labelled current:
that is what the check leg would see.

    python3 classify_jev.py --unit clause --draws 1
    python3 classify_jev.py --score          # every run in classify_runs/
"""

import argparse, collections, json, sys, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import jev  # noqa: E402
import gate_variants as GV  # noqa: E402

RUNS = HERE / "classify_runs"
LABELS = ("current", "proposed", "argument")

# CLASSIFY_SYSTEM's three definitions, verbatim (src/verify.rs).
CRITERIA = {
    "current": "asserts how the code, files or tools behave TODAY. Checkable against captured "
               "evidence.",
    "proposed": "asserts what THIS DESIGN will build, add, change, or recommend. The evidence was "
                "captured before that change exists, so it cannot speak to this.",
    "argument": "a reason, a decision, an entailment, or a statement about what is right or "
                "necessary. Nothing captured can settle it.",
}

# Decisive clauses from the adjudications: (memo prefix, claim id, clause
# substring, what the check must be told). `must` = a CORRECT warning sits
# here; `must_not` = a proposal-read-as-current false alarm sits here.
DECISIVE = [
    ("tet28", "C14", "a claim is only { id, proposition, cited fact ids, withdrawn }", "must"),
    ("tet30", "C12", "without inspecting a single character of prose or proposition text", "must"),
    ("tet30", "C3", "tet28's C13 was graded `qualifies` by two passes 918 seconds apart", "must"),
    ("tet30", "C7", "Both sides are produced by a `now_unix` that is byte-identical", "must"),
    ("tet47", "C18", "with no test or fixture depending on its wording", "must"),
    ("tet47", "C6", "Every evidence record already stores a pin", "must"),
    ("tet56", "C1", "with a clean working tree", "must"),
    ("tet56", "C3", "The `Search` observation's label is the only part of a grep census", "must"),
    ("tet56", "C9", "the returned bytes and the captured `output` are the same value `shown`", "must"),
    ("tet61", "C15", "the other 31 occurrences being its own definition", "must"),
    ("tet56", "C8", "seven of them (claim, fact, check, prose, ack, render, get) return bounded", "must_not"),
    ("tet56", "C11", "the bound and the label are both assembled inside the function", "must_not"),
    ("tet56", "C19", "how much the author was shown is unrecoverable from the ledger", "must_not"),
]


OPEN, CLOSE = "([{", ")]}"


def depth0_clauses(sentence):
    """`gate_variants.clauses`, but never inside brackets or a code span.

    The plain splitter cuts at every comma, so "{ id, proposition, cited fact
    ids, withdrawn }" became four one-word units and Jev labelled
    `proposition` on its own. An enumeration is one assertion.
    """
    out, start, depth, code, i = [], 0, 0, False, 0
    while i < len(sentence):
        ch = sentence[i]
        if ch == "`":
            code = not code
        elif not code and ch in OPEN:
            depth += 1
        elif not code and ch in CLOSE:
            depth = max(0, depth - 1)
        elif not code and depth == 0:
            m = GV.CLAUSE.match(sentence, i)
            if m and m.start() == i:
                out.append((start, i))
                start = m.end()
                i = m.end()
                continue
        i += 1
    out.append((start, len(sentence)))
    return out


def units_for(text, unit):
    sents = GV.sentences(text)
    if unit == "sentence":
        units = sents
    elif unit == "clause0":
        units = [s[a:b].strip() for s in sents for a, b in depth0_clauses(s)]
    else:
        units = [s[a:b].strip() for s in sents for a, b in GV.clauses(s)]
    units = [u for u in units if len(u) > 3]
    # Every unit is a verbatim span of the claim, by construction; say so.
    return [u for u in units if u in text]


def classify_one(key, text, unit):
    units = units_for(text, unit)
    if not units:
        return {"units": []}, 0.0
    qs = {f"u{i}": {"type": "choice", "criteria": CRITERIA,
                    "instructions": "You are given one claim from a software design memo (the "
                                    "state). Label ONE part of it by what that part asserts.\n"
                                    f"PART: {u}"}
          for i, u in enumerate(units)}
    answers, cost, _ = GV.ask_chunked(f"CLAIM:\n{text}", qs, key)
    out = []
    for i, u in enumerate(units):
        a = answers.get(f"u{i}", {})
        out.append({"text": u, "label": a.get("choice"), "p": a.get("probabilities")})
    return {"units": out}, cost


def run(unit, draws, workers):
    key = jev.api_key()
    subjects = GV.claim_subjects()
    RUNS.mkdir(exist_ok=True)
    for d in range(1, draws + 1):
        rows, cost, t0 = [], 0.0, time.time()
        with ThreadPoolExecutor(max_workers=workers) as ex:
            futs = {ex.submit(classify_one, key, c["text"], unit): (m, c) for m, c in subjects}
            for f in futs:
                m, c = futs[f]
                try:
                    g, cst = f.result()
                except Exception as e:
                    g, cst = {"error": f"{type(e).__name__}: {e}"}, 0.0
                cost += cst
                rows.append({"memo": m, "id": c["id"], "text": c["text"], **g})
        out = RUNS / f"claim_{unit}_d{d}.json"
        json.dump({"unit": unit, "draw": d, "cost": cost, "rows": rows}, open(out, "w"), indent=1)
        print(f"{out.name}: ${cost:.4f}, {len(rows)} claims, "
              f"{sum('error' in r for r in rows)} errors, {time.time() - t0:.0f}s", file=sys.stderr)


# ---------------------------------------------------------------- scoring

def jev_label(u, tau):
    """Argmax, or `current` whenever P(current) >= tau.

    Losing a correct warning (a current clause hidden from the check) costs
    more than keeping a proposal false alarm, so the current label can be
    given on less than a plurality. tau=None is the plain argmax.
    """
    p = u.get("p") or {}
    if tau is not None and p.get("current", 0.0) >= tau:
        return "current"
    return u.get("label")


def char_labels(text, spans):
    """A label per character of `text`, from (span, label) pairs; None where unlabelled."""
    lab = [None] * len(text)
    for s, l in spans:
        if not s or l not in LABELS:
            continue
        i = text.find(s)
        if i < 0:
            continue
        for j in range(i, i + len(s)):
            lab[j] = l
    return lab


def llm_labels():
    """The LLM classifier's majority label per character, over its three draws."""
    by = collections.defaultdict(list)
    for r in json.load(open(HERE / "retro_full125x3.json")):
        if isinstance(r.get("prop"), str) and r.get("assertions"):
            by[(r["memo"][:5], r["id"])].append(
                (r["prop"], [(a.get("text"), a.get("label")) for a in r["assertions"]]))
    out = {}
    for k, draws in by.items():
        text = draws[0][0]
        per = [char_labels(text, spans) for _, spans in draws]
        maj = []
        for j in range(len(text)):
            c = collections.Counter(p[j] for p in per if p[j])
            maj.append(c.most_common(1)[0][0] if c else None)
        out[k] = (text, maj)
    return out


def visible_current(text, lab, clause):
    """Would the check see `clause` as current? True if any char of it is labelled current."""
    i = text.find(clause)
    if i < 0:
        return None
    return any(lab[j] == "current" for j in range(i, i + len(clause)))


def score():
    llm = llm_labels()
    runs = sorted(RUNS.glob("claim_*_d*.json"))
    if not runs:
        print("no runs")
        return
    print(f"LLM classifier: {len(llm)} claims with a majority labelling "
          f"(retro_full125x3.json, 3 draws)\n")
    print(f"  {'run':<22} {'agree':>6} {'cur->cur':>9} {'LLM cur, Jev not':>17} "
          f"{'must':>6} {'must_not':>9}  cost")
    base = {"must": 0, "must_not": 0}
    for k in DECISIVE:
        memo, cid, clause, want = k
        t, lab = llm.get((memo, cid), (None, None))
        if t is None:
            continue
        v = visible_current(t, lab, clause)
        base[want] += (v is True) if want == "must" else (v is False)
    n_must = sum(1 for k in DECISIVE if k[3] == "must")
    n_not = len(DECISIVE) - n_must
    print(f"  {'LLM (majority of 3)':<22} {'—':>6} {'—':>9} {'—':>17} "
          f"{base['must']:>3}/{n_must:<2} {base['must_not']:>5}/{n_not:<3}")
    for p, tau in [(p, t) for p in runs for t in (None, 0.4, 0.3)]:
        d = json.load(open(p))
        agree = tot = cur_both = llm_cur_not = 0
        mine = {}
        for r in d["rows"]:
            k = (r["memo"][:5], r["id"])
            lab = char_labels(r["text"], [(u["text"], jev_label(u, tau)) for u in r.get("units", [])])
            mine[k] = (r["text"], lab)
            if k not in llm:
                continue
            _, ll = llm[k]
            for a, b in zip(lab, ll):
                if a and b:
                    tot += 1
                    agree += a == b
                    cur_both += a == b == "current"
                    llm_cur_not += b == "current" and a != "current"
        dec = {"must": 0, "must_not": 0}
        missing = []
        for memo, cid, clause, want in DECISIVE:
            t, lab = mine.get((memo, cid), (None, None))
            v = visible_current(t, lab, clause) if t else None
            if v is None:
                missing.append(f"{memo} {cid}")
                continue
            dec[want] += (v is True) if want == "must" else (v is False)
        llm_cur = cur_both + llm_cur_not
        name = p.stem + ("" if tau is None else f" t{tau}")
        print(f"  {name:<22} {agree / max(tot, 1):6.0%} {cur_both / max(llm_cur, 1):9.0%} "
              f"{llm_cur_not / max(llm_cur, 1):17.0%} {dec['must']:>3}/{n_must:<2} "
              f"{dec['must_not']:>5}/{n_not:<3}  ${d['cost']:.4f}"
              + (f"   (clause not found: {', '.join(missing)})" if missing else ""))
    print("\n  agree     share of characters both label, labelled the same")
    print("  cur->cur  of what the LLM calls current, the share Jev also calls current")
    print("  must      correct warnings whose clause still reaches the check as current")
    print("  must_not  proposal false alarms whose clause Jev keeps away from the check")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--unit", choices=["clause", "clause0", "sentence"], default="clause0")
    ap.add_argument("--draws", type=int, default=1)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--score", action="store_true")
    a = ap.parse_args()
    if a.score:
        score()
    else:
        run(a.unit, a.draws, a.workers)


if __name__ == "__main__":
    main()
