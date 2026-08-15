#!/usr/bin/env python3
"""What `verify.verbs` would say about `fact` and `prose`, which ship off.

`claim` is on by default and has numbers behind it — 83% precision, 30%
recall from the retrodiction. The other two verbs have never been measured
on a real memo at all, and the design says so in as many words: `fact` is
"the one verb already half covered, deterministically and for free, by
`scope`" with a semantic residue "this design declines to say how large ...
because nothing measures it", and `prose` is "the least-evidenced comparison
of the three". This measures both.

# There is no verdict ledger for either, so this measures the other thing

The retrodiction could score `claim` because claims carry graded verdicts.
Facts and prose blocks carry none — an evidence row's subject is a claim id
— so recall against ground truth is not available here and this harness does
not pretend otherwise. What it measures instead is the quantity the design's
first kill condition is actually about: **flag rate on real, published,
already-grounded material**. A verb that flags a large share of prose seven
finished memos shipped is disqualified whatever its recall, because the
author pays attention per flag.

For `fact` there is one piece of real ground truth, and it is deterministic.
`scope`'s `attention` check already reports a note naming a location its own
extent does not cover. Cross-referencing the two says how much of the
verifier's output is the deterministic check's work done again, and how much
is the semantic residue nobody has sized. That residue is the whole argument
for turning `fact` on.

    python3 verbs_eval.py --populations
    python3 verbs_eval.py --verb fact  --limit 12
    python3 verbs_eval.py --verb fact  --out fact1.json
    python3 verbs_eval.py --verb prose --out prose1.json
"""

import argparse, json, os, re, sys, time
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from retrodict import CORPUS, SELF, DEFAULT_URL, DEFAULT_MODEL, one_call  # noqa: E402
from literals_eval import VERIFY_RS, MAX_EVIDENCE_BYTES, observations  # noqa: E402
from extract_eval import auth_headers  # noqa: E402


def json_object(body):
    """`verify.rs::json_object` — the largest brace-delimited substring."""
    if not body:
        return None
    a, b = body.find("{"), body.rfind("}")
    if a < 0 or b < a:
        return None
    try:
        return json.loads(body[a:b + 1])
    except Exception:
        return None


def shipped(const):
    """A `const NAME: &str = r#"..."#;` block, out of the source that ships it."""
    m = re.search(rf'const {const}: &str = r#"(.*?)"#;', VERIFY_RS.read_text(), re.S)
    if not m:
        sys.exit(f"could not find `const {const}` in src/verify.rs — the harness "
                 "refuses to measure a prompt it invented")
    return m.group(1)


# ---------------------------------------------------------------------------
# The corpus. Facts carry their current note (notes are revisable and the one
# on the page is the one a reader meets); prose blocks likewise carry their
# current text. Unlike the retrodiction there is nothing to replay to, because
# there are no verdicts to retrodict against.
# ---------------------------------------------------------------------------

def load(memo):
    snap = os.path.join(CORPUS, memo + ".tetel")
    facts = {}
    for l in open(os.path.join(snap, "facts.jsonl")):
        d = json.loads(l)
        if d["event"] == "Create":
            facts[d["id"]] = {
                "note": d.get("note") or "",
                "labels": [e.get("label", "") for e in (d.get("extent") or [])],
                "obs": observations({"output": d.get("output") or "",
                                     "extent": d.get("extent") or []}) or [],
            }
        elif d["event"] == "Revise" and d["id"] in facts:
            facts[d["id"]]["note"] = d.get("note") or ""

    claims = {}
    for l in open(os.path.join(snap, "claims.jsonl")):
        d = json.loads(l)
        if d["event"] == "Create":
            claims[d["id"]] = list(d.get("from") or [])
        elif d["event"] == "Revise" and d["id"] in claims and d.get("from"):
            claims[d["id"]] = list(d["from"])

    prose = {}
    order = []
    for l in open(os.path.join(snap, "prose.jsonl")):
        d = json.loads(l)
        i = d.get("id")
        if d["event"] == "Create":
            order.append(i)
            prose[i] = {"text": d.get("text") or "",
                        "cites": list(d.get("cite") or []),
                        "heading": bool(d.get("heading"))}
        elif d["event"] == "Revise" and i in prose:
            if d.get("text") is not None:
                prose[i]["text"] = d["text"]
            if d.get("cite"):
                prose[i]["cites"] = list(d["cite"])
    return facts, claims, [(i, prose[i]) for i in order]


