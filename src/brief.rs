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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger;

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
