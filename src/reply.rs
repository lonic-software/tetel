//! How large an MCP reply may be, and the one place that holds every
//! reply to it.
//!
//! # The defect this exists for
//!
//! Past a size threshold, Claude Code writes an MCP tool's reply to a file
//! and hands the model the file's path instead. An agent restricted to
//! tetel's own tools has no way to open that path, so the reply is simply
//! gone, and nothing in the stub says what was in it (TET-93). The
//! installed 2.1.282 binary sets that threshold at 50,000 characters for a
//! tool that declares nothing, and lets a tool declare its own through
//! `_meta["anthropic/maxResultSizeChars"]`. `docs/design/tet93-bounded-reply.md`
//! records how both were read.
//!
//! # Two numbers, and why they differ
//!
//! [`REPLY_BUDGET`] is what tetel sends: at most 32768 UTF-8 bytes,
//! counting the content text **and** the serialized `structured_content`.
//! A structured reply carries its JSON in both fields
//! (`CallToolResult::structured` puts the same value in each), and which of
//! them Claude Code counts was not established, so the budget counts both.
//! Bytes, because a JavaScript string's length never exceeds its UTF-8
//! byte length, so a byte bound is also a bound on the client's measure.
//! 32768 is under the 50,000 an undeclared tool gets, so the budget alone
//! keeps a reply clear of the spill even for a client that ignores the
//! declaration.
//!
//! [`DECLARED_MAX_RESULT_SIZE_CHARS`] is what every tool declares. It is
//! headroom, not permission to send more: it covers a client counting both
//! fields, and stays well under the 500,000 ceiling because a reply that
//! is never spilled still costs its reader context.
//!
//! # The backstop is a floor
//!
//! [`bound`] cuts whatever reaches it over the budget, so that "every reply
//! is bounded" does not rest on each verb remembering to bound itself. It
//! is not how a verb is meant to answer: a verb that knows its own shape
//! (which lines a `look` showed, which records a `query` listed) says what
//! it left out in its own terms, and a reply the backstop has to cut is a
//! defect in the verb that produced it.

use rmcp::ErrorData;
use rmcp::model::{CallToolResponse, CallToolResult, ContentBlock};
use serde_json::Value;

/// The most any MCP reply may carry, in UTF-8 bytes: content text plus the
/// serialization of `structured_content`, when there is one.
pub const REPLY_BUDGET: usize = 32768;

/// The `_meta` key through which a tool declares its own spill threshold
/// to Claude Code.
pub const MAX_RESULT_SIZE_KEY: &str = "anthropic/maxResultSizeChars";

/// The threshold every tool declares under [`MAX_RESULT_SIZE_KEY`]. See
/// the module doc for why it is not the budget.
pub const DECLARED_MAX_RESULT_SIZE_CHARS: u64 = 100_000;

/// A reply's size by the budget's measure: every text block's bytes, plus
/// the serialized `structured_content`.
pub fn size(result: &CallToolResult) -> usize {
    content_text_len(result) + result.structured_content.as_ref().map_or(0, |v| v.to_string().len())
}

fn content_text_len(result: &CallToolResult) -> usize {
    result.content.iter().filter_map(ContentBlock::as_text).map(|t| t.text.len()).sum()
}

