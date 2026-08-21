#!/usr/bin/env python3
"""Census of every `pattern` `look --grep` has ever recorded, across every
`facts.jsonl` in every workspace this machine has authored, to answer one
question: what regex dialect would change the meaning of patterns already on
file?

`look_grep` (`src/observe.rs`) built its `grep` argv with no `-E` and no
`-F`, so the dialect was whichever `grep` came first on `PATH` — GNU-extended
BRE on both macOS and Linux. In that dialect `|`, `+`, `?`, `(`, `)`, `{`,
`}` are all *literal* unless individually escaped with a backslash; only
`\\(`, `\\)`, `\\{`, `\\}` group and bound, and `\\|` is a GNU extension that
alternates. Passing `-E` (POSIX ERE) flips every one of those seven
characters to mean the opposite of what it meant unescaped — grouping and
alternation become the default spelling, and a literal `(` or `|` now needs
its own backslash.

This script reads every recorded `pattern` and sorts the distinct ones (and
every `NoMatch` extent, regardless of pattern repetition) into the buckets
that answer: how many patterns does `-E` fix, how many does it silently
change, and how many negative results were minted from a pattern that meant
something other than what it looks like.

Counting rule for "no-match extents inside minted facts" (load-bearing,
read this before trusting or re-deriving that number): it counts an
extent only when its structured `kind` field reads `"NoMatch"`. It does
NOT count an extent by matching `"no-match: "` as a prefix of `label`
text. On this machine those two rules disagree — 157 by `kind`, 216 by
label prefix — because `kind` (and `pattern`, needed for the
unescaped-`|` half of this same row) were both added to the record in one
change; entries minted before that change carry `kind: null` and
`pattern: ""` even when their *label* already read "no-match: ..." in
plain prose. This script does not reach for those 59 older entries by
parsing a pattern back out of label text, deliberately: their `pattern`
field is exactly as unrecorded as `matcher` is on every entry this whole
census is about, and this crate's own discipline is that unrecorded means
unrecorded, not reconstructed by guessing at prose (see `PendingEntry`'s
and `ExtentEntry`'s `kind`/`pattern`/`matcher`/`out_len` fields in
`src/pending.rs` and `src/facts.rs`). The 157/216 gap is therefore not
disagreement about what "no-match" means — both counts agree once you ask
"was this ever structurally recorded as one" — it's a difference in
whether prose is admitted as a substitute for a missing structured field.
This script says no.

Usage: `python3 scripts/grep-dialect-census.py [state-dir]`
Default `state-dir` is `~/.local/state/tetel/workspaces`, matching where
`tetel` writes a workspace's `facts.jsonl` on this machine (see
`src/workspace.rs`). Every number below is computed fresh on each run —
nothing here is a cached or hand-copied figure.
"""

import json
import re
import sys
from pathlib import Path


def unescaped_positions(pattern: str, char: str):
    """Every index in `pattern` where `char` appears preceded by an EVEN
    number of consecutive backslashes (0, 2, 4, ...) — i.e. not itself
    escaped. A run of `n` backslashes immediately before `char` alternates
    escaped/unescaped as `n` is odd/even; this is the same parity rule a
    shell or a regex engine applies to its own escape character, applied
    here by hand since the property is being read out of a *string*, not
    executed by anything that understands backslashes itself.
    """
    out = []
    i = 0
    while True:
        j = pattern.find(char, i)
        if j == -1:
            break
        k = j - 1
        run = 0
        while k >= 0 and pattern[k] == "\\":
            run += 1
            k -= 1
        if run % 2 == 0:
            out.append(j)
        i = j + 1
    return out


def has_unescaped(pattern: str, char: str) -> bool:
    return bool(unescaped_positions(pattern, char))


def escaped_positions(pattern: str, char: str):
    """The complement of `unescaped_positions`: every index of `char`
    preceded by an ODD run of backslashes — the run that consumes `char`
    into an escape sequence rather than leaving it standing on its own.
    """
    out = []
    i = 0
    while True:
        j = pattern.find(char, i)
        if j == -1:
            break
        k = j - 1
        run = 0
        while k >= 0 and pattern[k] == "\\":
            run += 1
            k -= 1
        if run % 2 == 1:
            out.append(j)
        i = j + 1
    return out


def has_escaped(pattern: str, char: str) -> bool:
    return bool(escaped_positions(pattern, char))


