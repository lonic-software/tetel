#!/usr/bin/env python3
"""Adapter for TypeSafe's System One endpoint — the sibling of `one_call`.

`retrodict.one_call` is chat-completions-shaped: messages, temperature,
max_tokens, reasoning effort, and a retry that triples the cap when reasoning
eats the budget. None of that exists here. Jev takes program state plus a map
of typed questions and returns typed answers with probabilities; there is no
sampling to truncate, so the retry that harness needs has no counterpart.

The key comes from the environment, or from the login keychain — never from a
file in this directory, and never written to one. That is the same rule
`src/verify.rs` holds for OPENROUTER_API_KEY, and it is why there is no
`verify.api_key` setting for `tetel config` to create.

Priced from the published rate: input $0.042/MTok, output free.
"""

import json, os, subprocess, urllib.request, urllib.error

ENDPOINT = "https://api.typesafe.ai/v1/systemone"
MODEL = "jev-latest"
KEY_VAR = "TYPESAFE_API_KEY"
KEYCHAIN_SERVICE = "typesafe-api-key"
USD_PER_INPUT_TOKEN = 0.042 / 1_000_000


def api_key():
    k = os.environ.get(KEY_VAR)
    if k:
        return k.strip()
    try:
        out = subprocess.run(
            ["security", "find-generic-password", "-a", os.environ.get("USER", ""),
             "-s", KEYCHAIN_SERVICE, "-w"],
            capture_output=True, text=True, timeout=10)
        if out.returncode == 0 and out.stdout.strip():
            return out.stdout.strip()
    except Exception:
        pass
    raise SystemExit(
        f"no key: set ${KEY_VAR}, or add it to the login keychain with\n"
        f"    security add-generic-password -a \"$USER\" -s {KEYCHAIN_SERVICE} -w")


def ask(state, questions, key=None, model=MODEL, timeout=60):
    """One call, many questions. Returns (answers, cost_usd, usage)."""
    body = json.dumps({"state": state, "model": model,
                       "questions": questions}).encode()
    req = urllib.request.Request(
        ENDPOINT, data=body, method="POST",
        headers={"Authorization": f"Bearer {key or api_key()}",
                 "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            d = json.loads(r.read())
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"HTTP {e.code}: {e.read()[:300].decode('utf8','replace')}")
    usage = d.get("usage", {})
    cost = usage.get("input_tokens", 0) * USD_PER_INPUT_TOKEN
    return d.get("answers", {}), cost, usage


if __name__ == "__main__":
    # Smoke test: one call, both primitives, against a state whose right
    # answers are not in doubt. A benchmark that cannot first reproduce an
    # obvious answer is measuring its own plumbing.
    answers, cost, usage = ask(
        "The build failed. Exit code 1. Three tests errored in payments/.",
        {"failed": {"type": "noul",
                    "instructions": "The build failed.",
                    "criteria": {"true": "it failed", "false": "it passed"}},
         "area": {"type": "choice",
                  "instructions": "Which area do the errors sit in?",
                  "criteria": {"payments": "payment code", "auth": "login code",
                               "search": "search code"}}})
    print(json.dumps(answers, indent=1))
    print(f"usage={usage} cost=${cost:.8f}")
