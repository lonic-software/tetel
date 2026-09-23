"""Where this harness's data lives: outside tetel's worktree (TET-97).

The files these scripts read and write by fixed name (replies, draws, labels) live in the
tetel-eval-data repository (https://github.com/lonic-software/tetel-eval-data), under its
`verifier-eval/` directory. It used to sit here, where it quoted memo prose into every census
tetel took of its own source. A path the memos cite as `scripts/verifier-eval/<rel>` is
`DATA / <rel>`, byte for byte as tetel last committed it.

The repository is found at `$TETEL_EVAL_DATA`, else beside tetel's checkout as
`../tetel-eval-data`. A missing checkout is an error at import, not a later empty glob.
"""
import os
import sys
from pathlib import Path

_ROOT = Path(os.environ.get("TETEL_EVAL_DATA")
             or Path(__file__).resolve().parents[3] / "tetel-eval-data")
DATA = _ROOT / "verifier-eval"

if not DATA.is_dir():
    sys.exit(f"no eval data at {DATA}: clone "
             "https://github.com/lonic-software/tetel-eval-data beside tetel, "
             "or set TETEL_EVAL_DATA to its root")
