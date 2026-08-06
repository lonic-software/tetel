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
/// prototype supported but this port does not (`tmove` reordering — see
/// `prose.rs`; `--before` insertion was later restored). Self-contained: it
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
  prose then — do not defer it to a writing phase at the end. Where it
  belongs in the finished document is a separate question from when you
  wrote it: `tetel prose --before <ID>` puts a block ahead of an existing
  one, so a paragraph discovered late can still open a section.
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

## Grounding — where free reading stops

Orientation is free and unrecorded. Read however you like — grep around,
skim files, follow a call chain — to work out what you are even looking
at. None of that needs `look`/`run`.

But anything a fact rests on has to go *through* `look`/`run`, not just be
informed by it. Opening a file you already understood, only to give
`tetel fact` something to point at, records the act and throws away the
discovery it was supposed to be evidence of — the ledger shows a `look`,
not that it was theater.

`tetel fact` refuses on an empty pending buffer, so a fact minted with no
`look`/`run` at all is caught. It cannot tell a genuine discovery from a
`look` performed after the fact purely to satisfy that refusal — that
part is on you, not the tool.

## Spending evidence — the note is not the claim

Each of these is one observed failure from a real memo, turned into a
rule. Every one produced a defect that survived to a rendered document
and was caught later by a human reading closely.

- **A qualifier your note recorded is part of the fact.** If a note says
  `except` or `unless`, or records an ordering (`this runs AFTER that`),
  a claim resting on it either carries that qualifier or says why it is
  out of range. Writing `any`, `every` or `never` over a fact that
  recorded an exception is how a true fact becomes a false claim.

- **A comparison is a claim, not a note.** `X mirrors Y`, `both read the
  same field` — if this fact's extent contains only X, you have not read
  Y here. Mint a second fact for Y and put the comparison in a claim
  citing both. A note may name Y as context; it may not conclude about
  it. Two notes that each reach across to the other are how two facts
  come to contradict each other with nothing to notice it.

- **A number keeps the unit its instrument counted.** If the counter
  counted calls to one function, spend it as that. Converting it to
  another unit, or deriving a size from it, is a new assertion that needs
  its own claim or stays out of the prose.

- **A command that exits 0 has established nothing by exiting 0.** Before
  resting a fact on `run`, reread the transcript: does the output state
  the proposition, or only fail to deny it? If the proposition lives in a
  file the run wrote, that file belongs beside the memo, and the note
  says where.

Before `tetel render`, run `tetel review` and read each paragraph against
the claims it cites, asking one question: does this paragraph say
anything none of its cited claims says? The sentence carrying your
recommendation must cite a claim that states it, not a claim that happens
to sit nearby. Mint the missing claim or cut the sentence — do not let
the strongest sentence in the memo be the one with no row behind it.

Closing the loop is yours to do, not someone else's: once the document is
rendered (`tetel render`), run `tetel check` against it. That is your own
last check on what you just wrote, not a second reader's job.
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
        // The review step must stay named in the loop: `review` exists
        // to be run before `render`, and a brief that never mentions it
        // leaves the one channel that catches prose asserting past its
        // claims to be discovered by accident.
        assert!(AUTHORING_BRIEF.contains("tetel review"));
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
