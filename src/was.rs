//! What an act on an existing id acted on, echoed in that act's reply.
//!
//! # The defect this exists for
//!
//! `prose --revise P24` used to answer `P24 revised.` and nothing else, so
//! a revision that overwrote the wrong block read exactly like one that hit
//! the intended block (TET-66). Ids never change, but positions do, and an
//! author who counts blocks is off by one after the first insertion. Across
//! the 13 memos under `docs/design` on 2026-09-26, the `why` of a later
//! revision records five prose revisions that hit the wrong block (two of
//! them headings), one that dropped a block's second paragraph, and one
//! claim revision that replaced its citations with the wrong list. Each was noticed only later, by the
//! author. Nothing in the reply had shown it.
//!
//! So every act that names an existing id — revise, withdraw, acknowledge —
//! now describes what that id held before the act. The describing is done
//! once, here, and both front ends print from it: the MCP reply carries
//! [`Was::json`] as `was`, and the CLI prints [`Was::line`] under its usual
//! first line.
//!
//! # What is echoed, and why not the whole text
//!
//! Only enough to recognise the object: the block's kind, its first line,
//! the ids it cites and, for a prose revision, the paragraph count before
//! and after. The citations carry the most signal. In all five wrong-block
//! revisions, the `why` named a claim the overwritten block did not cite,
//! and four of the five passed no new citations, so the citations are
//! echoed whether or not the act changed them. The whole text is not
//! echoed: it would be carried on every revision to help in the rare one
//! that went wrong, and the author usually still holds the old text anyway.
//!
//! # Size
//!
//! The reply budget reserves [`ENTRY_CAP`] beside a maximal `verify` for
//! everything else in a reply (see [`crate::reply::VERIFY_ALLOWANCE`]), so a
//! `was` object stays well inside it: [`OPENING_CAP`] for the first line and
//! [`CITES_CAP`] for the citations, each measured JSON-escaped, which is how
//! the reply counts them.
//!
//! [`ENTRY_CAP`]: crate::reply::ENTRY_CAP

use serde_json::{Value, json};

use crate::claims::Claim;
use crate::prose::Block;
use crate::reply::ELLIPSIS;
use crate::targets::Target;
use crate::transplants::{Premise, Transplant};

/// The most of an object's first line that `was` quotes, in JSON-escaped
/// bytes.
pub const OPENING_CAP: usize = 160;

/// The most of an object's citation list that `was` names, in JSON-escaped
/// bytes. The ids past it are counted in `cites_more`.
pub const CITES_CAP: usize = 256;

/// What one id held before an act on it. Build it with one of the
/// constructors, before the act writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Was {
    /// `heading` or `paragraph` for a prose block; `None` for everything
    /// else, whose id prefix already names its kind.
    kind: Option<&'static str>,
    /// A heading's depth.
    level: Option<u8>,
    /// The object's first line.
    opening: String,
    /// The ids it cited, or `None` for an object that cites nothing by
    /// nature (a fact, a premise). A prose block or claim citing nothing
    /// has `Some` of an empty list, which says so.
    cites: Option<Vec<String>>,
    /// A prose revision's paragraph count before and after it.
    paragraphs: Option<(usize, usize)>,
}

/// A block's paragraphs: its runs of text separated by a blank line.
/// `render` emits a block's text as it is, so a blank line inside it is the
/// paragraph break a reader sees.
pub fn paragraphs(text: &str) -> usize {
    text.split("\n\n").filter(|p| !p.trim().is_empty()).count()
}

/// `s`'s first line, cut so its JSON-escaped form is within
/// [`OPENING_CAP`], with a trailing " …" when anything was left out.
fn opening(s: &str) -> String {
    let first = s.lines().next().unwrap_or("");
    let more_lines = first.len() < s.trim_end().len();
    if escaped_len(first) <= OPENING_CAP && !more_lines {
        return first.to_string();
    }
    let room = OPENING_CAP - ELLIPSIS.len();
    let mut used = 0;
    let mut end = 0;
    for (i, c) in first.char_indices() {
        let cost = escaped_len(c.encode_utf8(&mut [0; 4]));
        if used + cost > room {
            break;
        }
        used += cost;
        end = i + c.len_utf8();
    }
    format!("{}{ELLIPSIS}", &first[..end])
}

