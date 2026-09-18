#!/usr/bin/env python3
"""The gate, iterated: several ways of asking Jev whether a subject is worth the check leg.

`gate_jev.py` asked it of the whole note at once and failed — AUC 0.52 on
`prose`. Reading the failures (README, "The gate before the check leg") showed
the lowest-scoring true catches were small, untruncated notes whose defect the
refuter scored at 0.76-0.94 once pointed at the clause. The context was all
there; the question asked for a search. Each variant here changes how the note
is PRESENTED so that less of the search is left to the model:

  whole   the baseline, `gate_jev.py`'s three nouls on the whole note. Score: `any`.
  split   the note cut into sentences mechanically, one noul per sentence, all
          in one call. Score: the max.
  pick    one `choice` over the sentences plus NONE, so the sentences compete
          for the probability instead of each being judged alone.
          Score: 1 - P(NONE).
  atoms   every checkable token — a number, a count word, a quantifier, a
          backticked name — posed as its own noul with the clause that carries
          it and its sentence. This is the refuter's advantage (being pointed at the
          clause) produced without an LLM. Score: the max.

Every variant sees the same state as `gate_jev.py` — the note and
`verbs_eval.evidence_text`, which is `verify.rs`'s 14KB view — so a difference
between variants is the presentation and nothing else.

    python3 gate_variants.py fact_v1.json --variant split --draws 3
    python3 gate_variants.py claims --variant pick --draws 3     # the claim verb
    python3 score_gate_variants.py fact split pick atoms

Draws land in `gate_runs/<verb>_<variant>_d<n>.json`.
"""

import argparse, json, re, sys, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import verbs_eval as V  # noqa: E402
import jev  # noqa: E402
import gate_jev as G  # noqa: E402

RUNS = HERE / "gate_runs"
MAX_QUESTIONS_PER_CALL = 40

DISAGREES = ("asserts something the captured evidence, or the author's own figures elsewhere in "
             "the text, show to be otherwise")


def state_for(case):
    # Byte-identical to gate_jev.gate_one's state, on purpose.
    return f"AUTHOR'S TEXT:\n{case['text']}\n\n{evidence_for(case)}"


def evidence_for(case):
    # A claim case carries its evidence pre-rendered (see claim_subjects).
    return case["evidence"] if "evidence" in case else V.evidence_text(case)


def claim_subjects():
    """The claim verb's population: `retrodict.py`'s first-render claim wordings.

    Each claim as it stood at its memo's first render, against the cited facts
    together with the overlap set — the `union` arm, which is what
    `verify.rs` assembles for a claim at mint time. The label is what a later
    grounding pass said: `refuted` is a defect a gate must not skip,
    `supports_only` is what it should, and `qualified` is neither and is
    reported separately rather than forced into either population.
    """
    import glob, os
    import retrodict as R
    memos = sorted(os.path.basename(p)[:-len(".evidence.jsonl")]
                   for p in glob.glob(os.path.join(R.CORPUS, "*.evidence.jsonl")))
    def render(c, arm):
        labels, blob = R.evidence_text(c, arm)
        return ("EVIDENCE — what was opened or run:\n" + "\n".join(f"  - {l}" for l in labels)
                + "\n\nEVIDENCE — captured output:\n" + blob)

    out = []
    for m in memos:
        if m == R.SELF:
            continue
        for c in R.load_memo(m):
            ev = render(c, "union")
            label = ("refuted" if c["refuted"] else "supports_only" if c["supports_only"]
                     else "qualified" if c["qualified"] else "other")
            out.append((m, {"id": c["id"], "text": c["prop"], "evidence": ev,
                            "evidence_cites": render(c, "cites"), "label": label}))
    return out


# A sentence ends at . ; or : followed by something that starts a sentence, or
# at a blank line. `src/prose.rs:211` does not split: no space follows the colon.
SPLIT = re.compile(r"(?<=[.;:])\s+(?=[A-Z`(*\"'])|\n\s*\n")


def sentences(text):
    out = [s.strip() for s in SPLIT.split(text) if s and s.strip()]
    return [s for s in out if len(s) > 12]


def ask_chunked(state, questions, key):
    """Jev takes a question map; keep each call to a bounded number of questions."""
    items = list(questions.items())
    answers, cost, tokens = {}, 0.0, 0
    for i in range(0, len(items), MAX_QUESTIONS_PER_CALL):
        a, c, u = jev.ask(state, dict(items[i:i + MAX_QUESTIONS_PER_CALL]), key=key, timeout=120)
        answers.update(a)
        cost += c
        tokens += (u or {}).get("input_tokens", 0)
    return answers, cost, tokens


