#!/usr/bin/env bash
#
# Run the three pair-guards after a change to this crate's source.
#
# The guards assert that two artifacts which must agree still do:
#
#   tests/roundtrip.rs                      what `render` writes, `check` reads back
#   tests/mcp_cli.rs  every_cli_subcommand_has_an_mcp_tool
#                     review_does_not_advertise_parameters_it_ignores
#                                           CLI surface <-> MCP surface
#   tests/mcp_cli.rs  tool_descriptions_stay_tied_to_the_behaviour_they_promise
#                                           each description <-> the thing that keeps it true
#
# Until this script existed they ran only when somebody typed `cargo test`,
# which is the same failure the guards were built to fix, one level up.
#
# Reads the hook payload on stdin and does its own filtering rather than
# relying on the `if` matcher, so the whole thing is testable by piping
# JSON at it — a guard whose trigger cannot be exercised is the defect
# this repository keeps finding in itself.
#
# Exit 2 (with the reason on stderr) reports the failure to the caller.
# It deliberately does NOT try to revert the edit: a legitimate two-step
# change — add a CLI subcommand, then add its MCP tool — is momentarily
# in exactly the state the guards refuse, and blocking there would make
# honest work unwritable, which is the cardinal sin `scope.rs` names
# about tetel's own refusals.

set -uo pipefail

payload=$(cat)
file=$(printf '%s' "$payload" | jq -r '.tool_input.file_path // .tool_response.filePath // empty')

# Only this crate's Rust sources can move either seam.
case "$file" in
  *"/tetel/src/"*.rs) ;;
  *) exit 0 ;;
esac

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

if out=$(cd "$repo" && cargo test --test mcp_cli --test roundtrip 2>&1); then
  exit 0
fi

{
  echo "pair-guards FAILED after editing $file"
  echo
  printf '%s\n' "$out" | grep -E "^(test |error|thread |assertion|---- )" | head -30
  echo
  echo "Two artifacts that must agree no longer do. Run:"
  echo "    cargo test --test mcp_cli --test roundtrip"
  echo "If this is the middle of a deliberate two-step change, finish the"
  echo "other half — the guard is reporting, not refusing."
} >&2
exit 2
