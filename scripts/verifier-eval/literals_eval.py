#!/usr/bin/env python3
"""What `verify.literals` would have said, over the corpus the gate used.

The two disagreement kinds shipped with numbers — 83% precision, 30% recall,
from `retrodict.py` over 125 claims at their first-render wording. The
`unevidenced` kind shipped with a disclaimer. This is the run that replaces it.

Same corpus, same reconstruction, same arm. A claim's wording is replayed to
its memo's first render, the evidence is the facts it cited *together with the
overlap set* (which is what `claim_subject` assembles at mint time), and the
question put to the model is the one `LITERALS_SYSTEM` asks.

# Why the prompt is read out of the Rust source

Not copied. `read_shipped_prompt` parses the `const LITERALS_SYSTEM` block out
of `src/verify.rs` and fails loudly if it cannot find it. The memo this
verifier came from already made the mistake of quoting a number earned by a
configuration other than the one that shipped, and a prompt pasted into a
harness is the same mistake with a longer fuse.

# Three measures, because one would not be honest on its own

  machine-refutation — of the literals the model raised, how many did a
    substring search find in the capture anyway. Needs no labels and no
    ground truth: the model's own claim is checkable, and this is the shipped
    filter's kill rate. It measures the model, not the feature.

  claim-level join — flagged against what the graders later concluded, in
    exactly the shape `retrodict.py` reports, so the number is comparable
    with the 83%/30% on the page. Read it knowing the denominator is wrong
    on purpose: a literal flag is about a clause, and "this claim later
    needed work" is about a claim, so a correct flag on a sound claim counts
    against precision here.

  literal-in-note — of the literals raised on claims a later pass did not
    simply support, how many appear verbatim in what the grader actually
    wrote. This is the measure at the right granularity: it asks whether the
    check named the thing the grader went on to object to. Reported at two
    minimum lengths, because a literal of one or two characters matches a
    grader's prose by accident.

    python3 literals_eval.py --populations        # reconstruct only, no calls
    python3 literals_eval.py --limit 10           # a cheap shakedown
    python3 literals_eval.py --repeat 3 --out literals3.json
    python3 literals_eval.py --summarise literals3.json
"""

import argparse, json, os, re, sys, time, urllib.request
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from retrodict import CORPUS, SELF, DEFAULT_URL, DEFAULT_MODEL, one_call  # noqa: E402
from extract_eval import auth_headers  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
VERIFY_RS = REPO / "src" / "verify.rs"
MAX_EVIDENCE_BYTES = 14_000          # `verify.rs`'s own constant, asserted below


def read_shipped_prompt():
    """`LITERALS_SYSTEM`, out of the source that ships it."""
    src = VERIFY_RS.read_text()
    m = re.search(r'const LITERALS_SYSTEM: &str = r#"(.*?)"#;', src, re.S)
    if not m:
        sys.exit("could not find `const LITERALS_SYSTEM` in src/verify.rs — "
                 "the harness refuses to measure a prompt it invented")
    b = re.search(r"const MAX_EVIDENCE_BYTES: usize = ([0-9_]+);", src)
    if not b or int(b.group(1).replace("_", "")) != MAX_EVIDENCE_BYTES:
        sys.exit("MAX_EVIDENCE_BYTES in src/verify.rs no longer matches this harness")
    return m.group(1)


# ---------------------------------------------------------------------------
# The corpus, reconstructed the way `retrodict.load_memo` does — but keeping
# `out_len`, which it discards and this needs. The shipped evidence is
# presented per observation and the quote relation searches each observation
# separately, so a loader that only keeps the joined output cannot reproduce
# either.
# ---------------------------------------------------------------------------

def observations(fact):
    """`Fact::observation_outputs` in Python, refusals included.

    Returns None on exactly the records the Rust refuses: a missing
    `out_len`, boundaries that do not account for the whole output, or a cut
    inside a character. A best-effort answer here would put text in front of
    the model that the shipped filter could never accept back.
    """
    out, at, blob = [], 0, fact["output"]
    raw = blob.encode()
    for e in fact["extent"]:
        n = e.get("out_len")
        if n is None:
            return None
        if n == 0:
            continue
        if out:
            at += 1                       # the separator `mint` joined with
        end = at + n
        if end > len(raw):
            return None
        try:
            out.append(raw[at:end].decode())
        except UnicodeDecodeError:
            return None
        at = end
    if at != len(raw):
        return None
    return out