def noul(instructions, true):
    return {"type": "noul", "instructions": instructions,
            "criteria": {"true": true, "false": "it does not. " + G.NOT_A_DISAGREEMENT}}


# ---------------------------------------------------------------- variants

def v_whole(case, key):
    a, cost, tokens = ask_chunked(state_for(case), G.QUESTIONS, key)
    parts = {k: a.get(k, {}).get("noul") for k in G.QUESTIONS}
    return {"score": parts["any"], "parts": parts}, cost, tokens


def v_split(case, key):
    sents = sentences(case["text"])
    qs = {f"s{i}": noul("This one sentence of the author's text disagrees with the captured "
                        f"evidence: it {DISAGREES}.\nSENTENCE: {s}",
                        "this sentence disagrees with the evidence")
          for i, s in enumerate(sents)}
    a, cost, tokens = ask_chunked(state_for(case), qs, key)
    ps = [a.get(f"s{i}", {}).get("noul") for i in range(len(sents))]
    return {"score": max(p for p in ps if p is not None),
            "parts": [{"text": s, "p": p} for s, p in zip(sents, ps)]}, cost, tokens


def v_pick(case, key):
    sents = sentences(case["text"])
    crit = {f"S{i + 1}": s for i, s in enumerate(sents)}
    crit["NONE"] = "No sentence disagrees with the evidence. " + G.NOT_A_DISAGREEMENT
    qs = {"which": {"type": "choice",
                    "instructions": "Which sentence of the author's text disagrees with the "
                                    f"captured evidence? A disagreeing sentence {DISAGREES}.",
                    "criteria": crit}}
    a, cost, tokens = ask_chunked(state_for(case), qs, key)
    probs = a.get("which", {}).get("probabilities") or {}
    return {"score": 1.0 - probs.get("NONE", 1.0),
            "parts": [{"text": s, "p": probs.get(f"S{i + 1}")} for i, s in enumerate(sents)]
            + [{"text": "NONE", "p": probs.get("NONE")}]}, cost, tokens


# Round 2. `pick` is the round-1 winner on `fact` (AUC 0.85, 54% skipped with
# no defect lost) and fails `prose` on two diagnosed causes, one lever each.
#
# RECALL. NOT_A_DISAGREEMENT was ported from CHECK_SYSTEM, whose job is
# precision — it decides what an author is WARNED about. A gate only decides
# whether that check runs, so its errors cost the other way round. The ported
# text says "the text says less than the evidence shows" is not a disagreement,
# and tet61 P4 is exactly that shape: a rule restated without the existential
# gate the code applies. Every variant scored it 0.14-0.29. The recall
# criteria keep only the one exclusion that is about what evidence CAN speak
# to — proposals — and name the omitted-condition shape as a disagreement.
RECALL_DISAGREES = (
    "states something about the code, a tool's output or a measurement that the captured "
    "evidence, or the author's own figures elsewhere in the text, show to be otherwise: a wrong "
    "count, number, name, range or line; a rule or behaviour described differently from how the "
    "code does it; a condition, case or exception the code applies that the sentence leaves out "
    "while presenting itself as the rule; or an every, only, never, exactly or same that the "
    "evidence shows to be false")
RECALL_NONE = ("No sentence does that. A sentence describing what the design PROPOSES to build or "
               "change is not a disagreement: evidence captured beforehand cannot contradict it.")

# CLASSIFY. The highest-scoring `prose` negatives under `pick` are sentences
# describing the proposed design ("the snapshot manifest gains a tenth entry").
# That is the cluster prose_v1_adjudicated.md found in the LLM check leg, and
# the classify step fixed it there (11 -> 0). Same idea, same call: one choice
# per sentence, and only the CURRENT share of a sentence counts toward the score.
KIND = {"CURRENT": "It describes the code, a tool's output or a measurement as it exists now — "
                   "something the captured evidence could confirm or contradict.",
        "PROPOSED": "It describes what this design will build, add or change. Evidence captured "
                    "before the design exists cannot contradict it.",
        "ARGUMENT": "It is reasoning, motivation, judgement or framing rather than a factual "
                    "statement about the code."}