def subjects(memo, verb):
    """`fact_subject` and `prose_subject` from `verify.rs`, over the corpus."""
    facts, claims, prose = load(memo)
    out = []
    if verb == "fact":
        for fid, f in facts.items():
            # `collect` contributes nothing for a fact whose observation
            # boundaries cannot be trusted, so such a fact has no captured
            # side at all and is not a comparison.
            if not f["obs"] or not f["note"].strip():
                continue
            out.append(dict(memo=memo, id=fid, verb="fact", text=f["note"],
                            ids=[fid], facts=facts))
    else:
        for pid, p in prose:
            if p["heading"] or not p["cites"] or not p["text"].strip():
                continue
            wanted = []
            for cid in p["cites"]:
                for f in claims.get(cid, []):
                    if f not in wanted:
                        wanted.append(f)
            if not any(facts.get(f, {}).get("obs") for f in wanted):
                continue
            out.append(dict(memo=memo, id=pid, verb="prose", text=p["text"],
                            ids=wanted, facts=facts))
    return out


def evidence_text(case):
    """`verify.rs::evidence_text` — per observation, per-observation budget."""
    labels, blob, budget, withheld = [], [], MAX_EVIDENCE_BYTES, 0
    for fid in case["ids"]:
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


def containing(case, span):
    if not span:
        return []
    return [f for f in case["ids"]
            if any(span in o for o in case["facts"].get(f, {}).get("obs", []))]


LABELS = ("current", "proposed", "argument")


def parse_assertions(body, text):
    v = json_object(body)
    rows = (v or {}).get("assertions") if isinstance(v, dict) else None
    if not isinstance(rows, list):
        return None, "classify reply had no `assertions` array"
    kept = []
    for r in rows:
        if not isinstance(r, dict):
            continue
        t, lab = r.get("text") or "", r.get("label") or ""
        if lab not in LABELS:
            return None, f"classify returned unknown label {lab!r}"
        if t and t in text:
            kept.append({"text": t, "label": lab})
    if not kept:
        return None, f"classify returned {len(rows)} assertion(s), none verbatim"
    return json.dumps({"assertions": kept}), None


def run_case(url, model, prompts, case, timeout, max_tokens, effort):
    classify, check = prompts
    t0, cost = time.time(), 0.0
    try:
        body, c = one_call(url, model, classify, f"CLAIM:\n{case['text']}",
                           timeout, max_tokens, effort)
        cost += c
        labelled, err = parse_assertions(body, case["text"])
        if err:
            return dict(memo=case["memo"], id=case["id"], verb=case["verb"],
                        status="unparsable", detail=err, cost=cost)
        user = (f"CLAIM:\n{case['text']}\n\nASSERTIONS:\n{labelled}\n\n"
                f"{evidence_text(case)}")
        body, c = one_call(url, model, check, user, timeout, max_tokens, effort)
        cost += c
    except Exception as e:
        return dict(memo=case["memo"], id=case["id"], verb=case["verb"],
                    status="error", detail=f"{type(e).__name__}: {e}", cost=cost)

    v = json_object(body)
    rows = (v or {}).get("disagreements") if isinstance(v, dict) else None
    if not isinstance(rows, list):
        return dict(memo=case["memo"], id=case["id"], verb=case["verb"],
                    status="unparsable", detail=(body or "")[:200], cost=cost)
    findings = []
    for r in rows:
        if not isinstance(r, dict) or r.get("kind") not in ("contradicts", "overreaches"):
            return dict(memo=case["memo"], id=case["id"], verb=case["verb"],
                        status="unparsable", detail=f"kind={r!r}"[:200], cost=cost)
        span = r.get("evidence") or ""
        clause = r.get("clause") or ""
        where = containing(case, span)
        findings.append(dict(kind=r["kind"], clause=clause, why=r.get("why") or "",
                             facts=where, quoted=bool(where),
                             clause_quoted=bool(clause) and clause in case["text"],
                             span=span if where else None,
                             rejected=None if where else (span or None)))
    return dict(memo=case["memo"], id=case["id"], verb=case["verb"], status="ok",
                text=case["text"], findings=findings, cost=cost,
                elapsed=round(time.time() - t0, 2))


# ---------------------------------------------------------------------------

def attention_flagged():
    """Facts `check` already reports as naming a location outside their extent.

    Read out of `tetel check`'s human-owed partition rather than
    reimplemented, so the deterministic baseline is the one that actually
    ships rather than this file's idea of it.
    """
    import subprocess
    out = defaultdict(set)
    tetel = Path(__file__).resolve().parents[2] / "target" / "debug" / "tetel"
    for memo in sorted(Path(CORPUS).glob("*.md")):
        if memo.name == SELF:
            continue
        try:
            r = subprocess.run([str(tetel), "check", str(memo)],
                               capture_output=True, text=True, timeout=120)
        except Exception:
            continue
        for line in (r.stdout + r.stderr).splitlines():
            m = re.match(r"\s*-\s+(F\d+): its note names ", line)
            if m:
                out[memo.name].add(m.group(1))
    return out