fn escaped_len(s: &str) -> usize {
    Value::from(s).to_string().len() - 2
}

/// As many leading `ids` as fit [`CITES_CAP`], and how many did not.
fn cites_shown(ids: &[String]) -> (Vec<String>, usize) {
    let mut used = 2; // the brackets
    let mut shown = Vec::new();
    for id in ids {
        let cost = escaped_len(id) + 3; // its quotes and a comma
        if used + cost > CITES_CAP {
            break;
        }
        used += cost;
        shown.push(id.clone());
    }
    let more = ids.len() - shown.len();
    (shown, more)
}

impl Was {
    /// A prose block. `now` is the text a revision replaces it with, so the
    /// paragraph counts can be compared; `None` for an act that leaves the
    /// text alone.
    pub fn block(b: &Block, now: Option<&str>) -> Was {
        Was {
            kind: Some(if b.heading { "heading" } else { "paragraph" }),
            level: if b.heading { b.level } else { None },
            opening: opening(&b.text),
            cites: Some(b.cite.clone()),
            paragraphs: now.map(|n| (paragraphs(&b.text), paragraphs(n))),
        }
    }

    /// A fact's note.
    pub fn note(note: &str) -> Was {
        Was { kind: None, level: None, opening: opening(note), cites: None, paragraphs: None }
    }

    pub fn claim(c: &Claim) -> Was {
        Was { kind: None, level: None, opening: opening(&c.prop), cites: Some(c.from.clone()), paragraphs: None }
    }

    /// A target: its symbol and the census fact it cites.
    pub fn target(t: &Target) -> Was {
        Was { kind: None, level: None, opening: opening(&t.symbol), cites: Some(vec![t.from.clone()]), paragraphs: None }
    }

    /// A transplant: its donor fact and the target it installs into.
    pub fn transplant(t: &Transplant) -> Was {
        Was {
            kind: None,
            level: None,
            opening: opening(&format!("from {} into {}", t.from, t.into)),
            cites: None,
            paragraphs: None,
        }
    }

    /// A premise: the donor's words it selected.
    pub fn premise(p: &Premise) -> Was {
        Was { kind: None, level: None, opening: opening(&p.text), cites: None, paragraphs: None }
    }

    /// The `was` object of an MCP reply.
    pub fn json(&self) -> Value {
        let mut out = json!({ "opening": self.opening });
        if let Some(k) = self.kind {
            out["kind"] = json!(k);
        }
        if let Some(l) = self.level {
            out["level"] = json!(l);
        }
        if let Some(c) = &self.cites {
            let (shown, more) = cites_shown(c);
            out["cites"] = json!(shown);
            if more > 0 {
                out["cites_more"] = json!(more);
            }
        }
        if let Some((before, after)) = self.paragraphs {
            out["paragraphs"] = json!({ "before": before, "after": after });
        }
        out
    }

