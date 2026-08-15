#!/usr/bin/env python3
"""The retrodiction test the mint-warning design gates itself on.

Reconstructs every claim's wording *as it stood at its memo's first render*,
runs the verifier over it against the evidence that claim cited, and asks two
questions: did it flag, and — for the claims a later pass refuted — did the
flag name the same defect.

The corpus is the seven memos under `docs/design/`, whose claim logs record
every Create and Revise with a timestamp, so a claim's text at any past moment
is exactly recoverable. T0 for each memo is its earliest evidence-row
timestamp; a claim created after T0 has no first-render wording to feed the
verifier and leaves every denominator rather than counting as a miss.

Two kill conditions, declared in the design before this was written:

  * 7 or more of the 62 supports-only claims flagged  -> steering hazard
  * 2 or fewer of the 9 refuted claims surfaced with a matching defect
    -> does not catch what forces a revision round

`--arm cites` compares against the facts the claim cited. `--arm union` adds
the overlap set — every uncited fact sharing an extent key with a cited one,
which is what `claims::overlap_report` computes at mint time. The design
chose `union` on the argument that citations are author-controlled and so
break the completeness half of scope.rs's independence; running both is how
that argument gets tested rather than assumed.

    python3 retrodict.py --populations      # reconstruct only, no model calls
    python3 retrodict.py --arm cites
    python3 retrodict.py --arm union
"""

import argparse, json, glob, os, re, sys, time, urllib.request
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from direct_eval import SYSTEM, parse  # noqa: E402  — the same prompt the eval measured
from extract_eval import auth_headers  # noqa: E402

SCOPE_SYSTEM = """You compare one claim an author wrote against the evidence a tool captured for it, and you report only DISAGREEMENTS.

There are exactly two kinds of disagreement:

  contradicts — the captured evidence shows something incompatible with what the claim asserts:
                a different number, name, type, line, or behaviour.
  overreaches — the claim ranges wider than what was captured. It says "every", "never", "only",
                "no", "always", "any" or "cannot" about a population the evidence samples rather
                than covers; or it reasons about a file, symbol or behaviour the evidence never
                touched.

A DESIGN MEMO SAYS TWO KINDS OF THING IN THE SAME VOICE, and only one of them is checkable:

  what the code does TODAY                  — checkable against the captured evidence
  what THIS DESIGN WILL MAKE IT DO          — NOT checkable, because the evidence was captured
                                              before the change the memo exists to propose

The absence, from the captured code, of something this design proposes to ADD is NEVER a
disagreement. If a claim describes a section, field, check, verb, entry or behaviour that the
design introduces, the captured code will not contain it — that is expected and correct, not a
contradiction. For example, a claim that "a `## Transplants` section renders beside the
modification-target section" describes what the design builds; finding no such section in the
current renderer is not a finding. Report a contradiction ONLY where the claim describes CURRENT
behaviour and the evidence shows that behaviour differing.

Nothing else is a disagreement. In particular:

  * Evidence that does not fully ESTABLISH the claim is NOT a disagreement. A design memo's claims
    routinely rest on more than their citations show; reporting that is noise, not a finding.
  * A claim about what SHOULD be built, what a design ought to do, or what follows logically from
    an argument is not checkable against captured bytes. Report nothing for it.
  * "The captured material does not touch X" is not a disagreement. That is insufficiency, which
    this check does not report, however tempting it is to phrase it as a missing scope.
  * A claim asserting LESS than the evidence shows is not a disagreement.
  * Prose describing code is not a disagreement with the code.
  * Your own uncertainty is not a disagreement.

The captured output may have been truncated, and says so where it was. Never report a disagreement
that rests on material you were not shown.

For each disagreement name the exact failing clause of the claim, and quote the span of captured
evidence that shows it. Both must be VERBATIM — copied character for character from the text above,
never paraphrased or reconstructed. A finding whose quotation cannot be found in the evidence is
worse than no finding at all, because it sends the reader to check against text that does not exist.

Reply with one JSON object and nothing else:
{"disagreements": [{"kind": "contradicts"|"overreaches", "clause": "", "evidence": "", "why": ""}]}

An empty list is the common and correct answer. Return it whenever neither kind is present."""