def pick_with(disagrees, none, classify=False, unit="sentence", evidence_first=False,
              weight="current"):
    """`pick`, with one presentation lever changed at a time.

    classify        also ask, per unit, CURRENT / PROPOSED / ARGUMENT, and store
                    the three probabilities so a weighting can be re-scored
                    offline without another call.
    weight          how a unit's pick probability is discounted when classify is
                    on: "current" keeps P(CURRENT) (round 2 — too strict: an
                    argument sentence often carries an embedded fact), and
                    "notproposed" keeps 1 - P(PROPOSED), since only a proposal
                    is beyond what evidence can speak to.
    unit            "sentence", or "clause" — the sentence cut again at , ; :
                    and dashes, so the choice points at a smaller span.
    evidence_first  the capture before the author's text in the state.
    """
    def v(case, key):
        units = sentences(case["text"])
        if unit == "clause":
            units = [s[a:b].strip() for s in units for a, b in clauses(s)]
            units = [u for u in units if len(u) > 12]
        crit = {f"S{i + 1}": s for i, s in enumerate(units)}
        crit["NONE"] = none
        qs = {"which": {"type": "choice",
                        "instructions": f"Which {unit} of the author's text disagrees with the "
                                        f"captured evidence? A disagreeing {unit} {disagrees}.",
                        "criteria": crit}}
        if classify:
            for i, s in enumerate(units):
                qs[f"k{i + 1}"] = {"type": "choice", "criteria": KIND,
                                   "instructions": f"What kind of {unit} is this?\n"
                                                   f"{unit.upper()}: {s}"}
        state = (f"{evidence_for(case)}\n\nAUTHOR'S TEXT:\n{case['text']}" if evidence_first
                 else state_for(case))
        a, cost, tokens = ask_chunked(state, qs, key)
        probs = a.get("which", {}).get("probabilities") or {}
        parts = []
        for i, s in enumerate(units):
            kind = (a.get(f"k{i + 1}", {}).get("probabilities") or {}) if classify else {}
            w = (kind.get("CURRENT", 1.0) if weight == "current"
                 else 1.0 - kind.get("PROPOSED", 0.0))
            parts.append({"text": s, "p": probs.get(f"S{i + 1}"), "kind": kind, "w": w})
        score = (sum((p["p"] or 0.0) * p["w"] for p in parts) if classify
                 else 1.0 - probs.get("NONE", 1.0))
        return {"score": score, "parts": parts + [{"text": "NONE", "p": probs.get("NONE")}]}, \
            cost, tokens
    return v


NUM_WORDS = ("one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|"
             "fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|hundred|"
             "once|twice|single|both|half|dozen")
QUANTIFIERS = ("every|each|all|only|exactly|never|none|nothing|no|always|any|same|sole|whole|"
               "entire|neither|either|unique|identical|precisely")
ATOM = re.compile(rf"`(?!\s)[^`\n]{{1,80}}`"                               # a quoted name
                  rf"|(?<![\w/.:#-])\d[\d,._]*(?:\s*[-–]\s*\d[\d,._]*)?\b"  # a figure, not a location
                  rf"|(?<![\w`])(?:{NUM_WORDS}|{QUANTIFIERS})\b", re.I)

# Clause boundaries inside a sentence. Parentheses are not one: "two lines of a
# test fixture (…:117-119)" is only checkable with the range still attached.
CLAUSE = re.compile(r"(?:,|;|:| —| –)\s+")


def clauses(sentence):
    out, start = [], 0
    for m in CLAUSE.finditer(sentence):
        out.append((start, m.start()))
        start = m.end()
    out.append((start, len(sentence)))
    return out


def atoms(sentence):
    """Checkable tokens in a sentence, each with the clause that carries it.

    The clause, not a word window: "two lines" is wrong only against the
    "117-119" six words later. Line and column locations (`:211`, `rs:117`) are
    not atoms of their own — the lookbehind excludes them — because a note full
    of file:line citations would otherwise pose dozens of questions whose max
    rises with nothing but their count. They still reach the model inside the
    clause of the figure they qualify.
    """
    spans = clauses(sentence)
    out, seen = [], set()
    for m in ATOM.finditer(sentence):
        lo, hi = next(((a, b) for a, b in spans if a <= m.start() < b), (0, len(sentence)))
        phrase = sentence[lo:hi].strip()
        if (m.group(0).lower(), phrase) not in seen:
            seen.add((m.group(0).lower(), phrase))
            out.append((m.group(0), phrase))
    return out


def v_atoms(case, key):
    qs, meta = {}, []
    for s in sentences(case["text"]):
        for tok, phrase in atoms(s):
            n = len(meta)
            meta.append({"sentence": s, "atom": tok, "phrase": phrase})
            qs[f"a{n}"] = noul(
                f"SENTENCE: {s}\n\nWithin that sentence, look only at the words «{phrase}» and at "
                f"what «{tok}» asserts there — a count, a number, a range, a name, a quantity or a "
                f"scope. What it asserts disagrees with the captured evidence, or with the author's "
                f"own figures elsewhere in the text.",
                "what those words assert is contradicted")
    if not qs:
        return {"score": 0.0, "parts": []}, 0.0, 0
    a, cost, tokens = ask_chunked(state_for(case), qs, key)
    for n, m in enumerate(meta):
        m["p"] = a.get(f"a{n}", {}).get("noul")
    return {"score": max(m["p"] for m in meta if m["p"] is not None), "parts": meta}, cost, tokens