    /// The line the CLI prints under `<id> revised.` (or `withdrawn.`,
    /// `acknowledged.`), e.g.
    /// `  it was a heading (level 2) "The degradation contract", citing nothing; 1 paragraph, now 2`.
    pub fn line(&self) -> String {
        let mut s = String::from("  it was");
        match (self.kind, self.level) {
            (Some(k), Some(l)) => s.push_str(&format!(" a {k} (level {l})")),
            (Some(k), None) => s.push_str(&format!(" a {k}")),
            _ => {}
        }
        s.push_str(&format!(" \"{}\"", self.opening));
        if let Some(c) = &self.cites {
            let (shown, more) = cites_shown(c);
            if shown.is_empty() && more == 0 {
                s.push_str(", citing nothing");
            } else {
                s.push_str(&format!(", citing {}", shown.join(", ")));
                if more > 0 {
                    s.push_str(&format!(" and {more} more"));
                }
            }
        }
        if let Some((before, after)) = self.paragraphs {
            let unit = if before == 1 { "paragraph" } else { "paragraphs" };
            s.push_str(&format!("; {before} {unit}, now {after}"));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(heading: bool, text: &str, cite: &[&str]) -> Block {
        Block {
            id: "P1".into(),
            heading,
            level: heading.then_some(2),
            text: text.into(),
            cite: cite.iter().map(|s| s.to_string()).collect(),
            revisions: 0,
        }
    }

    #[test]
    fn a_heading_revised_into_a_paragraph_says_it_was_a_heading() {
        let w = Was::block(&block(true, "The degradation contract", &[]), Some("A paragraph.\n\nAnother."));
        let j = w.json();
        assert_eq!(j["kind"], "heading");
        assert_eq!(j["level"], 2);
        assert_eq!(j["opening"], "The degradation contract");
        assert_eq!(j["cites"], json!([]));
        assert_eq!(j["paragraphs"], json!({"before": 1, "after": 2}));
        assert_eq!(
            w.line(),
            "  it was a heading (level 2) \"The degradation contract\", citing nothing; 1 paragraph, now 2"
        );
    }

    #[test]
    fn a_dropped_paragraph_shows_in_the_counts() {
        let w = Was::block(&block(false, "First.\n\nSecond.", &["C3"]), Some("First, reworded."));
        assert_eq!(w.json()["paragraphs"], json!({"before": 2, "after": 1}));
        assert_eq!(w.json()["cites"], json!(["C3"]));
        assert!(w.json().get("level").is_none());
    }

    #[test]
    fn paragraphs_ignore_leading_trailing_and_repeated_blank_lines() {
        assert_eq!(paragraphs("\n\nA\n\n\n\nB\n\n"), 2);
        assert_eq!(paragraphs("A\nstill A"), 1);
        assert_eq!(paragraphs(""), 0);
    }

    #[test]
    fn an_opening_is_the_first_line_and_says_when_more_followed() {
        assert_eq!(opening("one line"), "one line");
        assert_eq!(opening("first\nsecond"), format!("first{ELLIPSIS}"));
    }

    #[test]
    fn an_opening_is_cut_by_its_escaped_length() {
        // A quote costs two bytes escaped, so 160 of them cannot fit.
        let quotes = "\"".repeat(OPENING_CAP);
        let o = opening(&quotes);
        assert!(o.ends_with(ELLIPSIS), "{o}");
        assert!(escaped_len(&o) <= OPENING_CAP, "{} > {OPENING_CAP}", escaped_len(&o));
        // A multi-byte character is never split.
        let wide = "é".repeat(OPENING_CAP);
        assert!(escaped_len(&opening(&wide)) <= OPENING_CAP);
    }

    #[test]
    fn a_long_citation_list_is_cut_and_counted() {
        let ids: Vec<String> = (1..=200).map(|i| format!("F{i}")).collect();
        let c = Claim { id: "C1".into(), prop: "p".into(), from: ids, withdrawn: false, revisions: 0 };
        let j = Was::claim(&c).json();
        let shown = j["cites"].as_array().unwrap().len();
        assert_eq!(shown + j["cites_more"].as_u64().unwrap() as usize, 200);
        assert!(j["cites"].to_string().len() <= CITES_CAP);
        assert!(Was::claim(&c).line().ends_with(" more"), "{}", Was::claim(&c).line());
    }

    /// The whole object stays inside the room the reply budget leaves
    /// beside a maximal `verify`, at the worst each field can reach.
    #[test]
    fn the_largest_was_fits_inside_entry_cap() {
        let ids: Vec<String> = (1..=500).map(|i| format!("C{i}")).collect();
        let b = Block {
            id: "P1".into(),
            heading: true,
            level: Some(6),
            text: "\u{1}".repeat(10_000),
            cite: ids,
            revisions: 0,
        };
        let size = Was::block(&b, Some(&"x\n\n".repeat(10_000))).json().to_string().len();
        // What else shares the room: `"id":"P…","action":"acknowledged","was":`,
        // the braces and commas. 128 bytes covers them with an id of 80.
        assert!(size <= crate::reply::ENTRY_CAP - 128, "{size}");
    }
}