# An ERE/GNU-BRE bounded interval, `{m,n}` or `{m}` — the one case where an
# unescaped `{`...`}` pair is not "grouping-shaped" the way `(`...`)` or `|`
# is, but is still unmistakably a regex construct rather than candidate
# literal text. Matched only when both braces are themselves unescaped,
# using the same parity rule as `unescaped_positions`.
_INTERVAL = re.compile(r"\{[0-9]+(,[0-9]*)?\}")


def looks_like_an_interval(pattern: str) -> bool:
    for m in _INTERVAL.finditer(pattern):
        # Both braces of this match must themselves be in the unescaped
        # set for the interval to actually be one under BRE/ERE.
        if m.start() in unescaped_positions(pattern, "{") and (m.end() - 1) in unescaped_positions(pattern, "}"):
            return True
    return False


GROUPING_CHARS = ["+", "?", "(", ")", "{", "}"]


def main():
    state_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else Path.home() / ".local/state/tetel/workspaces"
    if not state_dir.is_dir():
        print(f"no such directory: {state_dir}", file=sys.stderr)
        return 1

    unparseable = 0
    distinct = set()
    no_match_total = 0
    no_match_unescaped_pipe = 0

    for facts_path in sorted(state_dir.glob("*/facts.jsonl")):
        for raw in facts_path.read_text(errors="replace").splitlines():
            line = raw.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                unparseable += 1
                continue
            if event.get("event") != "Create":
                continue
            for entry in event.get("extent", []):
                pattern = entry.get("pattern") or ""
                if not pattern:
                    continue
                distinct.add(pattern)
                # By structured `kind`, never by a `label` text prefix —
                # see the module docstring's "Counting rule" paragraph for
                # why an older entry with no `kind` at all is excluded
                # rather than recovered by parsing its label.
                if entry.get("kind") == "NoMatch":
                    no_match_total += 1
                    if has_unescaped(pattern, "|"):
                        no_match_unescaped_pipe += 1

    want_ere = set()
    escaped_bre_alternation = set()
    for p in distinct:
        if has_unescaped(p, "|"):
            want_ere.add(p)
        elif has_escaped(p, "|"):
            escaped_bre_alternation.add(p)

    # Distinct patterns carrying an unescaped grouping-shaped metacharacter
    # other than `|` — `+ ? ( ) { }`. Presence, not position: one qualifying
    # character anywhere in the pattern is enough to place it here.
    grouping_shaped = {p for p in distinct if any(has_unescaped(p, c) for c in GROUPING_CHARS)}

    also_pipe = {p for p in grouping_shaped if p in want_ere}
    without_pipe = grouping_shaped - also_pipe

    # Of the patterns with no unescaped `|`, some are still regex-intended
    # rather than literal text reaching for punctuation: an ERE/BRE bounded
    # interval (`{0,300}`), or an escaped `\|` alternation used alongside
    # the other metacharacter (both measured cases below). Everything left
    # is the literal-use candidate set.
    reinspected_as_regex = {p for p in without_pipe if looks_like_an_interval(p) or p in escaped_bre_alternation}
    literal_candidates = without_pipe - reinspected_as_regex

    pct = round(100 * no_match_unescaped_pipe / no_match_total) if no_match_total else 0

    print(f"parsed every facts.jsonl under {state_dir}, {unparseable} unparseable line(s)\n")
    print(f"distinct patterns ever recorded                {len(distinct)}")
    print(f"  want ERE — unescaped |                        {len(want_ere)}")
    print(f"  escaped \\| — GNU BRE alternation, works today   {len(escaped_bre_alternation)}")
    print(f"contain unescaped + ? ( ) {{ }}                   {len(grouping_shaped)}")
    print(f"  ...and also | (grouping, intended regex)        {len(also_pipe)}")
    print(
        f"  ...without | (candidate literal use)           {len(without_pipe)}"
        f"   -> {len(literal_candidates)} after inspection"
    )
    print(f"no-match extents inside minted facts          {no_match_total}")
    print(f"  minted from an unescaped |                    {no_match_unescaped_pipe}   ({pct}%)")

    if without_pipe - literal_candidates:
        print("\nreclassified out of 'candidate literal use' by the interval/escaped-alternation rule above:")
        for p in sorted(without_pipe - literal_candidates):
            print(f"  {p!r}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
