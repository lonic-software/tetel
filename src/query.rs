//! `tetel query facts|claims|prose|deps <id>` — plain, greppable,
//! read-only inspection. Never refuses: nothing here asserts anything,
//! so there is nothing to gate (mirrors `tquery`).
//!
//! # Every listing pages (TET-93)
//!
//! A query used to return the whole workspace, which past Claude Code's
//! spill threshold reached an agent as a file path it could not open.
//! Each view now emits whole records up to [`REPLY_BUDGET`] and says where
//! to continue. `from` is always an id: the record a listing starts at, or
//! the dependent `deps` starts at. A listing cuts each label and text to
//! [`ENTRY_CAP`], so one long record cannot crowd out the rest; `id`
//! returns one fact or claim uncut, and a fact's extents page by
//! `extent_from`, so every label can be read in full through some page
//! unless it is itself longer than the budget.
//!
//! The paging line leads the reply rather than trailing it, as `look`'s
//! does: the backstop in `call_tool` keeps a reply's front and cuts its
//! tail, so the front is the one place a cut could not remove it. The CLI
//! prints the same text, so the two surfaces page alike.

use std::borrow::Cow;
use std::io;
use std::path::Path;

use crate::reply::{ENTRY_CAP, REPLY_BUDGET, floor_char_boundary};
use crate::{claims, facts, prose};