JUDGE_SYSTEM = """You are given two structured records, extracted independently from two texts.

CLAIM holds what an author asserted in a design memo. EVIDENCE holds what a tool captured — files
that were opened, commands that were run, and what they returned.

Report only DISAGREEMENTS. A disagreement is a name bound to two things that cannot both be true,
a number the claim states that the evidence contradicts, a scope the claim ranges over that is
wider than the evidence covers, or a count the claim states that the evidence shows differently.

The same fact stated two ways is NOT a disagreement. A claim saying less than the evidence shows is
not a disagreement. A claim describing what the design WILL build is not contradicted by evidence
of code that predates it. Insufficiency is not a disagreement.

Reply with one JSON object and nothing else:
{"disagreements": [{"kind": "", "clause": "", "evidence": "", "why": ""}]}

An empty list is the common, correct answer."""

CLASSIFY_SYSTEM = """You are given one claim from a software design memo. Split it into its separate
assertions and label each.

  current   — asserts how the code, files or tools behave TODAY. Checkable against captured evidence.
  proposed  — asserts what THIS DESIGN will build, add, change, or recommend. The evidence was
              captured before that change exists, so it cannot speak to this.
  argument  — a reason, a decision, an entailment, or a statement about what is right or necessary.
              Nothing captured can settle it.

One sentence often carries more than one assertion, with different labels. Split them.

Quote each assertion VERBATIM from the claim — character for character, never paraphrased, never
merged, never invented. You are only sorting the author's own words.

Reply with one JSON object and nothing else:
{"assertions": [{"text": "", "label": "current"|"proposed"|"argument"}]}"""

CHECK_SYSTEM_FILTERED = """You are given assertions an author made about how a system behaves TODAY,
and the evidence a tool captured. Report only DISAGREEMENTS.

There are exactly two kinds:

  contradicts — the captured evidence shows something incompatible with the assertion: a different
                number, name, type, line, or behaviour.
  overreaches — the assertion ranges wider than what was captured. It says "every", "never", "only",
                "no", "always", "any" or "cannot" about a population the evidence samples rather
                than covers.

Nothing else is a disagreement. In particular:

  * Evidence that does not fully ESTABLISH an assertion is NOT a disagreement.
  * "The captured material does not touch X" is NOT a disagreement. That is insufficiency phrased
    as a missing scope, and it is still insufficiency.
  * An assertion saying LESS than the evidence shows is not a disagreement.
  * Prose describing code is not a disagreement with the code.
  * Your own uncertainty is not a disagreement.

The captured output may have been truncated, and says so where it was. Never report a disagreement
resting on material you were not shown.

For each disagreement, name the failing assertion and quote the span of captured evidence that shows
it. Both VERBATIM — copied character for character. A finding whose quotation cannot be found in the
evidence is worse than none.

Reply with one JSON object and nothing else:
{"disagreements": [{"kind": "contradicts"|"overreaches", "clause": "", "evidence": "", "why": ""}]}

An empty list is the common and correct answer."""

CHECK_SYSTEM = """You are given a claim from a design memo, a labelling of its assertions, and the
evidence a tool captured. Report only DISAGREEMENTS.

Each assertion carries one of three labels:

  current   — asserts how the system behaves TODAY. THESE ARE THE ONLY ONES YOU MAY REPORT AGAINST.
  proposed  — asserts what this design will build. The evidence predates it, so its absence from the
              captured code is expected and is never a finding.
  argument  — a reason, decision or entailment. Nothing captured can settle it.

You are shown all three because a contradiction often needs the others for context: a bound the
design *recommends* can be what makes a *current* assertion about byte counts wrong, and you cannot
see that if you only read the current ones. Use every label to understand the claim; report only
where the failing assertion is labelled `current`.

There are exactly two kinds:

  contradicts — the captured evidence shows something incompatible with the assertion: a different
                number, name, type, line, or behaviour.
  overreaches — the assertion ranges wider than what was captured. It says "every", "never", "only",
                "no", "always", "any" or "cannot" about a population the evidence samples rather
                than covers.

Nothing else is a disagreement. In particular:

  * Evidence that does not fully ESTABLISH an assertion is NOT a disagreement. Reporting that is
    noise, not a finding.
  * "The captured material does not touch X" is NOT a disagreement. That is insufficiency phrased
    as a missing scope, and it is still insufficiency.
  * An assertion saying LESS than the evidence shows is not a disagreement.
  * Prose describing code is not a disagreement with the code.
  * Your own uncertainty is not a disagreement.

The captured output may have been truncated, and says so where it was. Never report a disagreement
resting on material you were not shown.

For each disagreement, name the failing assertion and quote the span of captured evidence that shows
it. Both VERBATIM — copied character for character. A finding whose quotation cannot be found in the
evidence is worse than none, because it sends the reader to check against text that does not exist.

Reply with one JSON object and nothing else:
{"disagreements": [{"kind": "contradicts"|"overreaches", "clause": "", "evidence": "", "why": ""}]}

An empty list is the common and correct answer."""

