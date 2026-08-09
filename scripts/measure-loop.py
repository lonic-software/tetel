#!/usr/bin/env python3
#
# Measure what the design loop costs and what each round of it produces.
#
# This exists because the numbers it computes were previously kept by hand in a
# planning document, and they rotted exactly the way this project's own finding
# says hand-maintained enumerations rot: a figure taken once ("census greps cost
# ~135k tokens, ~40% of the run") was cited onward into three tickets and an
# ordering decision before anyone re-derived it, at which point both halves
# turned out wrong. A table cannot be re-run. This can.
#
#   yield   <docs-dir>                 what each grading pass produced
#   cost    <transcript.jsonl> [facts] what a run was charged, and when
#
# `yield` reads only files in this repository — each memo's `.evidence.jsonl`
# beside its `.tetel/claims.jsonl` — so it is reproducible by anyone with a
# clone and needs no agent harness.
#
# `cost` needs a Claude Code subagent transcript, which lives outside this
# repository under the harness's own state directory. That half is therefore
# NOT reproducible from a clone alone, and says so rather than pretending
# otherwise. Pass a memo's `.tetel/facts.jsonl` as the optional second argument
# to additionally join what was loaded into context against what ended up
# inside some fact's extent.
#
# Three measurement hazards are handled here rather than left to the caller,
# because getting any of them wrong silently changes the answer:
#
#   1. A transcript writes ONE LINE PER CONTENT BLOCK, and every line of a
#      request carries the same `message.id` and the same usage object. Summing
#      lines double- or triple-counts. We deduplicate on `message.id`, and take
#      the MAX per field rather than the first, because a streamed response's
#      early events carry partial output counts.
#   2. `pass window` attribution below is BY TIME, not by causation, and it
#      is only valid when passes run SEQUENTIALLY. They often do not: on the
#      run that produced these figures, six of eight attacker passes began
#      inside a grounder's window. When that happens the grounder's window
#      closes the moment the attacker starts — crediting it with nothing —
#      and the attacker's window absorbs every later revision, including the
#      ones the grounder caused.
#      So: fine for asking whether a round was DRY, useless for comparing
#      one instrument against another. Ratios of revisions-per-pass between
#      grounders and attackers computed from this output measure scheduling,
#      not yield, and three such ratios were published and withdrawn before
#      anyone noticed. Compare verdict counts instead — they involve no
#      attribution. A correct attribution needs sequential passes, or a
#      record linking a revision to the finding that provoked it, and no
#      such link exists today.
#   3. The in-extent join is BY FILE PATH, not by span. A fact touching
#      `checks.rs` marks a whole 1,432-line read as load-bearing. So the
#      "never in any extent" figure is a FLOOR on waste, not an estimate.
#
# Weights are the published cache multipliers, in base-input-token equivalents:
# cache write 1.25x, cache read 0.1x, output 5x. They are named here so that a
# reader can see what the single "cost" number is made of, and change it.

import json
import os
import sys
from collections import Counter

W_WRITE, W_READ, W_OUT = 1.25, 0.1, 5.0

# A payload entering context at request i of N is charged once as a cache write
# and then re-read on every later request. This is the whole reason `when`
# matters as much as `how much`.
def multiplier(i, n):
    return W_WRITE + W_READ * max(n - i, 0)


def load_jsonl(path):
    out = []
    with open(path, errors="replace") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    return out


# ---------------------------------------------------------------- yield ----