def cites_only(fn):
    """The same variant shown only the facts a claim cites, not the overlap set.

    The check leg reads the union because citations are author-controlled
    (retrodict.py's docstring). A gate is not the check: it only has to notice
    that something is off, and a defect in a claim is usually in what it cites.
    Fewer bytes to search is the lever; claim evidence runs 18KB at the median.
    """
    def v(case, key):
        return fn(dict(case, evidence=case.get("evidence_cites", case.get("evidence"))), key)
    return v


VARIANTS = {"whole": v_whole, "split": v_split, "pick": v_pick, "atoms": v_atoms,
            "pick_recall": pick_with(RECALL_DISAGREES, RECALL_NONE, classify=False),
            "pick_cls": pick_with(DISAGREES, "No sentence disagrees with the evidence. "
                                  + G.NOT_A_DISAGREEMENT, classify=True),
            "pick_both": pick_with(RECALL_DISAGREES, RECALL_NONE, classify=True),
            # round 3 — one lever each against `pick`
            "pick_prop": pick_with(DISAGREES, "No sentence disagrees with the evidence. "
                                   + G.NOT_A_DISAGREEMENT, classify=True, weight="notproposed"),
            "pick_clause": pick_with(DISAGREES, "No clause disagrees with the evidence. "
                                     + G.NOT_A_DISAGREEMENT, unit="clause"),
            "pick_evfirst": pick_with(DISAGREES, "No sentence disagrees with the evidence. "
                                      + G.NOT_A_DISAGREEMENT, evidence_first=True)}
# claim round — a claim is usually one long sentence, and its evidence is large
VARIANTS["split_cites"] = cites_only(v_split)
VARIANTS["pick_cites"] = cites_only(VARIANTS["pick"])
VARIANTS["pick_clause_cites"] = cites_only(VARIANTS["pick_clause"])


def run_one(fn, key, case):
    t0 = time.time()
    try:
        g, cost, tokens = fn(case, key)
    except Exception as e:
        # A gate that errors must not skip the call: no score, so it gates open.
        return {"error": f"{type(e).__name__}: {e}"}, 0.0, 0, time.time() - t0
    return g, cost, tokens, time.time() - t0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--variant", required=True, choices=sorted(VARIANTS))
    ap.add_argument("--draws", type=int, default=3)
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--limit", type=int, default=0)
    a = ap.parse_args()

    key = jev.api_key()
    if a.results == "claims":
        verb, found, subjects = "claim", {}, claim_subjects()
    else:
        d = json.load(open(a.results))
        verb = d["verb"]
        found = {(r["memo"], r["id"]): r for r in d["records"]}
        subjects = [(m, c) for m in sorted({r["memo"] for r in d["records"]})
                    for c in V.subjects(m, verb)]
    if a.limit:
        subjects = subjects[:a.limit]
    RUNS.mkdir(exist_ok=True)
    fn = VARIANTS[a.variant]

    for draw in range(1, a.draws + 1):
        rows, cost, lat, errors = [], 0.0, [], 0
        with ThreadPoolExecutor(max_workers=a.workers) as ex:
            futs = {ex.submit(run_one, fn, key, c): (m, c) for m, c in subjects}
            for fut in futs:
                m, c = futs[fut]
                g, cst, tokens, el = fut.result()
                cost += cst
                lat.append(el)
                errors += "error" in g
                r = found.get((m, c["id"]))
                rows.append({"memo": m, "id": c["id"], "gate": g, "input_tokens": tokens,
                             "label": c.get("label"),
                             "llm_status": r["status"] if r else "absent",
                             "llm_findings": [{"kind": f["kind"], "clause": f["clause"]}
                                              for f in (r["findings"] if r else [])]})
        lat.sort()
        out = RUNS / f"{verb}_{a.variant}_d{draw}.json"
        json.dump({"verb": verb, "variant": a.variant, "draw": draw, "source": a.results,
                   "model": jev.MODEL, "cost": cost, "n": len(rows), "errors": errors,
                   "latency_s": {"median": lat[len(lat) // 2], "max": lat[-1]},
                   "rows": rows}, open(out, "w"), indent=1)
        print(f"{out.name}: ${cost:.4f} over {len(rows)} subjects, {errors} errors, "
              f"median {lat[len(lat) // 2]:.2f}s", file=sys.stderr)


if __name__ == "__main__":
    main()