def load_memo(memo):
    snap = os.path.join(CORPUS, memo + ".tetel")
    rows = [json.loads(l) for l in open(os.path.join(CORPUS, memo + ".evidence.jsonl"))]
    t0 = min(r["predicate"]["timestamp"] for r in rows)

    verdicts = defaultdict(list)
    for r in rows:
        p = r["predicate"]
        verdicts[r["subject"][0]["name"]].append((p["verdict"], p.get("note") or ""))

    at_t0, created = {}, {}
    for l in open(os.path.join(snap, "claims.jsonl")):
        d = json.loads(l)
        if d["event"] == "Withdraw":
            continue
        created.setdefault(d["id"], d["timestamp"])
        if d["timestamp"] <= t0:
            at_t0[d["id"]] = (d["prop"], d.get("from") or [])

    facts = {}
    for l in open(os.path.join(snap, "facts.jsonl")):
        d = json.loads(l)
        if d["event"] != "Create":
            continue
        f = {"output": d.get("output") or "", "extent": d.get("extent") or []}
        facts[d["id"]] = {
            "labels": [e.get("label", "") for e in f["extent"]],
            "keys": {e.get("key", "") for e in f["extent"]},
            # `collect` contributes nothing for a fact whose boundaries cannot
            # be trusted, rather than something unverifiable.
            "obs": observations(f) or [],
        }

    out = []
    for cid, (prop, cites) in at_t0.items():
        if cid not in verdicts or created.get(cid, 0) > t0:
            continue
        vs = [v for v, _ in verdicts[cid]]
        out.append(dict(
            memo=memo, id=cid, prop=prop, cites=cites, facts=facts,
            verdicts=verdicts[cid],
            supports_only=set(vs) == {"supports"},
            refuted="refutes" in vs,
        ))
    return out


def subject_ids(case):
    """`claim_subject`: the cited facts together with the overlap set."""
    facts, cited = case["facts"], list(case["cites"])
    union = set()
    for fid in cited:
        if fid in facts:
            union |= facts[fid]["keys"]
    extra = sorted(fid for fid, f in facts.items()
                   if fid not in cited and (f["keys"] & union))
    return cited + extra


def evidence_text(case):
    """`verify.rs::evidence_text`, per observation and per-observation budget."""
    labels, blob, budget, withheld = [], [], MAX_EVIDENCE_BYTES, 0
    for fid in subject_ids(case):
        f = case["facts"].get(fid)
        if not f:
            continue
        for lab in f["labels"]:
            labels.append(f"  - [{fid}] {lab}")
        for n, output in enumerate(f["obs"], 1):
            blob.append(f"--- {fid} observation {n} ---")
            raw = output.encode()
            if len(raw) <= budget:
                blob.append(output)
                budget -= len(raw)
                continue
            t = budget
            while t > 0:
                try:
                    cut = raw[:t].decode()
                    break
                except UnicodeDecodeError:
                    t -= 1
            else:
                cut = ""
            withheld += len(raw) - t
            blob.append(cut)
            blob.append(f"[... {len(raw) - t} bytes of captured output not shown]")
            budget = max(0, budget - t)
    if withheld:
        blob.append(f"\n[{withheld} bytes of captured output withheld in total "
                    "— this comparison saw a bounded view]")
    return ("EVIDENCE — what was opened or run:\n" + "\n".join(labels)
            + "\n\nEVIDENCE — captured output:\n" + "\n".join(blob))


NUMBER_WORDS = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight",
                "nine", "ten", "eleven", "twelve"]
PATH_SUFFIXES = [".rs", ".py", ".md", ".json", ".jsonl", ".toml", ".log", ".txt", ".sh", ".html"]
WORD_RE = re.compile(r"[0-9]|\b(?:" + "|".join(NUMBER_WORDS) + r")\b", re.I)


def is_quantity(literal):
    """`verify.rs::is_checkable` — a quantity or a path, not a bare name.

    Mirrored rather than shared, like the other two filters, because the
    harness has to be able to measure a build it is not linked against. The
    Rust is the authority; a test there pins the same cases this does.
    """
    low = literal.lower()
    if "/" in low or any(x in low for x in PATH_SUFFIXES):
        return True
    return bool(WORD_RE.search(literal))


def has_evidence(case):
    """Whether any fact in the subject contributed a usable observation."""
    return any(case["facts"].get(f, {}).get("obs") for f in subject_ids(case))


def containing(case, span):
    """`Subject::containing` — per observation, no normalisation."""
    if not span:
        return []
    return [fid for fid in subject_ids(case)
            if any(span in o for o in case["facts"].get(fid, {}).get("obs", []))]


# ---------------------------------------------------------------------------