/// Hold one tool call's outcome to [`REPLY_BUDGET`]. An outcome within it
/// comes back untouched.
///
/// A completed result over it is reduced in two steps. If its content
/// text alone fits, only `structured_content` is dropped: tetel builds
/// structured replies with `CallToolResult::structured`, which puts the
/// same JSON in the content, so nothing is lost and nothing is marked.
/// Otherwise it becomes text only: the structured value's `verify` object
/// first and whole, then a prefix of the rest, then a marker saying the
/// reply was cut. A structured rest is compact JSON with its fields in
/// ascending order of size, so the small identifying ones (`id`, `action`,
/// `exit_code`) come before any long list or output that the cut falls in;
/// `serde_json`'s own order is alphabetical and put `fact`'s `id` behind
/// `attention` and `folded`. (Pretty printing was tried and rejected: a long
/// string field, like `run`'s `output`, is one pretty line, so a line cut
/// showed nothing of it.)
///
/// `verify` is exempt from cutting because a finding is marked delivered
/// while the reply is built (`verify_block` in `mcp.rs`), and no surface
/// prints a delivered finding afterwards, so a cut finding would be lost
/// for good. Keeping `verify` itself within the budget is the job of the
/// verbs that carry it; here it is carried whole even if that breaks the
/// bound, because a spilled reply is still readable by someone and a
/// dropped finding is not.
///
/// The marker speaks only about the reply. Whether the verb's capture is
/// whole is the verb's to say, in its own label and reply.
///
/// A protocol error's message is cut the same way, with any `data` folded
/// into the text first, so no outcome is exempt.
pub fn bound(outcome: Result<CallToolResponse, ErrorData>) -> Result<CallToolResponse, ErrorData> {
    match outcome {
        Ok(CallToolResponse::Complete(result)) => Ok(CallToolResponse::Complete(bound_result(result))),
        Ok(other) => Ok(other),
        Err(err) => Err(bound_error(err)),
    }
}

fn bound_result(mut result: CallToolResult) -> CallToolResult {
    if size(&result) <= REPLY_BUDGET {
        return result;
    }
    if result.structured_content.is_some() && content_text_len(&result) <= REPLY_BUDGET {
        result.structured_content = None;
        return result;
    }

    let (head, rest) = match result.structured_content.take() {
        Some(Value::Object(mut fields)) => {
            let head = fields
                .remove("verify")
                .map(|v| format!("{}\n", serde_json::json!({ "verify": v })))
                .unwrap_or_default();
            (head, smallest_first(fields))
        }
        Some(other) => (String::new(), other.to_string()),
        None => (String::new(), texts(&result.content)),
    };
    result.content = vec![ContentBlock::text(cut(&head, &rest))];
    result
}

/// A JSON object as compact text, its fields ordered by their serialized
/// size, smallest first.
fn smallest_first(fields: serde_json::Map<String, Value>) -> String {
    let mut parts: Vec<String> =
        fields.into_iter().map(|(k, v)| format!("{}:{v}", Value::String(k))).collect();
    parts.sort_by_key(String::len);
    format!("{{{}}}", parts.join(","))
}

/// `data` goes first because it is the structured part, usually short, and
/// the cut keeps the front: after a long message it would always be lost.
fn bound_error(mut err: ErrorData) -> ErrorData {
    let data_len = err.data.as_ref().map_or(0, |d| d.to_string().len());
    if err.message.len() + data_len <= REPLY_BUDGET {
        return err;
    }
    let text = match err.data.take() {
        Some(data) => format!("{data}\n{}", err.message),
        None => err.message.into_owned(),
    };
    err.message = cut("", &text).into();
    err
}

/// Every text block's text, one block per line.
fn texts(content: &[ContentBlock]) -> String {
    content.iter().filter_map(ContentBlock::as_text).map(|t| t.text.as_str()).collect::<Vec<_>>().join("\n")
}

/// `head` whole, then as much of `rest` as fits with the marker inside
/// [`REPLY_BUDGET`], then the marker. `rest` that fits whole is not marked.
fn cut(head: &str, rest: &str) -> String {
    if head.len() + rest.len() <= REPLY_BUDGET {
        return format!("{head}{rest}");
    }
    let total = rest.len();
    // Reserved at its longest before the cut: `shown` never has more digits
    // than `total`.
    let reserved = marker(total, total).len() + 1;
    let room = REPLY_BUDGET.saturating_sub(head.len() + reserved);
    let shown = prefix(rest, room);
    format!("{head}{shown}\n{}", marker(shown.len(), total))
}

