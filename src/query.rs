//! `tetel query facts|claims|prose|deps <id>` — plain, greppable,
//! read-only inspection. Never refuses: nothing here asserts anything,
//! so there is nothing to gate (mirrors `tquery`).

use std::io;
use std::path::Path;

use crate::{claims, facts, prose};

pub fn facts_text(workspace_dir: &Path) -> io::Result<String> {
    let mut out = String::new();
    for f in facts::load_all(workspace_dir)? {
        out.push_str(&format!("{}\t{}\trevisions: {}\n", f.id, f.pin, f.revisions));
        out.push_str(&format!("  note: {}\n", f.note));
        for e in &f.extent {
            out.push_str(&format!("  extent: {} [world-state: {}]\n", e.label, e.world_state));
        }
    }
    Ok(out)
}

pub fn claims_text(workspace_dir: &Path) -> io::Result<String> {
    let mut out = String::new();
    for c in claims::load_all(workspace_dir)? {
        let status = if c.withdrawn { "withdrawn" } else { "active" };
        out.push_str(&format!("{}\t[{}]\trevisions: {}\tfrom: {}\n", c.id, status, c.revisions, c.from.join(",")));
        out.push_str(&format!("  prop: {}\n", c.prop));
    }
    Ok(out)
}

pub fn prose_text(workspace_dir: &Path) -> io::Result<String> {
    let mut out = String::new();
    for (i, b) in prose::load_all(workspace_dir)?.iter().enumerate() {
        let kind = if b.heading { format!("heading L{}", b.level.unwrap_or(0)) } else { "para".to_string() };
        let shown = b.text.lines().next().unwrap_or("");
        let cites = if b.cite.is_empty() { "(none)".to_string() } else { b.cite.join(",") };
        out.push_str(&format!("{}\t{}\t[{}]\t{}\tcites: {}\trevisions: {}\n", i + 1, b.id, kind, shown, cites, b.revisions));
    }
    Ok(out)
}

pub fn deps_text(workspace_dir: &Path, id: &str) -> io::Result<String> {
    let mut out = String::new();
    if id.starts_with('F') {
        if !facts::exists(workspace_dir, id)? {
            return Ok(format!("tetel query: no such fact: {id}\n"));
        }
        out.push_str(&format!("{id} rests on: (facts are foundational observations; nothing)\n"));
        out.push_str(&format!("{id} cited by:\n"));
        let mut found = false;
        for c in claims::load_all(workspace_dir)? {
            if c.from.iter().any(|f| f == id) {
                out.push_str(&format!("  {}\n", c.id));
                found = true;
            }
        }
        if !found {
            out.push_str("  (none)\n");
        }
    } else if id.starts_with('C') {
        if !claims::exists(workspace_dir, id)? {
            return Ok(format!("tetel query: no such claim: {id}\n"));
        }
        out.push_str(&format!("{id} rests on:\n"));
        for c in claims::load_all(workspace_dir)? {
            if c.id == id {
                for f in &c.from {
                    out.push_str(&format!("  {f}\n"));
                }
            }
        }
        out.push_str(&format!("{id} cited by:\n"));
        let mut found = false;
        for b in prose::load_all(workspace_dir)? {
            if b.cite.iter().any(|c| c == id) {
                out.push_str(&format!("  {}\n", b.id));
                found = true;
            }
        }
        if !found {
            out.push_str("  (none)\n");
        }
    } else {
        out.push_str("tetel query deps: id must start with F or C\n");
    }
    Ok(out)
}