def parse_literals(body):
    """`literal_findings`'s decode half. None is unparsable, never clean."""
    if not body:
        return None
    a, b = body.find("{"), body.rfind("}")
    if a < 0 or b < a:
        return None
    try:
        v = json.loads(body[a:b + 1])
    except Exception:
        return None
    rows = v.get("unevidenced")
    return rows if isinstance(rows, list) else None


def run_case(url, model, prompt, case, timeout, max_tokens, effort):
    user = f"TEXT:\n{case['prop']}\n\n{evidence_text(case)}"
    t = time.time()
    try:
        content, cost = one_call(url, model, prompt, user, timeout, max_tokens, effort)
    except Exception as e:
        return dict(memo=case["memo"], id=case["id"], status="error",
                    detail=f"{type(e).__name__}: {e}", cost=0.0)
    rows = parse_literals(content)
    if rows is None:
        return dict(memo=case["memo"], id=case["id"], status="unparsable",
                    detail=(content or "")[:200], cost=cost)

    raised, kept, not_verbatim, refuted, not_a_quantity = [], [], 0, 0, 0
    for r in rows:
        lit = (r.get("literal") or "") if isinstance(r, dict) else ""
        clause = (r.get("clause") or "") if isinstance(r, dict) else ""
        why = (r.get("why") or "") if isinstance(r, dict) else ""
        raised.append(lit)
        # Filter 1: the literal must be the author's own text.
        if not lit or lit not in case["prop"]:
            not_verbatim += 1
            continue
        # Filter 2: no observation shown may contain it. This is the whole
        # assertion, and it is the one the model gets wrong.
        where = containing(case, lit)
        if where:
            refuted += 1
            continue
        # Filter 3: it must name a quantity, not a symbol, flag, path or
        # quantifier. Instructed first and it did not hold — the model
        # stopped naming symbols and started naming quantifiers — so it is
        # mechanical.
        if not is_quantity(lit):
            not_a_quantity += 1
            continue
        kept.append(dict(literal=lit, clause=clause, why=why,
                         clause_quoted=bool(clause) and clause in case["prop"]))
    return dict(memo=case["memo"], id=case["id"], status="ok",
                prop=case["prop"], verdicts=case["verdicts"],
                supports_only=case["supports_only"], refuted_later=case["refuted"],
                # Whether anything was captured at all. On a claim whose facts
                # carry no usable observations — a legacy record with no
                # `out_len`, or a fact that captured nothing — "no capture
                # carries this literal" is trivially true of every literal in
                # the text, so the two populations cannot share a denominator.
                has_evidence=has_evidence(case),
                raised=len(rows), kept=kept,
                not_verbatim=not_verbatim, machine_refuted=refuted,
                not_a_quantity=not_a_quantity,
                cost=cost, elapsed=round(time.time() - t, 2))


# ---------------------------------------------------------------------------