def cmd_yield(docs_dir):
    ledgers = sorted(
        os.path.join(docs_dir, f) for f in os.listdir(docs_dir) if f.endswith(".evidence.jsonl")
    )
    if not ledgers:
        print(f"no *.evidence.jsonl under {docs_dir}", file=sys.stderr)
        return 2

    for ledger in ledgers:
        claims_path = ledger.replace(".evidence.jsonl", ".tetel/claims.jsonl")
        if not os.path.exists(claims_path):
            continue
        name = os.path.basename(ledger).replace(".evidence.jsonl", "")

        recs = []
        for r in load_jsonl(ledger):
            p = r.get("predicate", {})
            subj = r.get("subject", [{}])[0].get("name", "?")
            recs.append((subj, p.get("pass", "?"), p.get("timestamp", 0), p.get("verdict", "?")))
        if not recs:
            continue

        revisions = [e["timestamp"] for e in load_jsonl(claims_path) if e.get("event") == "Revise"]

        passes = {}
        for claim, pid, t, verdict in recs:
            passes.setdefault(pid, []).append((t, verdict, claim))
        order = sorted(passes.items(), key=lambda kv: min(x[0] for x in kv[1]))
        starts = [min(x[0] for x in v) for _, v in order]

        print(f"\n=== {name}")
        print(f"    {len(recs)} records, {len(revisions)} claim revisions, {len(order)} passes")
        print(f"    {'#':<3} {'mins':>5} {'recs':>5} {'claims':>7} {'sup':>4} {'qual':>5} {'ref':>4} {'revisions after':>16}")
        for n, (pid, rs) in enumerate(order, 1):
            start, end = min(x[0] for x in rs), max(x[0] for x in rs)
            later = [s for s in starts if s > start]
            limit = min(later) if later else float("inf")
            provoked = sum(1 for t in revisions if start <= t < limit)
            v = Counter(x[1] for x in rs)
            print(
                f"    {n:<3} {round((end - start) / 60):>5} {len(rs):>5} "
                f"{len(set(x[2] for x in rs)):>7} {v['supports']:>4} {v['qualifies']:>5} "
                f"{v['refutes']:>4} {provoked:>16}"
            )
        # Warn in the output, not only in the header: overlapping windows
        # make the column comparable across rounds and NOT across kinds.
        spans = sorted((min(x[0] for x in v), max(x[0] for x in v),
                        "attacker" if not any(vv == "supports" for _, vv, _ in v) else "grounder")
                       for _, v in order)
        overlaps = sum(1 for i in range(len(spans)) for j in range(i + 1, len(spans))
                       if spans[i][1] >= spans[j][0] and spans[i][2] != spans[j][2])
        print("    (revisions after = claim revisions inside that pass's window; time, not causation)")
        if overlaps:
            print(f"    !! {overlaps} cross-kind overlapping window(s): passes ran concurrently, so this")
            print("       column CANNOT be used to compare grounders against attackers. See the header.")
    return 0


# ----------------------------------------------------------------- cost ----

def requests_of(transcript):
    """Unique requests in order, as (cache_write, cache_read, output).

    Deduplicated on `message.id`, MAX per field -- see hazard 1 in the header.
    """
    best, order = {}, []
    for rec in load_jsonl(transcript):
        m = rec.get("message")
        if not isinstance(m, dict):
            continue
        u = m.get("usage")
        if not isinstance(u, dict):
            continue
        mid = m.get("id") or rec.get("uuid")
        if mid not in best:
            order.append(mid)
        cur = best.get(mid, (0, 0, 0))
        best[mid] = (
            max(cur[0], u.get("cache_creation_input_tokens", 0) or 0),
            max(cur[1], u.get("cache_read_input_tokens", 0) or 0),
            max(cur[2], u.get("output_tokens", 0) or 0),
        )
    return [best[m] for m in order]


def tool_results_of(transcript):
    """(request_index, tool_name, path, chars) for every tool result."""
    seen, idx, uses, out = set(), -1, {}, []
    for rec in load_jsonl(transcript):
        m = rec.get("message")
        if not isinstance(m, dict):
            continue
        u, mid = m.get("usage"), m.get("id") or rec.get("uuid")
        if isinstance(u, dict) and mid not in seen:
            seen.add(mid)
            idx += 1
        for b in m.get("content") or []:
            if not isinstance(b, dict):
                continue
            if b.get("type") == "tool_use":
                inp = b.get("input") or {}
                uses[b.get("id")] = (
                    b.get("name", "?"),
                    inp.get("file_path") or inp.get("path") or inp.get("root") or "",
                )
            elif b.get("type") == "tool_result":
                c = b.get("content")
                s = c if isinstance(c, str) else json.dumps(c)
                name, path = uses.get(b.get("tool_use_id"), ("?", ""))
                out.append((max(idx, 0), name, path, len(s)))
    return out


def extent_files(facts_path):
    paths = set()

    def walk(o):
        if isinstance(o, dict):
            for k, v in o.items():
                if k in ("key", "world_root") and isinstance(v, str):
                    paths.add(v)
                else:
                    walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)

    for rec in load_jsonl(facts_path):
        walk(rec)
    return {p for p in paths if os.path.splitext(p)[1]}