CORPUS = "/Volumes/SSD/Documents/lonic/tetel/docs/design"
SELF = "tet-verifier-mint-warning.md"  # the memo proposing this; not part of its own corpus
DEFAULT_URL = "https://openrouter.ai/api/v1/chat/completions"
DEFAULT_MODEL = "openai/gpt-5.6-luna"


def load_memo(memo):
    """Every claim's first-render wording, its cites, and the verdicts it later drew."""
    snap = os.path.join(CORPUS, memo + ".tetel")
    rows = [json.loads(l) for l in open(os.path.join(CORPUS, memo + ".evidence.jsonl"))]
    t0 = min(r["predicate"]["timestamp"] for r in rows)

    verdicts = defaultdict(list)
    for r in rows:
        p = r["predicate"]
        verdicts[r["subject"][0]["name"]].append((p["verdict"], p.get("note") or ""))

    # Replay the claim log up to T0. The last Create/Revise at or before it is
    # the wording a verifier would have seen at first render.
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
        if d["event"] == "Create":
            facts[d["id"]] = {
                "output": d.get("output") or "",
                "labels": [e.get("label", "") for e in d.get("extent", [])],
                "keys": {e.get("key", "") for e in d.get("extent", [])},
            }

    out = []
    for cid, (prop, cites) in at_t0.items():
        if cid not in verdicts:
            continue                       # never graded — nothing to retrodict against
        if created.get(cid, 0) > t0:
            continue                       # no first-render wording
        vs = [v for v, _ in verdicts[cid]]
        out.append(dict(
            memo=memo, id=cid, prop=prop, cites=cites, facts=facts,
            verdicts=verdicts[cid],
            supports_only=set(vs) == {"supports"},
            refuted="refutes" in vs,
            qualified="qualifies" in vs,
        ))
    return out


def overlap_ids(case):
    """`claims::overlap_report` in Python: uncited facts sharing an extent key."""
    facts, cited = case["facts"], set(case["cites"])
    union = set()
    for fid in cited:
        if fid in facts:
            union |= facts[fid]["keys"]
    return sorted(fid for fid, f in facts.items() if fid not in cited and (f["keys"] & union))


def evidence_text(case, arm, budget=14000):
    ids = list(case["cites"])
    if arm == "union":
        ids += overlap_ids(case)
    labels, outputs = [], []
    for fid in ids:
        f = case["facts"].get(fid)
        if not f:
            continue
        labels += f["labels"]
        if f["output"]:
            outputs.append(f"--- {fid} ---\n{f['output']}")
    blob = "\n".join(outputs)
    if len(blob) > budget:                 # truncation is disclosed, never silent
        blob = blob[:budget] + f"\n[... {len(blob) - budget} bytes of captured output not shown]"
    return labels, blob


def one_call(url, model, system, user, timeout, max_tokens, effort):
    """One completion, retried upward if reasoning consumed the whole budget."""
    spent, cap, content = 0.0, max_tokens, ""
    for _ in range(3):
        body = {"model": model,
                "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
                "temperature": 0.0, "max_tokens": cap}
        if effort != "none":
            body["reasoning"] = {"effort": effort}
        req = urllib.request.Request(url, data=json.dumps(body).encode(), headers=auth_headers())
        with urllib.request.urlopen(req, timeout=timeout) as r:
            d = json.load(r)
        spent += (d.get("usage") or {}).get("cost", 0.0) or 0.0
        ch = d["choices"][0]
        content = ch["message"].get("content") or ""
        if ch.get("finish_reason") != "length" and content.strip():
            break
        cap *= 3
    return content, spent