/// The longest prefix of `s` within `room` bytes that ends at a line
/// boundary, or at a char boundary where the last line boundary in reach
/// would give up more than half the room: a short header followed by one
/// long line (`look` on a minified file) would otherwise show only the
/// header.
fn prefix(s: &str, room: usize) -> &str {
    let mut end = room.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    match s[..end].rfind('\n') {
        Some(i) if i >= end / 2 => &s[..i],
        _ => &s[..end],
    }
}

fn marker(shown: usize, total: usize) -> String {
    format!("[tetel: this reply was cut to its first {shown} of {total} bytes to stay within the {REPLY_BUDGET}-byte reply bound]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CUT: &str = "[tetel: this reply was cut";

    fn complete(result: CallToolResult) -> Result<CallToolResponse, ErrorData> {
        Ok(CallToolResponse::Complete(result))
    }

    fn unwrap_complete(outcome: Result<CallToolResponse, ErrorData>) -> CallToolResult {
        match outcome {
            Ok(CallToolResponse::Complete(r)) => r,
            other => panic!("expected a completed result, got {other:?}"),
        }
    }

    fn text_of(result: &CallToolResult) -> &str {
        assert_eq!(result.content.len(), 1, "a bounded reply is one text block");
        &result.content[0].as_text().expect("text block").text
    }

    /// `n` lines of 99 bytes each, numbered, so a cut point is visible.
    fn lines(n: usize) -> String {
        (0..n).map(|i| format!("{i:08} {}", "x".repeat(90))).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn an_over_budget_structured_reply_comes_back_text_only_within_the_budget_marked() {
        for is_error in [false, true] {
            let value = json!({ "exit_code": 0, "output": lines(1000) });
            let result = if is_error {
                CallToolResult::structured_error(value)
            } else {
                CallToolResult::structured(value)
            };
            assert!(content_text_len(&result) > REPLY_BUDGET, "premise: the content alone is over");

            let out = unwrap_complete(bound(complete(result)));
            assert!(size(&out) <= REPLY_BUDGET, "is_error={is_error}: size {} over the budget", size(&out));
            assert!(out.structured_content.is_none(), "is_error={is_error}: structured content must go");
            assert_eq!(out.is_error, Some(is_error), "is_error must survive the cut");
            let text = text_of(&out);
            assert!(text.ends_with("-byte reply bound]"), "is_error={is_error}: no marker at the end: …{}", &text[text.len() - 200..]);
            assert!(text.contains(CUT));
            assert!(text.contains("\\n00000200 "), "the cut must show the output, not stop before it");
        }
    }

    #[test]
    fn a_reply_whose_content_fits_loses_only_its_structured_copy_and_is_not_marked() {
        // With a `verify` object, so the text-only form (verify split out
        // onto its own line) differs from the content and the two steps
        // are told apart.
        let value = json!({ "output": lines(200), "verify": { "status": "off" } });
        let result = CallToolResult::structured(value);
        let content_before = text_of(&result).to_string();
        assert!(content_before.len() <= REPLY_BUDGET && size(&result) > REPLY_BUDGET, "premise: only the pair is over");

        let out = unwrap_complete(bound(complete(result)));
        assert!(out.structured_content.is_none());
        assert_eq!(text_of(&out), content_before, "the content must come back byte-identical");
        assert!(!text_of(&out).contains(CUT), "nothing was cut, so nothing may be marked");
    }

    #[test]
    fn a_delivered_verify_finding_survives_the_cut_whole() {
        let verify = json!({
            "status": "ok",
            "for_mint": "C3",
            "findings": [{
                "kind": "contradicts",
                "clause": "every call passes through one site",
                "evidence": "fn call_tool(",
                "why": "a sentinel finding whose text must survive",
                "facts": ["F2"],
            }],
        });
        let value = json!({ "id": "C4", "action": "created", "overlap": lines(1000), "verify": verify });
        let out = unwrap_complete(bound(complete(CallToolResult::structured(value))));

        assert!(size(&out) <= REPLY_BUDGET, "size {} over the budget", size(&out));
        let text = text_of(&out);
        let first = text.lines().next().unwrap();
        let carried: Value = serde_json::from_str(first).expect("the first line is the verify object");
        assert_eq!(carried, json!({ "verify": verify }), "verify must come back whole");
        assert!(text.contains(CUT), "the rest was cut, so it must be marked");
    }

    #[test]
    fn a_plain_text_reply_is_cut_at_a_line_boundary() {
        let body = lines(1000);
        let out = unwrap_complete(bound(complete(CallToolResult::success(vec![ContentBlock::text(body.clone())]))));
        let text = text_of(&out);
        assert!(text.len() <= REPLY_BUDGET);
        let (shown, _) = text.rsplit_once('\n').expect("marker on its own line");
        assert!(body.starts_with(shown), "the kept part must be a prefix of the original");
        assert!(body[shown.len()..].starts_with('\n'), "the cut must fall at a line boundary");
    }

    #[test]
    fn a_short_header_before_one_long_line_does_not_waste_the_budget() {
        let body = format!("==> /tmp/min.js <==\n{}", "x".repeat(100_000));
        let out = unwrap_complete(bound(complete(CallToolResult::success(vec![ContentBlock::text(body)]))));
        let text = text_of(&out);
        assert!(text.len() <= REPLY_BUDGET);
        assert!(text.len() > REPLY_BUDGET / 2, "only {} bytes shown: the cut stopped at the header", text.len());
    }

    #[test]
    fn a_minted_id_is_not_cut_behind_a_long_list() {
        // `folded` sorts before `id` alphabetically.
        let folded: Vec<String> = (0..2000).map(|i| format!("search {i}: {}", "y".repeat(40))).collect();
        let value = json!({ "id": "F77", "action": "minted", "attention": [], "folded": folded, "verify": { "status": "off" } });
        let out = unwrap_complete(bound(complete(CallToolResult::structured(value))));
        let text = text_of(&out);
        assert!(text.len() <= REPLY_BUDGET);
        assert!(text.contains(r#""id":"F77""#), "the minted id was cut away: {}", &text[..200]);
    }

    #[test]
    fn one_long_line_is_cut_at_a_char_boundary() {
        // Three-byte chars, so a byte-offset cut would split one.
        let body = "中".repeat(20_000);
        let out = unwrap_complete(bound(complete(CallToolResult::success(vec![ContentBlock::text(body)]))));
        let text = text_of(&out);
        assert!(text.len() <= REPLY_BUDGET);
        assert!(text.starts_with("中中中"), "a line with no break must still be shown, not dropped");
    }

    #[test]
    fn an_over_budget_protocol_error_is_cut_the_same_way() {
        let err = ErrorData::internal_error(lines(1000), Some(json!({ "detail": "kept in the text" })));
        let Err(out) = bound(Err(err)) else { panic!("an error stays an error") };
        assert!(out.message.len() <= REPLY_BUDGET, "message {} over the budget", out.message.len());
        assert!(out.data.is_none(), "data is folded into the text, not sent beside it");
        assert!(out.message.contains(CUT));
        assert!(out.message.contains("kept in the text"), "data must survive a long message");
    }

    #[test]
    fn an_under_budget_outcome_comes_back_byte_identical() {
        let small = CallToolResult::structured(json!({ "id": "F1", "verify": { "status": "off" } }));
        let before = serde_json::to_string(&small).unwrap();
        let out = unwrap_complete(bound(complete(small)));
        assert_eq!(serde_json::to_string(&out).unwrap(), before);

        let err = ErrorData::internal_error("small", Some(json!({ "k": 1 })));
        let Err(out) = bound(Err(err.clone())) else { panic!() };
        assert_eq!(out, err);
    }
}
