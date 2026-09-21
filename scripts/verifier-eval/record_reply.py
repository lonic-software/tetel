#!/usr/bin/env python3
"""Record one raw Jev reply, whole, into a committed file.

`jev.ask()` keeps `answers` and `usage` and drops the rest, so until this
file existed no harness had ever stored what the endpoint actually returns.
Two things in docs/design/tet-verifier-jev.md rested on a live call nobody
could re-read: the reply's top-level shape, and the versioned model the
alias `jev-latest` resolves to. This writes both down.

The request asks the two primitives the design uses — `noul` and `choice` —
over the same state as jev.py's smoke test. The key is read the way jev.py
reads it and goes only into the Authorization header; neither the request
headers nor the key are written anywhere.

    python3 record_reply.py            # writes jev_reply_<UTC date>.json
"""

import datetime, json, sys, urllib.request
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import jev  # noqa: E402

REQUEST = {
    "state": "The build failed. Exit code 1. Three tests errored in payments/.",
    "model": jev.MODEL,
    "questions": {
        "failed": {"type": "noul", "instructions": "The build failed.",
                   "criteria": {"true": "it failed", "false": "it passed"}},
        "area": {"type": "choice", "instructions": "Which area do the errors sit in?",
                 "criteria": {"payments": "payment code", "auth": "login code",
                              "search": "search code"}},
    },
}


def main():
    req = urllib.request.Request(
        jev.ENDPOINT, data=json.dumps(REQUEST).encode(), method="POST",
        headers={"Authorization": f"Bearer {jev.api_key()}",
                 "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        reply = json.loads(r.read())
    now = datetime.datetime.now(datetime.timezone.utc)
    out = HERE / f"jev_reply_{now:%Y-%m-%d}.json"
    out.write_text(json.dumps({
        "recorded_at": now.isoformat(timespec="seconds"),
        "endpoint": jev.ENDPOINT,
        "request": REQUEST,
        "reply": reply,
    }, indent=1) + "\n")
    print(f"wrote {out.name}: top-level keys {sorted(reply)}, model {reply.get('model')!r}")


if __name__ == "__main__":
    main()