def judge(url, model, case, arm, timeout, max_tokens, effort):
    """Approach C on a real memo: extract the claim, extract the evidence, compare the two.

    The two extractions never learn they are related — that separation is the
    approach's whole bet, and the reason its findings are about structure rather
    than about a fluent claim's persuasiveness.
    """
    from extract2 import SYSTEM as EXTRACT_SYSTEM  # local: only this arm needs it
    labels, blob = evidence_text(case, arm)
    cx, c1 = one_call(url, model, EXTRACT_SYSTEM, "TEXT:\n" + case["prop"],
                      timeout, max_tokens, effort)
    ev, c2 = one_call(url, model, EXTRACT_SYSTEM,
                      "TEXT:\n" + "\n".join(labels) + "\n" + blob, timeout, max_tokens, effort)
    out, c3 = one_call(url, model, JUDGE_SYSTEM,
                       "CLAIM:\n" + cx + "\n\nEVIDENCE:\n" + ev, timeout, max_tokens, effort)
    return out, c1 + c2 + c3, blob


def split_check(url, model, case, arm, timeout, max_tokens, effort, context="full"):
    """Classify the claim's assertions, then check only the checkable ones.

    The previous single-prompt attempt merged both questions and the model
    generalised "absence of what the design adds is not a finding" into near
    silence — it cleared all four false positives and lost five of eight real
    ones. Separating them keeps the disagreement check simple, and makes the
    classification an inspectable artefact rather than a judgement buried
    inside a verdict.
    """
    labels, blob = evidence_text(case, arm)
    raw, c1 = one_call(url, model, CLASSIFY_SYSTEM, "CLAIM:\n" + case["prop"],
                       timeout, max_tokens, effort)
    assertions = []
    m = re.search(r"\{.*\}", raw, re.S)
    if m:
        try:
            for x in (json.loads(m.group(0)).get("assertions") or []):
                if isinstance(x, dict) and x.get("text"):
                    assertions.append({"text": str(x["text"]),
                                       "label": str(x.get("label", "")).strip().lower()})
        except json.JSONDecodeError:
            pass
    current = [x["text"] for x in assertions if x["label"] == "current"]
    if not current:
        # Nothing checkable in this claim. That is a real answer, not a failure:
        # a claim made entirely of proposals and arguments has nothing a
        # capture can disagree with.
        return '{"disagreements": []}', c1, blob, assertions
    # `filtered` sends only the `current` assertions; `full` sends the whole
    # claim with the labelling beside it. Filtering alone cost a real finding:
    # a claim whose 32,768-byte bound was labelled `proposed` and whose byte
    # counts were labelled `current` had its contradiction severed, because
    # neither half is wrong without the other. Both kept so the difference can
    # be measured rather than assumed.
    if context == "filtered":
        user = ("ASSERTIONS ABOUT CURRENT BEHAVIOUR:\n" +
                "\n".join(f"  - {t}" for t in current) +
                "\n\nEVIDENCE — what was opened or run:\n" + "\n".join(f"  - {x}" for x in labels) +
                f"\n\nEVIDENCE — captured output:\n{blob}")
        out, c2 = one_call(url, model, CHECK_SYSTEM_FILTERED, user, timeout, max_tokens, effort)
        return out, c1 + c2, blob, assertions
    user = ("THE CLAIM, IN FULL:\n" + case["prop"] +
            "\n\nHOW ITS ASSERTIONS CLASSIFY:\n" +
            "\n".join(f"  [{x['label']:8}] {x['text']}" for x in assertions) +
            "\n\nEVIDENCE — what was opened or run:\n" + "\n".join(f"  - {x}" for x in labels) +
            f"\n\nEVIDENCE — captured output:\n{blob}")
    out, c2 = one_call(url, model, CHECK_SYSTEM, user, timeout, max_tokens, effort)
    return out, c1 + c2, blob, assertions


def parse_disagreements(s):
    """The scope question's reply: a list, possibly empty. None means unparsed."""
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
            out.append({k: str(x.get(k, "")) for k in ("kind", "clause", "evidence", "why")})
    return out


def quotes(span, blob):
    """`Fact::quotes` in miniature — is the span verbatim in the captured output?

    Byte-exact and unnormalised, as `facts.rs` is, because the whole value of a
    quotation is that it can be found. Whitespace is the one concession: the
    model reflows long spans across lines and that is a rendering difference,
    not a different quotation.
    """
    n = lambda t: re.sub(r"\s+", " ", t or "").strip()
    return bool(n(span)) and n(span) in n(blob)


