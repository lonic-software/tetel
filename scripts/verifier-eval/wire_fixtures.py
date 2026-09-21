#!/usr/bin/env python3
"""Write the typed legs' request questions, as the harness sends them, to
`tests/fixtures/typed_wire/`.

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


if __name__ == "__main__":
    main()
