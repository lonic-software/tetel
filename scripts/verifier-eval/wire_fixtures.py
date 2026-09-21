#!/usr/bin/env python3
"""Write the typed legs' request questions, as the harness sends them, to
`tests/fixtures/typed_wire/`, with what its splitters and literal proposer
make of a few edge-case texts.

`src/verify.rs` asserts its own requests against these byte for byte. The
questions ARE the presentation that was measured — wording, option order and
key order alike — so a port is checked against the harness that produced the
numbers, not against a transcription of it.

    python3 wire_fixtures.py
"""

import json, sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import gate_variants as GV  # noqa: E402
import classify_jev  # noqa: E402
import literals_eval as L  # noqa: E402
import literals_jev  # noqa: E402
import refute_jev  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "tests" / "fixtures" / "typed_wire"

# Eleven sentences, so option S10 exists and a sorted map would send it
# before S2; clauses, dashes and a colon so the clause cut has work to do.
TEXT = ("The check makes two calls. It retries once on a truncated draw; the budget covers "
        "both: `timeout_ms` is 120000. Version 1.2 ships. e.g. the lower-case start stays "
        "joined. The gate runs first — before classify – and costs one call. A third sentence "
        "is here. A fourth sentence is here. A fifth sentence is here. A sixth sentence is "
        "here. A seventh sentence is here. An eighth sentence is here. A ninth sentence, "
        "with a clause, is here.\n\nA tenth sentence after a blank line.")

# A claim for classify and the literal leg: brackets and code spans holding
# commas the depth-0 cut must not split at, figures with the word they count,
# a parenthesised word NOUN must not take, number words, and paths with and
# without backticks and suffixes.
CLAIM = ("It read `src/verify.rs` and docs/verify.md: the gate asks 40 questions per call "
         "(3 files) over { id, proposition, cited fact ids } and `a, b` — twelve lines, 1,024 "
         "bytes at 12.5% of 14_000. The lower-case start stays joined; it retries two times "
         "on scripts/ and v1.2.3, then acks.jsonl.\n\nA second paragraph names TET-96, one "
         "call and 7_ items: done.")

# Texts the candidate, clause and unit ports are compared on, beside CLAIM.
EDGES = [
    "See `src/a.rs`, dir/sub/, a/b.jsonl and x.json.bak; 1,024 bytes (3 files) at 12.5% of 40_000.",
    "It took 1.5s: two-phase, Twelve lines, v1.2.3 and 3.x; ~7 items, #5 and 10.0.0.1 — 4%x (5) Items.",
    "Paths ``a/b`` and a//b/c.rs.rs, foo.py-bar, 3,, and 7_ and 9. Done: { a, b [c, d] } `x, y` e, f.",
    "Unbalanced ) closers ] first, then (an open one, never closed, and `an open code span, too",
    # NOUN never takes a word that opens with `(`.
    "It keeps 3 (three) files and 2 (x) more beside them.",
    # A sentence found earlier inside another: `clause_of` places each
    # sentence by `find`, so the literal crossing the blank line gets the
    # whole text, not the second sentence's clause.
    "The cache holds 9 or more, fine.\n\nThe cache holds 9\n\nItems are kept here always.",
]


def dump(obj):
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))


def captured(variant):
    seen = {}

    def fake(state, questions, key):
        seen["questions"] = questions
        return {}, 0.0, 0
    GV.ask_chunked = fake
    GV.VARIANTS[variant]({"text": TEXT, "evidence": "EVIDENCE"}, None)
    return seen["questions"]


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "text.txt").write_text(TEXT)
    for variant, name in (("pick_cls", "gate_fact.json"), ("pick_clause", "gate_claim.json")):
        (OUT / name).write_text(dump(captured(variant)) + "\n")
    (OUT / "refute_fact.json").write_text(dump(refute_jev.questions()) + "\n")
    (OUT / "claim.txt").write_text(CLAIM)
    seen = {}
    classify_jev.GV.ask_chunked = lambda state, qs, key: (seen.update(qs=qs), ({}, 0.0, 0))[1]
    classify_jev.classify_one(None, CLAIM, "clause0")
    (OUT / "classify_claim.json").write_text(dump(seen["qs"]) + "\n")
    judged = [c for c in literals_jev.candidates(CLAIM) if L.is_quantity(c)]
    (OUT / "literals_claim.json").write_text(dump(literals_jev.questions(CLAIM, judged)) + "\n")
    splits = [{"text": t, "candidates": literals_jev.candidates(t),
               "clauses": [literals_jev.clause_of(t, c) for c in literals_jev.candidates(t)],
               "units": classify_jev.units_for(t, "clause0")} for t in [CLAIM] + EDGES]
    (OUT / "splits_claim.json").write_text(json.dumps(splits, ensure_ascii=False, indent=1) + "\n")


if __name__ == "__main__":
    main()