def summarise(records):
    ok_all = [r for r in records if r["status"] == "ok"]
    bad = [r for r in records if r["status"] != "ok"]
    out = []
    out.append(f"cases            {len(records)}   ({len(ok_all)} ok, {len(bad)} not)")
    for st in sorted({r["status"] for r in bad}):
        out.append(f"  {st:<14} {sum(1 for r in bad if r['status'] == st)}")
    cost = sum(r.get("cost", 0.0) for r in records)
    out.append(f"cost             ${cost:.4f} total, ${cost / max(1, len(records)):.5f} each")
    if not ok_all:
        return "\n".join(out)

    blind = [r for r in ok_all if not r.get("has_evidence", True)]
    blind_claims = {(r["memo"], r["id"]) for r in blind}
    if blind:
        bk = sum(len(r["kept"]) for r in blind)
        out.append(
            f"\nEXCLUDED: {len(blind_claims)} claims ({len(blind)} draws) whose facts carry no "
            f"usable captured\n  output — legacy records with no `out_len`, or facts that captured "
            f"nothing. Every\n  literal in such a text is trivially unevidenced, so they cannot "
            f"share a\n  denominator with the rest. They produced {bk} surviving findings between "
            f"them —\n  reported at the end, not counted below.")
    draws = [r for r in ok_all if r.get("has_evidence", True)]
    if not draws:
        return "\n".join(out)

    raised = sum(r["raised"] for r in draws)
    refuted = sum(r["machine_refuted"] for r in draws)
    nv = sum(r["not_verbatim"] for r in draws)
    naq = sum(r.get("not_a_quantity", 0) for r in draws)
    kept = sum(len(r["kept"]) for r in draws)
    out.append("\nMACHINE-REFUTATION  (needs no ground truth)")
    out.append(f"  literals raised          {raised}")
    out.append(f"  not the author's words   {nv}")
    out.append(f"  not a quantity           {naq}")
    out.append(f"  in the capture after all {refuted}")
    out.append(f"  survived to the author   {kept}")
    if raised:
        out.append(f"  dropped by a filter      {100.0 * (refuted + nv + naq) / raised:.0f}% of what it raised")

    # Collapse draws to claims by majority vote, which is how the
    # retrodiction reported. A claim verified three times is one claim; a
    # literal raised in one draw of three is noise the author would meet on
    # a coin flip, and counting it as a finding would credit instability as
    # coverage.
    by_claim = defaultdict(list)
    for r in draws:
        by_claim[(r["memo"], r["id"])].append(r)
    ok, stable_total, stable_all = [], 0, 0
    for rs in by_claim.values():
        need = len(rs) // 2 + 1
        tally = defaultdict(int)
        for r in rs:
            for k in {k["literal"]: k for k in r["kept"]}.values():
                tally[k["literal"]] += 1
        majority = {lit for lit, n in tally.items() if n >= need}
        if tally:
            stable_total += len(tally)
            stable_all += sum(1 for n in tally.values() if n == len(rs))
        base = dict(rs[0])
        # One entry per distinct literal, keeping the first draw's wording.
        base["kept"] = list({k["literal"]: k
                             for r in rs for k in r["kept"]
                             if k["literal"] in majority}.values())
        ok.append(base)
    if len(records) > len(by_claim):
        out.append(f"\nSTABILITY  {stable_all}/{stable_total} distinct literals were raised in "
                   f"every draw of their claim;\n  the rest appeared in some draws and not others. "
                   f"Only majority literals count below.")

    # Claim-level join, in `retrodict.py`'s shape.
    flagged = [r for r in ok if r["kept"]]
    sound_flagged = [r for r in flagged if r["supports_only"]]
    worked_flagged = [r for r in flagged if not r["supports_only"]]
    ever_worked = [r for r in ok if not r["supports_only"]]
    missed = [r for r in ok if not r["kept"] and r["refuted_later"]]
    out.append("\nCLAIM-LEVEL JOIN  (comparable with the shipped 83%/30%, wrong denominator on purpose)")
    out.append(f"  claims                   {len(ok)}")
    out.append(f"  flagged                  {len(flagged)}")
    out.append(f"    later needed work      {len(worked_flagged)}")
    out.append(f"    later only supported   {len(sound_flagged)}   <- flags on claims already sound")
    out.append(f"  never flagged, later refuted  {len(missed)}")
    if flagged:
        out.append(f"  precision                {100.0 * len(worked_flagged) / len(flagged):.0f}%   ({len(worked_flagged)}/{len(flagged)})")
    if ever_worked:
        out.append(f"  recall                   {100.0 * len(worked_flagged) / len(ever_worked):.0f}%   ({len(worked_flagged)}/{len(ever_worked)})")

    # The measure at the right granularity.
    out.append("\nLITERAL-IN-GRADER-NOTE  (of literals raised on claims a pass did not simply support)")
    for minlen in (1, 3, 5):
        hits = total = 0
        for r in worked_flagged:
            notes = " ".join(n for _, n in r["verdicts"])
            for k in r["kept"]:
                if len(k["literal"]) < minlen:
                    continue
                total += 1
                hits += k["literal"] in notes
        if total:
            out.append(f"  literal >= {minlen} chars    {hits}/{total}   "
                       f"({100.0 * hits / total:.0f}% named in what the grader wrote)")
        else:
            out.append(f"  literal >= {minlen} chars    none")

    cq = [k for r in ok for k in r["kept"]]
    if cq:
        n = sum(1 for k in cq if k["clause_quoted"])
        out.append(f"\nCLAUSE FIDELITY   {n}/{len(cq)} surviving findings quoted the author's clause verbatim")

    if blind:
        br = sum(r["raised"] for r in blind)
        bk = sum(len(r["kept"]) for r in blind)
        rate = 100.0 * sum(1 for r in blind if r["kept"]) / len(blind)
        out.append(
            f"\nTHE BLIND POPULATION  (no captured output; excluded above)\n"
            f"  claims                   {len(blind_claims)}   ({len(blind)} draws)\n"
            f"  literals raised          {br}\n"
            f"  survived to the author   {bk}   <- the filter rejected none, and could not\n"
            f"  draws flagged            {rate:.0f}%\n"
            f"  Nothing here is a judgement about the check. With no capture, the filter that\n"
            f"  makes an `unevidenced` finding trustworthy has nothing to search, so it cannot\n"
            f"  reject anything. This is what the shipped code does on such a claim today.")
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repeat", type=int, default=1)
    ap.add_argument("--limit", type=int, default=0, help="first N cases only, for a shakedown")
    ap.add_argument("--prompt-file", help="a candidate prompt to measure instead of the shipped one. "
                                          "Iterating variants is what this is for; the shipped "
                                          "prompt stays the default so a run with no flags is "
                                          "always a run of what ships.")
    ap.add_argument("--cases-file", help="json list of {memo,id} — run only those claims. Screening "
                                         "a variant on the claims it already gets wrong is cheap; "
                                         "reporting that as its accuracy would not be honest, so "
                                         "the summary says when the population was chosen.")
    ap.add_argument("--with-evidence-only", action="store_true",
                    help="skip claims whose facts carry no usable captured output")
    ap.add_argument("--populations", action="store_true", help="reconstruct only, no model calls")
    ap.add_argument("--summarise", help="re-read a results json and print the summary")
    ap.add_argument("--url", default=DEFAULT_URL)
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--reasoning-effort", default="high")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--max-tokens", type=int, default=4000)
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()

    if a.summarise:
        print(summarise(json.load(open(a.summarise))["records"]))
        return

    memos = sorted(os.path.basename(p)[:-len(".tetel")]
                   for p in __import__("glob").glob(os.path.join(CORPUS, "*.md.tetel")))
    cases = [c for m in memos if m != SELF for c in load_memo(m)]
    cases.sort(key=lambda c: (c["memo"], int(c["id"][1:])))
    chosen = None
    if a.cases_file:
        want = {(x["memo"], x["id"]) for x in json.load(open(a.cases_file))}
        cases = [c for c in cases if (c["memo"], c["id"]) in want]
        chosen = os.path.basename(a.cases_file)
    if a.with_evidence_only:
        cases = [c for c in cases if has_evidence(c)]
    if a.limit:
        cases = cases[:a.limit]

    if a.populations:
        untrusted = sum(1 for c in cases for f in c["facts"].values() if not f["obs"])
        print(f"memos            {len(memos) - 1}")
        print(f"claims           {len(cases)}")
        print(f"  supports-only  {sum(1 for c in cases if c['supports_only'])}")
        print(f"  ever refuted   {sum(1 for c in cases if c['refuted'])}")
        print(f"facts with untrusted observation boundaries (contributing nothing): {untrusted}")
        ev = [len(evidence_text(c)) for c in cases]
        print(f"evidence bytes   min {min(ev)}, median {sorted(ev)[len(ev)//2]}, max {max(ev)}")
        return

    if a.prompt_file:
        prompt = Path(a.prompt_file).read_text().rstrip("\n")
        origin = f"CANDIDATE {os.path.basename(a.prompt_file)} — not what ships"
    else:
        prompt = read_shipped_prompt()
        origin = "shipped, read from src/verify.rs"
    print(f"prompt           {len(prompt)} bytes, {origin}", file=sys.stderr)
    print(f"cases            {len(cases)} x {a.repeat} draw(s)"
          + (f", chosen population: {chosen}" if chosen else ""), file=sys.stderr)

    work = [c for c in cases for _ in range(a.repeat)]
    records = []
    with ThreadPoolExecutor(max_workers=a.workers) as ex:
        futs = [ex.submit(run_case, a.url, a.model, prompt, c,
                          a.timeout, a.max_tokens, a.reasoning_effort) for c in work]
        for i, f in enumerate(futs, 1):
            r = f.result()
            records.append(r)
            if i % 10 == 0 or i == len(futs):
                print(f"  {i}/{len(futs)}  ${sum(x.get('cost', 0) for x in records):.4f}",
                      file=sys.stderr)

    payload = dict(model=a.model, repeat=a.repeat, effort=a.reasoning_effort,
                   prompt_bytes=len(prompt), prompt_origin=origin,
                   chosen_population=chosen, records=records)
    if chosen:
        print(f"\nNOTE: this ran on a chosen population ({chosen}), not the corpus. "
              f"The\nnumbers below compare variants on the claims that population selects "
              f"for. They\nare not this check's accuracy and must never be quoted as it.",
              file=sys.stderr)
    if a.out:
        json.dump(payload, open(a.out, "w"), indent=1)
        print(f"wrote {a.out}", file=sys.stderr)
    print(summarise(records))


if __name__ == "__main__":
    main()