/// One query, as both surfaces ask it.
pub enum Query<'a> {
    /// Every fact, from the one with id `from`.
    Facts { from: Option<&'a str> },
    /// One fact with its labels uncut, its extents from the 1-based
    /// `extent_from`.
    Fact { id: &'a str, extent_from: Option<usize> },
    /// Every claim, from the one with id `from`.
    Claims { from: Option<&'a str> },
    /// One claim, whole.
    Claim { id: &'a str },
    /// Every prose block in document order, from the one with id `from`.
    Prose { from: Option<&'a str> },
    /// What `id` rests on, and its dependents from the one with id `from`.
    Deps { id: &'a str, from: Option<&'a str> },
}

pub fn text(workspace_dir: &Path, q: Query) -> io::Result<String> {
    match q {
        Query::Facts { from } => facts_text(workspace_dir, from),
        Query::Fact { id, extent_from } => fact_text(workspace_dir, id, extent_from),
        Query::Claims { from } => claims_text(workspace_dir, from),
        Query::Claim { id } => claim_text(workspace_dir, id),
        Query::Prose { from } => prose_text(workspace_dir, from),
        Query::Deps { id, from } => deps_text(workspace_dir, id, from),
    }
}

/// `s` if it is within [`ENTRY_CAP`], else its longest prefix that fits
/// beside a trailing " …".
fn capped(s: &str) -> Cow<'_, str> {
    if s.len() <= ENTRY_CAP {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(format!("{} …", floor_char_boundary(s, ENTRY_CAP - " …".len())))
    }
}

/// `line` (ending in a newline) if it fits in `room`, else its longest
/// prefix that fits beside a statement of the cut.
fn cut_stated(line: &str, room: usize) -> String {
    if line.len() <= room {
        return line.to_string();
    }
    let total = line.len();
    let stated = |shown: usize| format!(" [tetel: cut to its first {shown} of {total} bytes]\n");
    let prefix = floor_char_boundary(line, room.saturating_sub(stated(total).len()));
    format!("{prefix}{}", stated(prefix.len()))
}

/// The paging line: which records this page shows, of how many, and where
/// to continue when any remain — `resume` followed by the next id.
fn paging_line(noun: &str, resume: &str, first: usize, last: usize, total: usize, next: Option<&str>) -> String {
    match next {
        Some(z) => format!(
            "[tetel: showed {noun} {first}-{last} of {total}; continue with {resume}{z} — a reply is held to {REPLY_BUDGET} bytes]\n"
        ),
        None => format!("[tetel: showed {noun} {first}-{last} of {total}]\n"),
    }
}

/// One listed record: its id and its rendered lines.
struct Entry {
    id: String,
    text: String,
}

/// Page `entries` from the one with id `from`: whole entries up to the
/// budget, led by a paging line whenever the page is not the whole list.
/// `head` is printed on every page, under the paging line. An entry that
/// does not fit on a page by itself gets one to itself, rendered by
/// `alone` within the room it is given. The paging line names the next
/// id after `resume`.
fn paged(
    noun: &str,
    resume: &str,
    head: &str,
    entries: &[Entry],
    from: Option<&str>,
    alone: impl Fn(usize, usize) -> String,
) -> String {
    let n = entries.len();
    let start = match from {
        None => 0,
        Some(f) => match entries.iter().position(|e| e.id == f) {
            Some(i) => i,
            None => return format!("tetel query: no {noun} with id {f} to start from\n"),
        },
    };
    let longest = entries.iter().map(|e| e.id.as_str()).max_by_key(|id| id.len()).unwrap_or("");
    let reserve = paging_line(noun, resume, n, n, n, Some(longest)).len();
    let room = REPLY_BUDGET.saturating_sub(head.len() + reserve);

    let mut body = String::new();
    let mut end = start;
    while end < n && body.len() + entries[end].text.len() <= room {
        body.push_str(&entries[end].text);
        end += 1;
    }
    if end == start && start < n {
        body = alone(start, room);
        end = start + 1;
    }
    let line = if start > 0 || end < n {
        paging_line(noun, resume, start + 1, end, n, entries.get(end).map(|e| e.id.as_str()))
    } else {
        String::new()
    };
    format!("{line}{head}{body}")
}

fn fact_head(f: &facts::Fact, note: &str) -> String {
    format!("{}\t{}\trevisions: {}\n  note: {}\n", f.id, f.pin, f.revisions, note)
}

fn extent_line(e: &facts::ExtentEntry, label: &str) -> String {
    format!("  extent: {} [world-state: {}]\n", label, e.world_state)
}

fn facts_text(workspace_dir: &Path, from: Option<&str>) -> io::Result<String> {
    let all = facts::load_all(workspace_dir)?;
    let listed = |f: &facts::Fact| -> (String, Vec<String>) {
        let head = fact_head(f, &capped(&f.note));
        (head, f.extent.iter().map(|e| extent_line(e, &capped(&e.label))).collect())
    };
    let entries: Vec<Entry> = all
        .iter()
        .map(|f| {
            let (head, lines) = listed(f);
            Entry { id: f.id.clone(), text: head + &lines.concat() }
        })
        .collect();
    // A fact whose extents alone overflow a page: its extent lines while
    // they fit, then a count naming the call that reads the rest.
    let alone = |i: usize, room: usize| {
        let f = &all[i];
        let (mut out, lines) = listed(f);
        let rest = |k: usize| {
            format!(
                "  … {} more extents: read them with id: {}, extent_from: {}\n",
                lines.len() - k,
                f.id,
                k + 1
            )
        };
        // `rest(k)` prints two numbers, neither above the extent count, so
        // that count in both places is the widest line it can print.
        let reserve = rest(0).len() + lines.len().to_string().len() - 1;
        let mut k = 0;
        while k < lines.len() && out.len() + lines[k].len() + reserve <= room {
            out.push_str(&lines[k]);
            k += 1;
        }
        if k < lines.len() {
            out.push_str(&rest(k));
        }
        out
    };
    Ok(paged("facts", "from: ", "", &entries, from, alone))
}

/// One fact with its note and labels uncut, its extents paged from the
/// 1-based `extent_from`.
///
/// The note is shown only when `extent_from` is absent, and on a page of
/// its own when the first extent does not fit beside it; an `extent_from`
/// page carries just the id line. So a note or a label is cut only when
/// it is longer than a page by itself, and the cut says so.
fn fact_text(workspace_dir: &Path, id: &str, extent_from: Option<usize>) -> io::Result<String> {
    let Some(f) = facts::get(workspace_dir, id)? else {
        return Ok(format!("tetel query: no such fact: {id}\n"));
    };
    let m = f.extent.len();
    let resume = format!("id: {id}, extent_from: ");
    let note_only = format!(
        "[tetel: showed the note; {m} extents follow — continue with {resume}1 — a reply is held to {REPLY_BUDGET} bytes]\n"
    );
    let reserve = paging_line("extents", &resume, m, m, m, Some(&m.to_string())).len().max(note_only.len());
    let lines: Vec<String> = f.extent.iter().map(|e| extent_line(e, &e.label)).collect();
    let entries: Vec<Entry> =
        lines.iter().enumerate().map(|(k, l)| Entry { id: (k + 1).to_string(), text: l.clone() }).collect();
    let id_line = format!("{}\t{}\trevisions: {}\n", f.id, f.pin, f.revisions);
    let head = match extent_from {
        None => {
            let note = cut_stated(&format!("  note: {}\n", f.note), REPLY_BUDGET - reserve - id_line.len());
            let head = id_line + &note;
            if m > 0 && head.len() + lines[0].len() + reserve > REPLY_BUDGET {
                return Ok(note_only + &head);
            }
            head
        }
        Some(_) if m == 0 => return Ok(format!("tetel query: {id} has no extents\n")),
        Some(k) if k == 0 || k > m => {
            return Ok(format!("tetel query: {id} has {m} extents; extent_from must be 1-{m}\n"));
        }
        Some(_) => id_line,
    };
    let from = entries.get(extent_from.unwrap_or(1) - 1).map(|e| e.id.as_str());
    Ok(paged("extents", &resume, &head, &entries, from, |k, room| cut_stated(&lines[k], room)))
}

fn claims_text(workspace_dir: &Path, from: Option<&str>) -> io::Result<String> {
    let entries: Vec<Entry> = claims::load_all(workspace_dir)?
        .iter()
        .map(|c| Entry { id: c.id.clone(), text: claim_lines(c, true) })
        .collect();
    Ok(paged("claims", "from: ", "", &entries, from, |i, room| cut_stated(&entries[i].text, room)))
}

/// A claim's lines, its citations and proposition cut to [`ENTRY_CAP`]
/// when `cap` is set.
fn claim_lines(c: &claims::Claim, cap: bool) -> String {
    let status = if c.withdrawn { "withdrawn" } else { "active" };
    let from = c.from.join(",");
    let cap = |s: &str| if cap { capped(s).into_owned() } else { s.to_string() };
    format!("{}\t[{}]\trevisions: {}\tfrom: {}\n  prop: {}\n", c.id, status, c.revisions, cap(&from), cap(&c.prop))
}

/// One claim, whole. A claim has no extents to page, so only a claim
/// longer than the budget by itself is cut, and says so.
fn claim_text(workspace_dir: &Path, id: &str) -> io::Result<String> {
    let Some(c) = claims::load_all(workspace_dir)?.into_iter().find(|c| c.id == id) else {
        return Ok(format!("tetel query: no such claim: {id}\n"));
    };
    Ok(cut_stated(&claim_lines(&c, false), REPLY_BUDGET))
}

fn prose_text(workspace_dir: &Path, from: Option<&str>) -> io::Result<String> {
    let entries: Vec<Entry> = prose::load_all(workspace_dir)?
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let kind = if b.heading { format!("heading L{}", b.level.unwrap_or(0)) } else { "para".to_string() };
            let shown = b.text.lines().next().unwrap_or("");
            let cites = if b.cite.is_empty() { "(none)".to_string() } else { b.cite.join(",") };
            let text = format!(
                "{}\t{}\t[{}]\t{}\tcites: {}\trevisions: {}\n",
                i + 1,
                b.id,
                kind,
                capped(shown),
                capped(&cites),
                b.revisions
            );
            Entry { id: b.id.clone(), text }
        })
        .collect();
    Ok(paged("prose blocks", "from: ", "", &entries, from, |i, room| cut_stated(&entries[i].text, room)))
}

fn deps_text(workspace_dir: &Path, id: &str, from: Option<&str>) -> io::Result<String> {
    let (head, dependents) = if id.starts_with('F') {
        if !facts::exists(workspace_dir, id)? {
            return Ok(format!("tetel query: no such fact: {id}\n"));
        }
        let head = format!("{id} rests on: (facts are foundational observations; nothing)\n{id} cited by:\n");
        let cited_by: Vec<String> =
            claims::load_all(workspace_dir)?.into_iter().filter(|c| c.from.iter().any(|f| f == id)).map(|c| c.id).collect();
        (head, cited_by)
    } else if id.starts_with('C') {
        let Some(claim) = claims::load_all(workspace_dir)?.into_iter().find(|c| c.id == id) else {
            return Ok(format!("tetel query: no such claim: {id}\n"));
        };
        // Printed on every page, so held to ENTRY_CAP: past it, a count and
        // the call that lists them all.
        let mut head = format!("{id} rests on:\n");
        let mut listed = String::new();
        let mut shown = 0;
        for f in &claim.from {
            let line = format!("  {f}\n");
            if listed.len() + line.len() > ENTRY_CAP {
                break;
            }
            listed.push_str(&line);
            shown += 1;
        }
        head.push_str(&listed);
        if shown < claim.from.len() {
            head.push_str(&format!(
                "  … {} more facts: query claims with id: {id} lists them all\n",
                claim.from.len() - shown
            ));
        }
        head.push_str(&format!("{id} cited by:\n"));
        let cited_by: Vec<String> =
            prose::load_all(workspace_dir)?.into_iter().filter(|b| b.cite.iter().any(|c| c == id)).map(|b| b.id).collect();
        (head, cited_by)
    } else {
        return Ok("tetel query deps: id must start with F or C\n".to_string());
    };
    if dependents.is_empty() && from.is_none() {
        return Ok(format!("{head}  (none)\n"));
    }
    let entries: Vec<Entry> = dependents.into_iter().map(|d| Entry { text: format!("  {d}\n"), id: d }).collect();
    Ok(paged("dependents", "from: ", &head, &entries, from, |i, room| cut_stated(&entries[i].text, room)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::path::PathBuf;

    struct Ws(PathBuf);

    impl Drop for Ws {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ws(tag: &str) -> Ws {
        let dir = std::env::temp_dir().join(format!("tetel-query-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Ws(dir)
    }

    fn append(dir: &Path, file: &str, events: &[serde_json::Value]) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(file)).unwrap();
        for e in events {
            writeln!(f, "{e}").unwrap();
        }
    }

    /// A label unique to fact `i`'s extent `k`, `len` bytes long.
    fn label(i: usize, k: usize, len: usize) -> String {
        let head = format!("L{i}-{k}:");
        format!("{head}{}", "x".repeat(len - head.len()))
    }

    fn fact(i: usize, labels: &[String]) -> serde_json::Value {
        let extent: Vec<_> =
            labels.iter().map(|l| serde_json::json!({"key": l, "label": l, "world_state": "ws"})).collect();
        serde_json::json!({"event": "Create", "id": format!("F{i}"), "note": format!("note {i}"),
            "extent": extent, "output": "", "pin": "pin", "timestamp": 0})
    }

    /// The `from` a page says to continue with, if any; asserts the page is
    /// within the budget on the way.
    fn next_from(page: &str) -> Option<String> {
        assert!(page.len() <= REPLY_BUDGET, "a page of {} bytes is over the budget", page.len());
        let at = page.find("continue with from: ")? + "continue with from: ".len();
        Some(page[at..].split(' ').next().unwrap().to_string())
    }

    /// Page `listing` from the start to the end, returning every page.
    fn all_pages(listing: impl Fn(Option<&str>) -> String) -> Vec<String> {
        let mut pages = vec![listing(None)];
        while let Some(from) = next_from(pages.last().unwrap()) {
            assert!(pages.len() < 1000, "paging does not end");
            pages.push(listing(Some(&from)));
        }
        pages
    }

    /// The fact workspace C14 (vi) names: more than one page of facts, one
    /// of which carries more extents than a page can hold even with every
    /// label cut to ENTRY_CAP, and one label over the budget by itself.
    fn big_workspace(w: &Ws) -> Vec<Vec<String>> {
        let mut all = Vec::new();
        let mut events = Vec::new();
        for i in 1..=60 {
            let labels: Vec<String> = if i == 30 {
                (1..=100).map(|k| label(i, k, if k == 50 { 40_000 } else { 1500 })).collect()
            } else {
                (1..=3).map(|k| label(i, k, 600)).collect()
            };
            events.push(fact(i, &labels));
            all.push(labels);
        }
        append(&w.0, "facts.jsonl", &events);
        all
    }

    #[test]
    fn paging_facts_yields_every_id_once_with_every_page_within_the_budget() {
        let w = ws("facts");
        big_workspace(&w);
        let pages = all_pages(|from| facts_text(&w.0, from).unwrap());
        assert!(pages.len() > 2, "premise: the workspace must span several pages, got {}", pages.len());

        let ids: Vec<String> = pages
            .iter()
            .flat_map(|p| p.lines().filter(|l| l.starts_with('F')).map(|l| l.split('\t').next().unwrap().to_string()))
            .collect();
        let want: Vec<String> = (1..=60).map(|i| format!("F{i}")).collect();
        assert_eq!(ids, want, "every fact exactly once, in order");

        // The oversized fact has a page to itself, which names the call
        // that reads the extents it could not hold.
        let alone = pages.iter().find(|p| p.contains("\nF30\t")).unwrap();
        assert_eq!(alone.lines().filter(|l| l.starts_with('F')).count(), 1, "F30 must be alone on its page");
        assert!(alone.contains("more extents: read them with id: F30, extent_from: "), "{}", &alone[..300]);

        // A listing cuts every label to ENTRY_CAP.
        let longest = pages.iter().flat_map(|p| p.lines()).filter(|l| l.starts_with("  extent: ")).map(str::len).max();
        let bound = "  extent: ".len() + ENTRY_CAP + " [world-state: ws]".len();
        assert!(longest.unwrap() <= bound, "an extent line of {longest:?} bytes was not cut to ENTRY_CAP");
    }

    #[test]
    fn paging_one_fact_by_extent_from_yields_every_label_once_and_uncut() {
        let w = ws("fact");
        let labels = &big_workspace(&w)[29];
        let mut pages = vec![fact_text(&w.0, "F30", None).unwrap()];
        loop {
            let page = pages.last().unwrap();
            assert!(page.len() <= REPLY_BUDGET, "a page of {} bytes is over the budget", page.len());
            let Some(at) = page.find("continue with id: F30, extent_from: ") else { break };
            let k: usize = page[at..].split(": ").nth(2).unwrap().split(' ').next().unwrap().parse().unwrap();
            assert!(pages.len() < 200, "paging does not end");
            pages.push(fact_text(&w.0, "F30", Some(k)).unwrap());
        }
        assert!(pages.len() > 2, "premise: the extents must span several pages");

        let shown: Vec<&str> = pages.iter().flat_map(|p| p.lines()).filter(|l| l.starts_with("  extent: ")).collect();
        assert_eq!(shown.len(), labels.len(), "every extent exactly once");
        for (line, label) in shown.iter().zip(labels) {
            if label.len() < REPLY_BUDGET / 2 {
                assert_eq!(*line, format!("  extent: {label} [world-state: ws]"), "a label must be read uncut");
            } else {
                let prefix = &line["  extent: ".len()..line.find(" [tetel: cut to its first ").expect("the cut must be stated")];
                assert!(label.starts_with(prefix) && prefix.len() > REPLY_BUDGET / 2, "cut to {} bytes", prefix.len());
            }
        }
        assert!(pages.iter().all(|p| p.starts_with("[tetel: showed extents ") && p.contains("\nF30\t")));
    }

    #[test]
    fn claims_prose_and_dependents_page_every_id_once() {
        let w = ws("rest");
        append(&w.0, "facts.jsonl", &[fact(1, &[label(1, 1, 10)])]);
        // Enough dependents of F1 and C1 to overflow a page, the first few
        // hundred long enough that the listing has to cut them.
        let n = 4500;
        let long = |i: usize| if i <= 300 { 2000 } else { 1 };
        let claims: Vec<_> = (1..=n)
            .map(|i| serde_json::json!({"event": "Create", "id": format!("C{i}"), "prop": "p".repeat(long(i)), "from": ["F1"], "timestamp": 0}))
            .collect();
        append(&w.0, "claims.jsonl", &claims);
        let blocks: Vec<_> = (1..=n)
            .map(|i| serde_json::json!({"event": "Create", "id": format!("P{i}"), "heading": false, "text": "t".repeat(long(i)), "cite": ["C1"], "timestamp": 0}))
            .collect();
        append(&w.0, "prose.jsonl", &blocks);

        let ids = |pages: Vec<String>, pick: fn(&str) -> Option<String>| -> Vec<String> {
            assert!(pages.len() > 1, "premise: several pages, got {}", pages.len());
            pages.iter().flat_map(|p| p.lines().filter_map(pick).collect::<Vec<_>>()).collect()
        };
        let want = |c: char| (1..=n).map(|i| format!("{c}{i}")).collect::<Vec<_>>();

        let got = ids(all_pages(|f| claims_text(&w.0, f).unwrap()), |l| {
            l.starts_with('C').then(|| l.split('\t').next().unwrap().to_string())
        });
        assert_eq!(got, want('C'));
        let got = ids(all_pages(|f| prose_text(&w.0, f).unwrap()), |l| {
            l.split('\t').nth(1).filter(|id| id.starts_with('P')).map(str::to_string)
        });
        assert_eq!(got, want('P'));
        let got = ids(all_pages(|f| deps_text(&w.0, "F1", f).unwrap()), |l| l.strip_prefix("  C").map(|d| format!("C{d}")));
        assert_eq!(got, want('C'));
        let got = ids(all_pages(|f| deps_text(&w.0, "C1", f).unwrap()), |l| l.strip_prefix("  P").map(|d| format!("P{d}")));
        assert_eq!(got, want('P'));

        // `id` on a claim returns it whole, where the listing cut it.
        assert!(claim_text(&w.0, "C7").unwrap().contains(&"p".repeat(2000)));
        assert!(!claims_text(&w.0, None).unwrap().contains(&"p".repeat(ENTRY_CAP + 1)));
    }

    /// The "more extents" line prints two counts that can be wider than
    /// the ones its room was reserved for (50 and 1 against 20 and 31).
    /// That width shows only when the extent lines fill the room to the
    /// byte. Each extent line is 929 bytes, and stepping the note one byte
    /// at a time across more than that makes some step fill it exactly.
    #[test]
    fn an_oversized_fact_alone_on_its_page_stays_within_the_budget_at_every_note_length() {
        let w = ws("sweep");
        let labels: Vec<String> = (1..=50).map(|k| label(7, k, 900)).collect();
        for len in 1..=ENTRY_CAP {
            let mut f = fact(7, &labels);
            f["note"] = serde_json::json!("n".repeat(len));
            std::fs::write(w.0.join("facts.jsonl"), format!("{f}\n{}\n", fact(8, &labels))).unwrap();
            let page = facts_text(&w.0, None).unwrap();
            assert!(page.contains("more extents"), "premise: F7 must overflow its page at note length {len}");
            assert!(page.len() <= REPLY_BUDGET, "a page of {} bytes at note length {len}", page.len());
        }
    }

    /// A claim's rests-on list is printed on every `deps` page, so it has
    /// to be bounded or it crowds the dependents out and the page over.
    #[test]
    fn deps_on_a_claim_citing_thousands_of_facts_pages_within_the_budget() {
        let w = ws("wide-claim");
        let from: Vec<String> = (1..=5000).map(|i| format!("F{i}")).collect();
        let claim = serde_json::json!({"event": "Create", "id": "C1", "prop": "p", "from": from, "timestamp": 0});
        append(&w.0, "claims.jsonl", &[claim]);
        let blocks: Vec<_> = (1..=5000)
            .map(|i| serde_json::json!({"event": "Create", "id": format!("P{i}"), "heading": false, "text": "t", "cite": ["C1"], "timestamp": 0}))
            .collect();
        append(&w.0, "prose.jsonl", &blocks);

        let pages = all_pages(|f| deps_text(&w.0, "C1", f).unwrap());
        assert!(pages[0].contains("more facts: query claims with id: C1 lists them all"), "{}", &pages[0][..300]);
        let got: Vec<&str> = pages.iter().flat_map(|p| p.lines().filter(|l| l.starts_with("  P"))).collect();
        assert_eq!(got.len(), 5000, "every dependent exactly once");
        assert!(pages.len() < 10, "the header crowds the dependents out: {} pages", pages.len());
        let whole = claim_text(&w.0, "C1").unwrap();
        assert!(whole.contains(",F5000\n"), "the claim itself lists every fact it rests on");
    }

    /// A long note must not cost a label under the budget its full text,
    /// and a note longer than half the budget is still readable whole.
    #[test]
    fn a_long_note_leaves_every_label_under_the_budget_readable_uncut() {
        let w = ws("long-note");
        let labels: Vec<String> = (1..=4).map(|k| label(1, k, 20_000)).collect();
        let mut f = fact(1, &labels);
        f["note"] = serde_json::json!("n".repeat(15_000));
        let mut g = fact(2, &[label(2, 1, 10)]);
        g["note"] = serde_json::json!("m".repeat(25_000));
        append(&w.0, "facts.jsonl", &[f, g]);

        let mut pages = vec![fact_text(&w.0, "F1", None).unwrap()];
        while let Some(at) = pages.last().unwrap().find("extent_from: ") {
            let k: usize = pages.last().unwrap()[at + "extent_from: ".len()..].split(' ').next().unwrap().parse().unwrap();
            assert!(pages.len() < 20, "paging does not end");
            pages.push(fact_text(&w.0, "F1", Some(k)).unwrap());
        }
        assert!(pages.iter().all(|p| p.len() <= REPLY_BUDGET));
        assert!(pages[0].contains(&format!("  note: {}\n", "n".repeat(15_000))), "the note must be whole");
        let shown: Vec<&str> = pages.iter().flat_map(|p| p.lines()).filter(|l| l.starts_with("  extent: ")).collect();
        let want: Vec<String> = labels.iter().map(|l| format!("  extent: {l} [world-state: ws]")).collect();
        assert_eq!(shown, want, "every label exactly once, uncut");

        let page = fact_text(&w.0, "F2", None).unwrap();
        assert!(page.len() <= REPLY_BUDGET && page.contains(&"m".repeat(25_000)), "a 25 KB note must be readable whole");
        assert!(fact_text(&w.0, "F1", Some(5)).unwrap().contains("extent_from must be 1-4"));
    }

    #[test]
    fn a_whole_listing_carries_no_paging_line_and_a_bad_start_is_named() {
        let w = ws("small");
        append(&w.0, "facts.jsonl", &[fact(1, &[label(1, 1, 10)]), fact(2, &[label(2, 1, 10)])]);
        assert!(facts_text(&w.0, None).unwrap().starts_with("F1\t"), "a whole listing must not be marked partial");
        assert!(facts_text(&w.0, Some("F2")).unwrap().starts_with("[tetel: showed facts 2-2 of 2]\nF2\t"));
        assert_eq!(facts_text(&w.0, Some("F9")).unwrap(), "tetel query: no facts with id F9 to start from\n");
        assert!(fact_text(&w.0, "F1", Some(2)).unwrap().contains("extent_from must be 1-1"));
    }
}