def cmd_cost(transcript, facts_path=None):
    reqs = requests_of(transcript)
    if not reqs:
        print(f"no usage records in {transcript}", file=sys.stderr)
        return 2
    n = len(reqs)
    write = sum(r[0] for r in reqs)
    read = sum(r[1] for r in reqs)
    out = sum(r[2] for r in reqs)
    peak = max(r[0] + r[1] for r in reqs)
    total = W_WRITE * write + W_READ * read + W_OUT * out

    print(f"{os.path.basename(transcript)}")
    print(f"  {n} requests, peak context {peak:,}")
    print(f"  {'cache_write':<12} {write:>12,}  x{W_WRITE} = {W_WRITE*write:>13,.0f}  {100*W_WRITE*write/total:5.1f}%")
    print(f"  {'cache_read':<12} {read:>12,}  x{W_READ} = {W_READ*read:>13,.0f}  {100*W_READ*read/total:5.1f}%")
    print(f"  {'output':<12} {out:>12,}  x{W_OUT} = {W_OUT*out:>13,.0f}  {100*W_OUT*out/total:5.1f}%")
    print(f"  {'TOTAL':<12} {'':>12}         {total:>13,.0f}  base-input-equivalents")

    # A request that read nothing from cache paid to rewrite the whole prefix.
    # In practice this means the prompt cache expired while the run waited on
    # something -- a spawned subagent, most often.
    cold = [r for r in reqs if r[1] == 0 and r[0] > 50_000]
    if cold:
        c = sum(r[0] for r in cold)
        print(f"  cold starts (cache_read=0, write>50k): {len(cold)}, {c:,} write tokens "
              f"({100*c/write:.0f}% of all writes)")
    print(f"  write/peak = {write/peak:.1f}x   (~1.0x means incremental caching worked throughout)")

    results = tool_results_of(transcript)
    if not results:
        return 0
    rows = [(i, name, path, chars / 4.0, (chars / 4.0) * multiplier(i, n)) for i, name, path, chars in results]
    early = [r for r in rows if r[0] < 30]
    late = [r for r in rows if r[0] >= 30]

    def tot(rs):
        return sum(r[3] for r in rs), sum(r[4] for r in rs)

    et, ec = tot(early)
    lt, lc = tot(late)
    print(f"\n  tool results (~4 chars/token), charged at 1.25 + 0.1*(N-i):")
    print(f"    {'phase':<20} {'results':>8} {'tokens':>10} {'charged':>12} {'avg mult':>9}")
    print(f"    {'first 30 requests':<20} {len(early):>8} {et:>10,.0f} {ec:>12,.0f} {ec/et if et else 0:>8.1f}x")
    print(f"    {'remaining':<20} {len(late):>8} {lt:>10,.0f} {lc:>12,.0f} {lc/lt if lt else 0:>8.1f}x")
    if ec + lc:
        print(f"    -> {100*ec/(ec+lc):.0f}% of all tool-result charge is incurred in the first 30 requests")

    if facts_path:
        files = extent_files(facts_path)
        pathed = [r for r in rows if r[2]]
        cited = [r for r in pathed if r[2] in files]
        uncited = [r for r in pathed if r[2] not in files]
        ct, cc = tot(cited)
        ut, uc = tot(uncited)
        print(f"\n  joined against {len(files)} files covered by some fact's extent:")
        print(f"    became evidence : {len(cited):>4} results {ct:>9,.0f} tok  charged {cc:>11,.0f}")
        print(f"    never in extent : {len(uncited):>4} results {ut:>9,.0f} tok  charged {uc:>11,.0f}")
        if cc + uc:
            print(f"    -> {100*uc/(cc+uc):.0f}% of path-bearing charge bought nothing the ledger records")
            print(f"       (by file path, not by span -- a FLOOR on waste, not an estimate)")

    print("\n  largest single charges:")
    for i, name, path, tok, cost in sorted(rows, key=lambda r: -r[4])[:10]:
        tag = ""
        if facts_path and path:
            tag = "  extent=yes" if path in extent_files(facts_path) else "  extent=no"
        print(f"    req {i:>4}  {name:<22} {tok:>8,.0f} x {multiplier(i, n):>5.1f} = {cost:>10,.0f}{tag}"
              f"  {os.path.basename(path)[:40]}")
    return 0


USAGE = """usage:
  measure-loop.py yield <docs-dir>                     e.g. docs/design
  measure-loop.py cost  <transcript.jsonl> [facts.jsonl]

`yield` reads only this repository. `cost` needs a Claude Code subagent
transcript from outside it; see the header for why that half is not
reproducible from a clone alone."""

if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(USAGE, file=sys.stderr)
        sys.exit(2)
    verb = sys.argv[1]
    if verb == "yield":
        sys.exit(cmd_yield(sys.argv[2]))
    elif verb == "cost":
        sys.exit(cmd_cost(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else None))
    print(USAGE, file=sys.stderr)
    sys.exit(2)