def summarise(records, verb):
    ok = [r for r in records if r["status"] == "ok"]
    bad = [r for r in records if r["status"] != "ok"]
    o = [f"verb             {verb}",
         f"subjects         {len(records)}   ({len(ok)} ok, {len(bad)} not)"]
    for st in sorted({r["status"] for r in bad}):
        o.append(f"  {st:<14} {sum(1 for r in bad if r['status'] == st)}")
    cost = sum(r.get("cost", 0.0) for r in records)
    o.append(f"cost             ${cost:.4f} total, ${cost / max(1, len(records)):.5f} each")
    if not ok:
        return "\n".join(o)

    flagged = [r for r in ok if r["findings"]]
    fs = [f for r in ok for f in r["findings"]]
    o.append("")
    o.append(f"FLAG RATE ON REAL, PUBLISHED, ALREADY-GROUNDED MATERIAL")
    o.append(f"  flagged          {len(flagged)}/{len(ok)}   ({100.0 * len(flagged) / len(ok):.0f}%)")
    o.append(f"  findings         {len(fs)}")
    for k in ("contradicts", "overreaches"):
        o.append(f"    {k:<13} {sum(1 for f in fs if f['kind'] == k)}")
    if fs:
        o.append(f"  quoted verbatim  {sum(1 for f in fs if f['quoted'])}/{len(fs)}")
        o.append(f"  clause verbatim  {sum(1 for f in fs if f['clause_quoted'])}/{len(fs)}")

    if verb == "fact":
        att = attention_flagged()
        att_all = {(m, f) for m, fset in att.items() for f in fset}
        flg = {(r["memo"], r["id"]) for r in flagged}
        seen = {(r["memo"], r["id"]) for r in ok}
        att_here = att_all & seen
        o.append("")
        o.append("AGAINST THE DETERMINISTIC CHECK THAT ALREADY SHIPS (`scope`'s attention)")
        o.append(f"  attention flags  {len(att_here)}/{len(seen)} of the same facts")
        o.append(f"  both agree       {len(flg & att_here)}")
        o.append(f"  attention only   {len(att_here - flg)}   <- semantic check missed what a substring test caught")
        o.append(f"  verifier only    {len(flg - att_here)}   <- THE RESIDUE: what a model adds over `scope`")
        o.append("  The residue is the whole argument for turning `fact` on. Every")
        o.append("  one of those is worth reading by hand before it becomes a number.")
    return "\n".join(o)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--verb", choices=["fact", "prose"], default="fact")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--populations", action="store_true")
    ap.add_argument("--summarise")
    ap.add_argument("--url", default=DEFAULT_URL)
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--reasoning-effort", default="high")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--max-tokens", type=int, default=4000)
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()

    if a.summarise:
        d = json.load(open(a.summarise))
        print(summarise(d["records"], d["verb"]))
        return

    import glob
    memos = sorted(os.path.basename(p)[:-len(".tetel")]
                   for p in glob.glob(os.path.join(CORPUS, "*.md.tetel")))
    cases = [c for m in memos if m != SELF for c in subjects(m, a.verb)]
    if a.limit:
        cases = cases[:a.limit]

    if a.populations:
        for verb in ("fact", "prose"):
            cs = [c for m in memos if m != SELF for c in subjects(m, verb)]
            ev = [len(evidence_text(c)) for c in cs] or [0]
            print(f"{verb:6} comparable subjects {len(cs):4}   "
                  f"evidence bytes: median {sorted(ev)[len(ev)//2]}, max {max(ev)}")
        return

    prompts = (shipped("CLASSIFY_SYSTEM"), shipped("CHECK_SYSTEM"))
    print(f"prompts          {len(prompts[0])}+{len(prompts[1])} bytes, "
          f"shipped, read from src/verify.rs", file=sys.stderr)
    print(f"subjects         {len(cases)} ({a.verb}), split configuration", file=sys.stderr)

    records = []
    with ThreadPoolExecutor(max_workers=a.workers) as ex:
        futs = [ex.submit(run_case, a.url, a.model, prompts, c,
                          a.timeout, a.max_tokens, a.reasoning_effort) for c in cases]
        for i, f in enumerate(futs, 1):
            records.append(f.result())
            if i % 10 == 0 or i == len(futs):
                print(f"  {i}/{len(futs)}  ${sum(x.get('cost', 0) for x in records):.4f}",
                      file=sys.stderr)
    if a.out:
        json.dump(dict(verb=a.verb, model=a.model, records=records), open(a.out, "w"), indent=1)
        print(f"wrote {a.out}", file=sys.stderr)
    print(summarise(records, a.verb))


if __name__ == "__main__":
    main()