def ask(url, model, case, arm, timeout, max_tokens, effort, question="support"):
    labels, blob = evidence_text(case, arm)
    user = (f"CLAIM:\n{case['prop']}\n\n"
            f"EVIDENCE — what was opened or run:\n" + "\n".join(f"  - {x}" for x in labels) +
            f"\n\nEVIDENCE — captured output:\n{blob}")
    system = SCOPE_SYSTEM if question == "scope" else SYSTEM
    spent, cap = 0.0, max_tokens
    content = ""
    for _ in range(3):
        body = {"model": model,
                "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
                "temperature": 0.0, "max_tokens": cap}
        if effort != "none":
            body["reasoning"] = {"effort": effort}
        req = urllib.request.Request(url, data=json.dumps(body).encode(), headers=auth_headers())
        with urllib.request.urlopen(req, timeout=timeout) as r:
            d = json.load(r)
        spent += (d.get("usage") or {}).get("cost", 0.0) or 0.0
        ch = d["choices"][0]
        content = ch["message"].get("content") or ""
        if ch.get("finish_reason") != "length" and content.strip():
            break
        cap *= 3
    return content, spent, blob


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arm", choices=["cites", "union"], default="cites")
    ap.add_argument("--repeat", type=int, default=1, help="runs per case; the findings shuffle between runs")
    ap.add_argument("--split-context", choices=["filtered", "full"], default="full",
                    help="what the check sees: only the `current` assertions, or the whole claim with labels")
    ap.add_argument("--subset", help="json file of {memo,id} objects — run only those cases")
    ap.add_argument("--question", choices=["support", "scope", "judge", "split"], default="support",
                    help="`support`: does the evidence state the claim (the original, a truth check). "
                         "`scope`: does the claim contradict or overreach what was captured.")
    ap.add_argument("--populations", action="store_true", help="reconstruct only, no model calls")
    ap.add_argument("--url", default=DEFAULT_URL)
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--reasoning-effort", default="high")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--max-tokens", type=int, default=4000)
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()

    memos = sorted(os.path.basename(p)[: -len(".evidence.jsonl")]
                   for p in glob.glob(os.path.join(CORPUS, "*.evidence.jsonl")))
    memos = [m for m in memos if m != SELF]
    cases = [c for m in memos for c in load_memo(m)]
    if a.subset:
        want = {(x["memo"], x["id"]) for x in json.loads(Path(a.subset).read_text())}
        cases = [c for c in cases if (c["memo"], c["id"]) in want]

    sup = [c for c in cases if c["supports_only"]]
    ref = [c for c in cases if c["refuted"]]
    qual = [c for c in cases if c["qualified"]]
    print(f"corpus            {len(memos)} memos")
    print(f"graded claims with a first-render wording   {len(cases)}")
    print(f"  supports-only   {len(sup)}   (design says 62)")
    print(f"  ever refuted    {len(ref)}   (design says 9)")
    print(f"  ever qualified  {len(qual)}  (design says 60)")
    if a.populations:
        return

    out_path = a.out or f"retrodict_{a.question}_{a.arm}.json"
    spend = 0.0
    print(f"\nquestion: {a.question}   arm: {a.arm} — {len(cases)} claims, {a.workers} at a time\n", flush=True)

    def run_one(job):
        i, case, rep = job
        findings, verdict, note, err = [], None, "", None
        try:
            assertions = None
            if a.question == "split":
                raw, cost, blob, assertions = split_check(a.url, a.model, case, a.arm, a.timeout,
                                                          a.max_tokens, a.reasoning_effort,
                                                          a.split_context)
            elif a.question == "judge":
                raw, cost, blob = judge(a.url, a.model, case, a.arm, a.timeout,
                                        a.max_tokens, a.reasoning_effort)
            else:
                raw, cost, blob = ask(a.url, a.model, case, a.arm, a.timeout, a.max_tokens,
                                      a.reasoning_effort, a.question)
            if a.question in ("scope", "judge", "split"):
                findings = parse_disagreements(raw)
                err = None if findings is not None else "unparsed"
                findings = findings or []
                # Every quoted span checked against the captured output, which is
                # what C20 specifies before a finding may carry a quotation.
                for f in findings:
                    f["quote_verified"] = quotes(f.get("evidence"), blob)
                flagged = bool(findings)
                note = "; ".join(f"{f['kind']}: {f['why']}" for f in findings)
            else:
                verdict, note = parse(raw)
                err = None if verdict else "unparsed"
                flagged = verdict in ("refutes", "qualifies")
        except Exception as ex:
            cost, flagged, assertions = 0.0, False, None
            err = f"{type(ex).__name__}: {ex}"
        print(f"[{i+1}/{len(work)}] {case['memo'][:22]:24} {case['id']:5} "
              f"{'FLAG' if flagged else '    '} {verdict or (str(len(findings)) + ' dis.') or err}",
              flush=True)
        return dict(memo=case["memo"], id=case["id"], prop=case["prop"],
                    supports_only=case["supports_only"], refuted=case["refuted"],
                    qualified=case["qualified"], verdict=verdict, note=note, err=err,
                    flagged=flagged, cost=cost, findings=findings, assertions=assertions, rep=rep,
                    later=[{"verdict": v, "note": n} for v, n in case["verdicts"]])

    work = [(i, c, r) for i, (c, r) in enumerate((c, r) for c in cases for r in range(a.repeat))]
    with ThreadPoolExecutor(max_workers=a.workers) as ex:
        results = list(ex.map(run_one, work))
    spend = sum(r["cost"] for r in results)

    answered = [r for r in results if not r["err"]]
    s = [r for r in answered if r["supports_only"]]
    rf = [r for r in answered if r["refuted"]]
    s_flag = [r for r in s if r["flagged"]]
    rf_flag = [r for r in rf if r["flagged"]]

    Path(out_path).write_text(json.dumps(results, indent=2))
    q=[f for r in answered for f in (r.get("findings") or [])]
    ver=sum(1 for f in q if f.get("quote_verified"))
    print(f"\n{'='*66}\nRETRODICTION — question `{a.question}`, arm `{a.arm}`, {a.model}")
    print(f"errors (excluded from every denominator)  {len(results)-len(answered)} of {len(results)}")
    print(f"\nFALSE-POSITIVE SIDE — kill at 7 or more of 62")
    print(f"  supports-only claims flagged            {len(s_flag)} of {len(s)}"
          f"   -> {'KILL' if len(s_flag) >= 7 else 'passes'}")
    print(f"\nRECALL SIDE — kill at 2 or fewer of 9 (defect-match judged by hand)")
    print(f"  refuted claims flagged at all           {len(rf_flag)} of {len(rf)}"
          f"   -> flagging alone {'clears' if len(rf_flag) > 2 else 'does NOT clear'} the line")
    if q:
        print(f"\nQUOTATIONS — the check C20 specifies (Fact::quotes over captured output)")
        print(f"  findings carrying a verbatim span     {ver} of {len(q)}   "
              f"({100*ver/len(q):.0f}%)  -> {len(q)-ver} would be stripped")
    if a.repeat > 1:
        # A warning that varies run to run on identical input is a different
        # product from one that does not, so stability is reported, not assumed.
        per = defaultdict(list)
        for r in results:
            per[(r["memo"], r["id"])].append(r["flagged"])
        stable = sum(1 for v in per.values() if len(set(v)) == 1)
        print(f"\nSTABILITY over {a.repeat} runs of {len(per)} claims")
        print(f"  same answer every run                 {stable} of {len(per)}")
        for (m, i), v in sorted(per.items()):
            if len(set(v)) != 1:
                print(f"    unstable: {m[:34]:36} {i:5} flagged {sum(v)}/{len(v)}")
    print(f"\ncost  ${spend:.4f}   written to {out_path}")
    print(f"\nThe recall kill needs each flag to NAME THE DEFECT a later pass named.")
    print(f"That is a judgement over {len(rf)} items and is not scored here — read them:")
    for r in rf:
        print(f"\n  --- {r['memo'][:30]} {r['id']}  flagged={r['flagged']} ({r['verdict']})")
        print(f"      verifier: {(r['note'] or '(none)')[:200]}")
        for l in r["later"]:
            if l["verdict"] == "refutes":
                print(f"      refuted : {l['note'][:200]}")


if __name__ == "__main__":
    main()
