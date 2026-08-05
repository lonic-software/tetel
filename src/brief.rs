//! `tetel brief` — the grounding brief: every claim's id and proposition,
//! byte-identical to the ledger cell it came from, with `domain`/`extent`
//! withheld entirely so an independent grounding pass can't see what the
//! author declared the claim ranges over. `BriefItem` carries no field a
//! scope could leak through — that's enforced by its shape, not by care
//! at render time.

use serde::Serialize;

use crate::ledger::Claim;

pub struct BriefItem {
    pub id: String,
    pub proposition: String,
    pub pin: Option<String>,
}

/// Every ledger claim, unconditionally — including one already grounded,
/// and including one grounded only by ingested (attested-derived)
/// evidence. Ingestion enrolls a claim in the independent-grounding loop;
/// it does not excuse it, so nothing here may drop a claim from the brief
/// on account of what its evidence ledger already holds. A claim leaves
/// this list only by leaving the ledger itself — never by having been
/// reported on.
pub fn build(claims: &[Claim]) -> Vec<BriefItem> {
    claims
        .iter()
        .map(|c| BriefItem {
            id: c.id.clone(),
            proposition: c.proposition.clone(),
            pin: c.pin.clone(),
        })
        .collect()
}

pub fn render_text(display_path: &str, items: &[BriefItem]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "grounding brief: {display_path} — {} claim(s), scope withheld\n\n",
        items.len()
    ));
    for item in items {
        out.push_str(&format!("id: {}\n", item.id));
        out.push_str(&format!("proposition: {}\n", item.proposition));
        if let Some(pin) = &item.pin {
            out.push_str(&format!("pin: {pin}\n"));
        }
        out.push_str("scope: WITHHELD\n\n");
    }
    out
}

#[derive(Serialize)]
struct JsonItem<'a> {
    id: &'a str,
    proposition: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin: Option<&'a str>,
    scope: &'static str,
}

pub fn render_json(items: &[BriefItem]) -> String {
    let json_items: Vec<JsonItem> = items
        .iter()
        .map(|i| JsonItem {
            id: &i.id,
            proposition: &i.proposition,
            pin: i.pin.as_deref(),
            scope: "WITHHELD",
        })
        .collect();
    serde_json::to_string_pretty(&json_items).expect("brief items are plain strings and always serialize")
}

/// The authoring rhythm brief — handed to whoever (person or agent) is
/// about to write a document with `tetel look`/`run`/`fact`/`claim`/
/// `prose`/`render`. This is the highest-evidence instruction this
/// project has: run twice against the same task and the same tool,
/// differing only in whether this text was given, it produced
/// interleaved composition and a claim revision triggered by writing
/// prose in one arm, and strict front-to-back transcription (gather
/// every fact, then every claim, then all the prose) in the other.
///
/// Adapted from the harness prototype's own working brief to name this
/// crate's actual subcommands, and to drop the two features that
/// prototype supported but this port does not (`--before` prose
/// insertion, `tmove` reordering — see `prose.rs`). Self-contained: it
/// names no other document, repository, or path.
pub const AUTHORING_BRIEF: &str = "\
# Working brief — writing with tetel

Write the document using tetel's authoring commands: `look`, `run`, `fact`,
`claim`, `prose`, `render`.

## The rhythm — this is how the tool is meant to be used

Do not gather all your facts, then write all your claims, then write all
your prose. Work in passes, and let the document grow alongside the
evidence:

- As soon as a claim exists that you can say something about, write that
  prose then — do not defer it to a writing phase at the end.
- When writing prose makes you realise a claim is imprecise, wrong, or
  needs a qualification, revise the claim (`tetel claim --revise`) and
  then revise the prose that cites it (`tetel prose --revise`).
- When you find you need evidence you do not have, go and get it —
  `tetel look`/`tetel run`/`tetel fact` mid-way through writing is normal
  and expected, not a failure of planning.

A run in which nothing is revised and prose is written only after the
last claim means the tool was used as a transcription buffer. That is a
legitimate outcome to report if it is what genuinely happened — but do
not aim for it.
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger;

    #[test]
    fn authoring_brief_is_self_contained() {
        // This text ships in a public repository; it must never point at
        // a private planning repo, a path inside one, or any path at
        // all — a public reader has none of those to resolve.
        assert!(!AUTHORING_BRIEF.contains("lonic-planning"));
        assert!(!AUTHORING_BRIEF.contains("/Users/"));
        assert!(!AUTHORING_BRIEF.contains("/Volumes/"));
        assert!(!AUTHORING_BRIEF.contains("harness"));
        assert!(AUTHORING_BRIEF.contains("tetel claim --revise"));
    }

    #[test]
    fn briefed_proposition_is_byte_identical_to_the_source_cell() {
        let body: Vec<String> = "\
| ID | Proposition | Domain | Extent | Kind | Status |
|---|---|---|---|---|---|
| X-1 | `foo`'s bundle is **never** streamed [X-2], per §3 | the whole module | opened in full | READING | **VERIFIED** |
"
        .lines()
        .map(str::to_string)
        .collect();
        let import = ledger::import(&body);
        assert!(import.errors.is_empty());
        let claim = &import.claims[0];
        let items = build(&import.claims);
        assert_eq!(items[0].proposition, claim.proposition);
        assert_eq!(
            items[0].proposition,
            "`foo`'s bundle is **never** streamed [X-2], per §3"
        );
    }

    #[test]
    fn text_render_never_mentions_domain_or_extent_content() {
        let body: Vec<String> = "\
| ID | Proposition | Domain | Extent | Kind | Status |
|---|---|---|---|---|---|
| X-1 | a proposition | UNIQUEDOMAINTOKEN | UNIQUEEXTENTTOKEN | READING | **VERIFIED** |
"
        .lines()
        .map(str::to_string)
        .collect();
        let import = ledger::import(&body);
        let items = build(&import.claims);
        let text = render_text("memo.md", &items);
        assert!(!text.contains("UNIQUEDOMAINTOKEN"));
        assert!(!text.contains("UNIQUEEXTENTTOKEN"));
        assert!(text.contains("scope: WITHHELD"));
    }

    #[test]
    fn json_render_never_mentions_domain_or_extent_content() {
        let body: Vec<String> = "\
| ID | Proposition | Domain | Extent | Kind | Status |
|---|---|---|---|---|---|
| X-1 | a proposition | UNIQUEDOMAINTOKEN | UNIQUEEXTENTTOKEN | READING | **VERIFIED** |
"
        .lines()
        .map(str::to_string)
        .collect();
        let import = ledger::import(&body);
        let items = build(&import.claims);
        let json = render_json(&items);
        assert!(!json.contains("UNIQUEDOMAINTOKEN"));
        assert!(!json.contains("UNIQUEEXTENTTOKEN"));
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["scope"], "WITHHELD");
        assert_eq!(parsed[0]["id"], "X-1");
    }
}
