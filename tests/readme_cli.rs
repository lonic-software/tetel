//! Guards that `README.md`'s own prose stays tied to what `check` actually
//! partitions — the third of the three hand-maintained enumerations TET-63
//! found disagreeing with each other and with `src/report.rs`.
//!
//! `src/report.rs`'s scope strings and the MCP `check` description
//! (`tetel::mcp`'s `check_description`) both *build* their category lists
//! from `tetel::report::MACHINE_CHECKED_CATEGORIES`/`HUMAN_OWED_CATEGORIES`
//! now, so they cannot drift from that source — see that constant's own
//! doc comment and `tests/mcp_cli.rs`'s
//! `tool_descriptions_stay_tied_to_the_behaviour_they_promise`. `README.md`
//! is Markdown: no Rust runs there, so nothing can execute the constant.
//! This file is the "escape probe" the class fix uses for that one
//! consumer instead of a spot check — it reads the constant (an
//! independent source from the file it is grading) and asserts the file's
//! raw text contains every member, in both the direction that would miss a
//! category and the direction that would leave one described as not yet
//! built after it shipped.

use std::fs;

const README_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/README.md");

/// Collapses whitespace (including the line breaks Markdown wraps prose
/// at) to single spaces, so a category phrase that happens to straddle a
/// wrapped line in the source file still matches as one substring.
/// Deliberately too simple to share any code with `report::join_categories`
/// or `mcp::check_description` — an enforcer built from the same parser as
/// what it certifies balances exactly when that parser is wrong, which is
/// the trap this pair-guard family exists to avoid.
fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn readme() -> String {
    fs::read_to_string(README_PATH).expect("README.md should be readable")
}

/// Slices out the `## Direction` section's body (from its heading to the
/// next `##` heading), the section whose own text says its contents are
/// future work "gated on measurement rather than enthusiasm."
fn direction_section(readme: &str) -> &str {
    let heading = "## Direction";
    let start = readme.find(heading).expect("README.md should have a `## Direction` heading");
    let body_start = start + heading.len();
    let rest = &readme[body_start..];
    let end = rest.find("\n## ").map_or(readme.len(), |i| body_start + i);
    &readme[body_start..end]
}

/// Every category `report::render` actually partitions `check`'s output
/// into must be named somewhere in the README's own description of what
/// `check` tells you — not merely the two partition headers. Before this,
/// a category could ship into `report.rs` and never make it into this
/// file's hand-typed bullets, and nothing here would notice: TET-63 found
/// the human-owed bullet short by roughly twelve of eighteen.
#[test]
fn readme_names_every_check_category() {
    let readme = readme();
    let flat = normalized(&readme);
    for category in
        tetel::report::MACHINE_CHECKED_CATEGORIES.iter().chain(tetel::report::HUMAN_OWED_CATEGORIES.iter()).copied()
    {
        assert!(flat.contains(&normalized(category)), "README.md is missing check category {category:?}");
    }
}

/// The failure TET-63's correction comment demonstrated directly:
/// `README.md`'s `## Direction` section named `check`'s
/// prose-revised-since-proof residue as a plan after it had already
/// shipped in `c821b94`. A reader of a "not yet built" section has no way
/// to tell a stale entry from an honest one; the only guard against it is
/// making the two sections mutually exclusive and testing that they stay
/// that way.
#[test]
fn readme_direction_names_no_category_check_already_covers() {
    let readme = readme();
    let flat = normalized(direction_section(&readme));
    for category in
        tetel::report::MACHINE_CHECKED_CATEGORIES.iter().chain(tetel::report::HUMAN_OWED_CATEGORIES.iter()).copied()
    {
        assert!(
            !flat.contains(&normalized(category)),
            "README.md's Direction section names {category:?}, which `check` already covers — \
that section is future work only"
        );
    }
}
