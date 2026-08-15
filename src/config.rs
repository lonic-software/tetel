//! `tetel config` — settings that outlive a single command, in a file a
//! person can read and comment.
//!
//! # Why there is configuration at all
//!
//! Almost nothing here should be configurable, and the tool has managed
//! without any settings until now. The case that forced it is
//! [`crate::brief::DEFAULT_FLOOR`], which
//! [`crate::brief::render_text`] already describes in its own comment as
//! *"otherwise invisible configuration"* — a value that changes what a
//! grounding pass is asked to do, carried today by a flag the caller must
//! remember to pass identically on every invocation. A setting that must
//! be repeated by hand on every call is configuration whether or not
//! there is a file for it; the only question is whether it is written
//! down somewhere auditable.
//!
//! # Two scopes, and why there is no project scope
//!
//! - **Global** — one file per user, applying to every workspace.
//! - **Workspace** — one file inside a workspace's own state directory,
//!   overriding the global file for that workspace only.
//!
//! The obvious third scope is the *project*: "every design I write about
//! this repository uses these settings". It is deliberately absent,
//! because it has nowhere to live. Such a file would sit inside the
//! repository under design, and [`crate::workspace`] refuses to write
//! there at all — a separation the module documents as load-bearing.
//! Reading a project file would be possible; writing one with
//! `tetel config` would not, leaving a scope that can be honoured but
//! never set, which is worse than a scope that does not exist. If it is
//! wanted later, the honest form is a section in the *global* file keyed
//! by the world root, so the write still lands outside the repository.
//!
//! # Where the global file lives
//!
//! Resolved the same way [`crate::workspace::state_home`] resolves the
//! state directory, and for the same reason — the XDG layout is what a
//! reader will guess, and an environment override is how tests stay out
//! of a real home directory:
//!
//!   1. `$TETEL_CONFIG_HOME`, if set.
//!   2. `$XDG_CONFIG_HOME/tetel`, if `XDG_CONFIG_HOME` is set.
//!   3. `$HOME/.config/tetel`.
//!
//! This deviates from the sibling tool the layout was modelled on, which
//! keeps a single dotfile in the home directory. The deviation is
//! deliberate: tetel already resolves a *state* home this way, and being
//! consistent with tetel's own convention matters more than being
//! consistent with another tool's. State and configuration are also
//! genuinely different — workspaces are machine-owned and regenerable,
//! configuration is hand-written and belongs with a user's other
//! settings.
//!
//! # Strict when set, tolerant when read
//!
//! [`set`] refuses a value the key does not accept. [`resolve`] does not:
//! a file edited by hand into an unparsable state degrades to the next
//! scope rather than failing the command that read it.
//!
//! The asymmetry is not laziness in the second half. It is chosen because
//! **every setting here must be visible in the output it affects** — the
//! floor is printed beside the owed list precisely so a reader can tell
//! which value produced the schedule in front of them. A tolerated bad
//! value therefore cannot mislead: whatever was actually used is on the
//! page. A setting that could not be shown that way should not be added
//! to this file.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The file name used at both scopes.
const FILE_NAME: &str = "config.toml";

/// Overrides the directory holding the global file. Primarily how tests
/// avoid a real home directory — see the module doc comment.
const ENV_CONFIG_HOME: &str = "TETEL_CONFIG_HOME";

/// How many distinct non-author workspaces must grade a claim's current
/// wording before it leaves the owed list. See
/// [`crate::brief::DEFAULT_FLOOR`] for what the value means and why its
/// default is 1; this key only decides where the number comes from.
pub const KEY_GROUNDING_FLOOR: &str = "grounding.floor";

/// Whether the mint-time verifier runs at all. Off unless set.
pub const KEY_VERIFY_ENABLED: &str = "verify.enabled";
/// Which model performs the comparison, as `vendor/model`.
pub const KEY_VERIFY_MODEL: &str = "verify.model";
/// `direct` (one call) or `extract` (three calls, re-derivable findings).
pub const KEY_VERIFY_APPROACH: &str = "verify.approach";
/// How long one mint's verification may take, end to end across retries.
pub const KEY_VERIFY_TIMEOUT_MS: &str = "verify.timeout_ms";
/// Which authoring verbs are verified, comma-separated.
pub const KEY_VERIFY_VERBS: &str = "verify.verbs";

/// The approaches [`KEY_VERIFY_APPROACH`] accepts, in one place so the
/// validator, the error message and [`crate::verify`] cannot disagree.
///
/// First is the default, and it is `split` rather than `direct` because
/// `split` is the configuration the retrodiction actually ran: assertions
/// classified before they are checked, with the whole claim in view. The
/// direct one-call comparison won the earlier fifteen-case eval, but the
/// numbers the decision to build rests on came from `split`, and shipping
/// a default whose accuracy nothing measured on real memos would be
/// choosing the cheaper of two things on the strength of the other one's
/// evidence.
///
/// The design also names a third mode — a three-call extract-then-judge
/// pipeline, kept for findings re-derivable by anyone holding the two
/// extracted records with no model in the loop. It is **not built**, and
/// is absent here rather than accepted and quietly treated as one of the
/// two above.
pub const VERIFY_APPROACHES: &[&str] = &["split", "direct"];

/// The verbs [`KEY_VERIFY_VERBS`] accepts. Not every authoring verb: only
/// the three that write text a captured record can be compared against.
pub const VERIFY_VERBS: &[&str] = &["fact", "claim", "prose"];

/// Which file a `config` operation reads or writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// The per-user file, applying to every workspace.
    Global,
    /// The current workspace's file, overriding the global one.
    Workspace,
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Scope::Global => "global",
            Scope::Workspace => "workspace",
        })
    }
}

/// What a key accepts. Checked by [`set`], never by [`resolve`].
enum Accepts {
    /// A whole number at or above `min`.
    ///
    /// Deliberately `u32` and not `u64`: every consumer of a number in
    /// this file parses it back into a `u32`, and validating wider than
    /// the consumer parses opens a silent hole. A value that fits `u64`
    /// but not `u32` would pass here, resolve as
    /// [`Source::File`] rather than [`Source::Rejected`], then fail the
    /// consumer's own parse and fall back to a default — with no warning,
    /// because the warning is gated on `Rejected`. That is precisely the
    /// "discoverable only by noticing the schedule looks wrong" outcome
    /// the grounding floor exists to avoid.
    IntAtLeast(u32),
    /// `true` or `false`, spelled either way round in case.
    Bool,
    /// Exactly one of the listed words.
    OneOf(&'static [&'static str]),
    /// A comma-separated list, every element drawn from the listed words.
    /// The empty list is accepted and means "no verbs".
    SubsetOf(&'static [&'static str]),
    /// A model identifier, and specifically **not** a credential.
    ///
    /// This is the one value-level rule in the file that exists for a
    /// reason other than catching a typo. Configuration files are shared,
    /// committed and pasted into issues, so the API key must come from the
    /// environment and never land here. There is no `verify.api_key` entry
    /// to set — [`set`] resolves every key through [`KEYS`] and refuses
    /// what it does not find — but a plain string key would cheerfully
    /// accept a credential pasted into it by an author who tried, so the
    /// key nearest the credential's shape refuses that shape by
    /// construction. Refusing here breaches nothing: `set` already refuses
    /// on value shape, and a refusal at `config set` can never suppress a
    /// mint.
    ModelName,
}

struct KeyDef {
    name: &'static str,
    summary: &'static str,
    accepts: Accepts,
}

/// Every settable key. A key absent from this table cannot be set, which
/// is how a typo is caught rather than silently stored under a name
/// nothing reads.
const KEYS: &[KeyDef] = &[
    KeyDef {
        name: KEY_GROUNDING_FLOOR,
        summary: "how many non-author workspaces must grade a claim's current wording \
before it leaves the owed list (at least 1)",
        accepts: Accepts::IntAtLeast(1),
    },
    KeyDef {
        name: KEY_VERIFY_ENABLED,
        summary: "whether a mint compares what you wrote against the evidence it rests on \
(true or false; off unless set, and off does nothing without a key in the environment)",
        accepts: Accepts::Bool,
    },
    KeyDef {
        name: KEY_VERIFY_MODEL,
        summary: "which model performs that comparison, as vendor/model. Never a credential: \
the key comes from the environment, and a credential-shaped value is refused here",
        accepts: Accepts::ModelName,
    },
    KeyDef {
        name: KEY_VERIFY_APPROACH,
        summary: "`split` — two calls, the default: classify the claim's assertions, then check \
only the ones the evidence can speak to. This is the configuration measured on real memos. \
Or `direct` — one call, cheaper, and measured only on synthetic cases",
        accepts: Accepts::OneOf(VERIFY_APPROACHES),
    },
    KeyDef {
        name: KEY_VERIFY_TIMEOUT_MS,
        summary: "how long one mint's verification may take end to end, across every retry \
(milliseconds, at least 1000). It does not sit in front of a reply, so it can be generous",
        accepts: Accepts::IntAtLeast(1000),
    },
    KeyDef {
        name: KEY_VERIFY_VERBS,
        summary: "which authoring verbs are verified, comma-separated from fact, claim, prose \
(default: claim alone — see the design memo for why the other two are off)",
        accepts: Accepts::SubsetOf(VERIFY_VERBS),
    },
];

fn key_def(name: &str) -> Option<&'static KeyDef> {
    KEYS.iter().find(|k| k.name == name)
}

/// Whether a rejected value for `key` must be withheld rather than shown
/// back to the reader.
///
/// One predicate, consulted by every site that would otherwise print a
/// value it refused — [`set`], [`list_text`] and the `config <key>` read
/// path. The rule is worth nothing if it holds at two of the three: an
/// author who pasted a credential into `verify.model` by hand meets the
/// refusal on *read*, not on write, and that is the path most likely to
/// end up in a terminal capture or a bug report.
pub fn hides_rejected_value(key: &str) -> bool {
    matches!(key_def(key).map(|d| &d.accepts), Some(Accepts::ModelName))
}

/// The names of every settable key, for an error message that tells the
/// reader what they could have written instead.
pub fn known_keys() -> Vec<&'static str> {
    KEYS.iter().map(|k| k.name).collect()
}

/// The directory holding the global file — see the module doc comment for
/// resolution order.
pub fn global_dir() -> PathBuf {
    if let Ok(p) = std::env::var(ENV_CONFIG_HOME) {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(p) = std::env::var("XDG_CONFIG_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p).join("tetel");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("tetel")
}

/// The global file's path. May not exist; absence is not an error.
pub fn global_path() -> PathBuf {
    global_dir().join(FILE_NAME)
}

/// A workspace's own file, inside its state directory.
pub fn workspace_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(FILE_NAME)
}

/// Where a resolved value came from, so the answer can be reported rather
/// than merely used.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Source {
    /// No file set it; the caller's own default stands.
    Default,
    /// Set in the named scope.
    File(Scope),
    /// Set in the named scope, but the value is not one the key accepts,
    /// so it was skipped. Reported rather than silently swallowed.
    Rejected(Scope, String),
}

/// One key's value, with the scope that supplied it.
///
/// Workspace beats global. A value the key does not accept is skipped and
/// reported through [`Source::Rejected`] — the tolerant half of the
/// asymmetry the module doc comment explains.
pub fn resolve(key: &str, workspace_dir: Option<&Path>) -> (Option<String>, Source) {
    let mut rejected: Option<Source> = None;
    let candidates: [(Scope, Option<PathBuf>); 2] = [
        (Scope::Workspace, workspace_dir.map(workspace_path)),
        (Scope::Global, Some(global_path())),
    ];
    for (scope, path) in candidates {
        let Some(path) = path else { continue };
        let Some(raw) = read_key(&path, key) else { continue };
        match key_def(key) {
            Some(def) if !accepted(&def.accepts, &raw) => {
                // Remember the first rejection so the caller can say
                // which file to go and fix, then keep looking: a bad
                // workspace value should not mask a good global one.
                if rejected.is_none() {
                    rejected = Some(Source::Rejected(scope, raw));
                }
            }
            // A good value wins, but the rejection above is not thereby
            // forgotten — see [`rejections`], which is how a broken file
            // masked by a working one stays discoverable. Without it, a
            // workspace file containing `floor = 0` beside a global file
            // containing `floor = 3` is silently ignored forever.
            _ => return (Some(raw), Source::File(scope)),
        }
    }
    match rejected {
        Some(s) => (None, s),
        None => (None, Source::Default),
    }
}

/// Every scope whose value for `key` this key does not accept, whether or
/// not a lower scope supplied a usable one.
///
/// [`resolve`] can report only the rejection that actually cost the
/// caller a value; this reports the ones that did not, which are exactly
/// the ones nothing else would ever mention. The rule the module states
/// is that a value it failed to apply must not be discoverable only by
/// noticing the output looks wrong, and a rejection hidden behind a
/// working fallback is the case that rule misses.
pub fn rejections(key: &str, workspace_dir: Option<&Path>) -> Vec<(Scope, String)> {
    let Some(def) = key_def(key) else { return Vec::new() };
    let candidates: [(Scope, Option<PathBuf>); 2] = [
        (Scope::Workspace, workspace_dir.map(workspace_path)),
        (Scope::Global, Some(global_path())),
    ];
    candidates
        .into_iter()
        .filter_map(|(scope, path)| {
            let raw = read_key(path.as_deref()?, key)?;
            (!accepted(&def.accepts, &raw)).then_some((scope, raw))
        })
        .collect()
}

/// [`resolve`] for [`KEY_GROUNDING_FLOOR`], parsed.
pub fn grounding_floor(workspace_dir: Option<&Path>) -> (Option<u32>, Source) {
    let (raw, source) = resolve(KEY_GROUNDING_FLOOR, workspace_dir);
    (raw.and_then(|v| v.trim().parse::<u32>().ok()), source)
}

/// [`resolve`] for [`KEY_VERIFY_ENABLED`], parsed. Absent means off.
pub fn verify_enabled(workspace_dir: Option<&Path>) -> bool {
    resolve(KEY_VERIFY_ENABLED, workspace_dir)
        .0
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// [`resolve`] for [`KEY_VERIFY_MODEL`].
pub fn verify_model(workspace_dir: Option<&Path>) -> Option<String> {
    resolve(KEY_VERIFY_MODEL, workspace_dir).0
}

/// [`resolve`] for [`KEY_VERIFY_APPROACH`]. Absent means the first entry
/// in [`VERIFY_APPROACHES`], which is `split` — see that constant for why
/// the default is the two-call configuration and not the cheaper one.
pub fn verify_approach(workspace_dir: Option<&Path>) -> String {
    resolve(KEY_VERIFY_APPROACH, workspace_dir)
        .0
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| VERIFY_APPROACHES.contains(&v.as_str()))
        .unwrap_or_else(|| VERIFY_APPROACHES[0].to_string())
}

/// [`resolve`] for [`KEY_VERIFY_TIMEOUT_MS`], parsed.
pub fn verify_timeout_ms(workspace_dir: Option<&Path>) -> u64 {
    const DEFAULT_TIMEOUT_MS: u32 = 60_000;
    // Parsed as `u32` and widened, not parsed as `u64`. The validator
    // works in `u32`, and a consumer that parses wider than the validator
    // is the same silent hole in the other direction: it would accept, at
    // this one call site, a value `set` and `resolve` had already refused.
    resolve(KEY_VERIFY_TIMEOUT_MS, workspace_dir)
        .0
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_MS) as u64
}

/// [`resolve`] for [`KEY_VERIFY_VERBS`], parsed.
///
/// The default is `claim` alone. `fact` is half covered already, for free
/// and deterministically, by [`crate::scope`]; `prose` is the
/// highest-volume verb and the least-evidenced comparison. Both are one
/// `config` line away for anyone who wants them.
pub fn verify_verbs(workspace_dir: Option<&Path>) -> Vec<String> {
    match resolve(KEY_VERIFY_VERBS, workspace_dir).0 {
        Some(raw) => split_list(&raw)
            .into_iter()
            .map(|v| v.to_ascii_lowercase())
            .filter(|v| VERIFY_VERBS.contains(&v.as_str()))
            .collect(),
        None => vec!["claim".to_string()],
    }
}

fn accepted(accepts: &Accepts, raw: &str) -> bool {
    let raw = raw.trim();
    match accepts {
        Accepts::IntAtLeast(min) => raw.parse::<u32>().map(|n| n >= *min).unwrap_or(false),
        Accepts::Bool => matches!(raw.to_ascii_lowercase().as_str(), "true" | "false"),
        Accepts::OneOf(words) => words.contains(&raw.to_ascii_lowercase().as_str()),
        Accepts::SubsetOf(words) => split_list(raw)
            .iter()
            .all(|w| words.contains(&w.to_ascii_lowercase().as_str())),
        Accepts::ModelName => is_model_name(raw),
    }
}

/// A comma-separated list, empty elements dropped so `a,,b` and a trailing
/// comma are not errors nobody would predict.
fn split_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Whether `raw` is a model identifier rather than a credential.
///
/// One rule carries the weight: a model id names a vendor and a model,
/// `openai/gpt-5.6-luna`, and every credential shape this is guarding
/// against — `sk-…`, `sk-ant-…`, `sk-or-v1-…` — is a single opaque token
/// with no separator in it. So the slash is required, which refuses a
/// pasted key by its shape and not by trying to recognise a secret. The
/// prefix and length tests below are belt and braces: they cost nothing
/// and they refuse the one thing that would otherwise slip through, a
/// credential that happens to contain a slash.
fn is_model_name(raw: &str) -> bool {
    const CREDENTIAL_PREFIXES: [&str; 5] = ["sk-", "sk_", "pk-", "api-", "bearer "];
    if raw.is_empty() || raw.len() > 96 {
        return false;
    }
    let lower = raw.to_ascii_lowercase();
    if CREDENTIAL_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return false;
    }
    let Some((vendor, model)) = raw.split_once('/') else {
        return false;
    };
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':'))
    };
    ok(vendor) && ok(model)
}

/// Set `key` to `value` in `scope`, creating the file if needed.
///
/// Refuses an unknown key and a value the key does not accept, and writes
/// nothing when it refuses. Existing comments and unrelated keys in the
/// file survive the write.
pub fn set(scope: Scope, workspace_dir: Option<&Path>, key: &str, value: &str) -> io::Result<PathBuf> {
    let Some(def) = key_def(key) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "unknown setting `{key}`. Known settings: {}",
                known_keys().join(", ")
            ),
        ));
    };
    if !accepted(&def.accepts, value) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            match def.accepts {
                Accepts::IntAtLeast(min) => format!(
                    "`{key}` takes a whole number of at least {min}; got `{value}`"
                ),
                Accepts::Bool => format!("`{key}` takes `true` or `false`; got `{value}`"),
                Accepts::OneOf(words) => format!(
                    "`{key}` takes one of {}; got `{value}`",
                    words.join(", ")
                ),
                Accepts::SubsetOf(words) => format!(
                    "`{key}` takes a comma-separated list drawn from {}; got `{value}`",
                    words.join(", ")
                ),
                // Deliberately does not echo the value back. If an author
                // did paste a credential here, repeating it into a
                // terminal, a log or a bug report is the harm this rule
                // exists to prevent.
                Accepts::ModelName => format!(
                    "`{key}` takes a model identifier as vendor/model, such as \
openai/gpt-5.6-luna. The value given is not one, and is not echoed here in case it \
is a credential: API keys are read from the environment and never stored in a config \
file, which is shared, committed and pasted into issues"
                ),
            },
        ));
    }
    let path = path_for(scope, workspace_dir)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let updated = write_key(&existing, key, value, scope);
    fs::write(&path, updated)?;
    Ok(path)
}

/// Remove `key` from `scope`'s file, so the next scope down applies again.
pub fn unset(scope: Scope, workspace_dir: Option<&Path>, key: &str) -> io::Result<PathBuf> {
    // Through the registry, exactly as `set` is. Without this,
    // `--unset grounding.flooor` reported success and exited 0 while
    // `grounding.floor` stayed in force — an author believing they had
    // reverted a setting that is still deciding the schedule.
    if key_def(key).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "unknown setting `{key}`. Known settings: {}",
                known_keys().join(", ")
            ),
        ));
    }
    let path = path_for(scope, workspace_dir)?;
    // Nothing to remove is not an error, but neither is it a reason to
    // leave an empty file behind where none existed.
    if !path.exists() {
        return Ok(path);
    }
    let existing = fs::read_to_string(&path).unwrap_or_default();
    fs::write(&path, remove_key(&existing, key))?;
    Ok(path)
}

fn path_for(scope: Scope, workspace_dir: Option<&Path>) -> io::Result<PathBuf> {
    match scope {
        Scope::Global => Ok(global_path()),
        Scope::Workspace => {
            let dir = workspace_dir.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "workspace scope needs a workspace; none was given",
                )
            })?;
            // The workspace has to exist already. Writing into one that
            // does not would create a directory holding nothing but
            // `config.toml`, and `workspace::list` lists any directory —
            // so a mistyped `--workspace` would leave a workspace with no
            // facts, claims or prose that no authoring command ever made,
            // permanently, in `tetel workspaces`.
            if !dir.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "no workspace at {} — settings for a workspace are written into its own \
state directory, so the workspace has to exist first",
                        dir.display()
                    ),
                ));
            }
            Ok(workspace_path(dir))
        }
    }
}

/// Every known key with its effective value and where that value came
/// from — the whole point being that a reader can see which file to edit.
pub fn list_text(workspace_dir: Option<&Path>) -> String {
    let mut out = String::new();
    out.push_str(&format!("global:    {}\n", global_path().display()));
    match workspace_dir {
        Some(d) => out.push_str(&format!("workspace: {}\n", workspace_path(d).display())),
        None => out.push_str("workspace: (no workspace selected)\n"),
    }
    out.push('\n');
    for def in KEYS {
        let (value, source) = resolve(def.name, workspace_dir);
        let shown = match (&value, &source) {
            (Some(v), Source::File(scope)) => format!("{v}  (from {scope})"),
            // Same reason `set` does not echo a rejected model name: if
            // what is sitting in the file is a pasted credential, printing
            // it here spreads it to wherever this listing is pasted.
            (_, Source::Rejected(scope, _)) if hides_rejected_value(def.name) => {
                format!("(unset — the value in the {scope} file is not a model identifier)")
            }
            (_, Source::Rejected(scope, raw)) => {
                format!("(unset — `{raw}` in the {scope} file is not a value this key accepts)")
            }
            _ => "(unset)".to_string(),
        };
        out.push_str(&format!("{}\n  {}\n", def.name, shown));
        // A rejection the resolution stepped over: reported here because
        // nothing else will mention it, the value in force having come
        // from somewhere else entirely.
        if matches!(source, Source::File(_)) {
            for (scope, raw) in rejections(def.name, workspace_dir) {
                out.push_str(&if hides_rejected_value(def.name) {
                    format!("  also: the {scope} file holds a value this key does not accept (not echoed, in case it is a credential)\n")
                } else {
                    format!("  also: `{raw}` in the {scope} file is not a value this key accepts, and is being ignored\n")
                });
            }
        }
        out.push_str(&format!("  {}\n\n", def.summary));
    }
    out
}

// ---------------------------------------------------------------------
// A deliberately small TOML subset.
//
// Only `[table]` headers, `key = value` lines, `#` comments and blank
// lines are understood, which is the whole of what this file needs to
// be. Hand-rolled rather than taken from a crate because the alternative
// is a dependency tree for a format used by exactly one file: `set`
// preserves what it does not understand by rewriting only the line it
// changes, so an unrecognised construct survives untouched rather than
// being reformatted away.
//
// Values are read as raw text with surrounding quotes stripped. Every key
// declared above is a number, and a quoted number still parses, so a
// hand-written `floor = "2"` works rather than being rejected on a
// technicality nobody would predict.
// ---------------------------------------------------------------------

fn split_key(key: &str) -> (&str, &str) {
    match key.split_once('.') {
        Some((table, name)) => (table, name),
        None => ("", key),
    }
}

fn strip_value(raw: &str) -> String {
    let v = raw.trim();
    let v = v.split_once(" #").map(|(a, _)| a).unwrap_or(v).trim();
    v.trim_matches(|c| c == '"' || c == '\'').to_string()
}

fn read_key(path: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let (want_table, want_name) = split_key(key);
    let mut table = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') || t.is_empty() {
            continue;
        }
        if let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            table = name.trim().to_string();
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            if table == want_table && k.trim() == want_name {
                return Some(strip_value(v));
            }
        }
    }
    None
}

/// A value as it should appear on the right of an `=`.
///
/// Numbers and booleans go bare; everything else is quoted. [`read_key`]
/// strips optional quotes either way, so tetel round-trips its own file
/// regardless — but the file is called `config.toml`, the module invites
/// hand-editing, and `model = openai/gpt-5.6-luna` is not TOML. An editor
/// in TOML mode, a linter, or a later move to a real parser would all
/// choke on it, and none of them would be wrong to.
fn toml_value(raw: &str) -> String {
    let v = raw.trim();
    let bare = v.parse::<i64>().is_ok()
        || v.parse::<f64>().is_ok()
        || matches!(v, "true" | "false");
    if bare {
        v.to_string()
    } else {
        // Escapes are enough for the values this file accepts: no key
        // takes a newline, and a quote or backslash in a model name or a
        // verb list would already have failed validation.
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn write_key(existing: &str, key: &str, value: &str, scope: Scope) -> String {
    let value = &toml_value(value);
    let (want_table, want_name) = split_key(key);
    let mut out: Vec<String> = Vec::new();
    let mut table = String::new();
    let mut written = false;
    let mut table_seen = false;

    for line in existing.lines() {
        let t = line.trim();
        if let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            // Leaving a table: if this was the one we wanted and the key
            // was never found, append it before the next header rather
            // than at the end of the file, where it would land in the
            // wrong table.
            if table == want_table && table_seen && !written {
                out.push(format!("{want_name} = {value}"));
                written = true;
            }
            table = name.trim().to_string();
            if table == want_table {
                table_seen = true;
            }
            out.push(line.to_string());
            continue;
        }
        if !written && !t.starts_with('#') && !t.is_empty() {
            if let Some((k, _)) = t.split_once('=') {
                if table == want_table && k.trim() == want_name {
                    out.push(format!("{want_name} = {value}"));
                    written = true;
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }

    if !written {
        if existing.trim().is_empty() {
            out.push(header(scope));
        }
        if !table_seen && !want_table.is_empty() {
            if !out.is_empty() && !out.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
                out.push(String::new());
            }
            out.push(format!("[{want_table}]"));
        }
        out.push(format!("{want_name} = {value}"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

fn remove_key(existing: &str, key: &str) -> String {
    let (want_table, want_name) = split_key(key);
    let mut out: Vec<String> = Vec::new();
    let mut table = String::new();
    for line in existing.lines() {
        let t = line.trim();
        if let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            table = name.trim().to_string();
            out.push(line.to_string());
            continue;
        }
        if let Some((k, _)) = t.split_once('=') {
            if table == want_table && k.trim() == want_name {
                continue;
            }
        }
        out.push(line.to_string());
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

fn header(scope: Scope) -> String {
    let where_ = match scope {
        Scope::Global => {
            "# Tetel configuration. Values here apply to every workspace.\n\
             # A workspace's own config.toml overrides this file."
        }
        Scope::Workspace => {
            "# Tetel configuration for this workspace only.\n\
             # Values here override the global file."
        }
    };
    format!("{where_}\n#\n# Set values with `tetel config <key> <value>`.\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_then_reads_back() {
        let s = write_key("", KEY_GROUNDING_FLOOR, "3", Scope::Global);
        assert!(s.contains("[grounding]"), "{s}");
        assert!(s.contains("floor = 3"), "{s}");
    }

    #[test]
    fn rewrites_in_place_and_keeps_comments() {
        let start = "# keep me\n\n[grounding]\n# and me\nfloor = 2\n";
        let s = write_key(start, KEY_GROUNDING_FLOOR, "5", Scope::Global);
        assert!(s.contains("# keep me"), "{s}");
        assert!(s.contains("# and me"), "{s}");
        assert!(s.contains("floor = 5"), "{s}");
        assert!(!s.contains("floor = 2"), "{s}");
    }

    #[test]
    fn adds_to_an_existing_table_not_after_the_next_one() {
        let start = "[grounding]\nother = 1\n\n[later]\nx = 1\n";
        let s = write_key(start, KEY_GROUNDING_FLOOR, "4", Scope::Global);
        let floor_at = s.find("floor = 4").expect("written");
        let later_at = s.find("[later]").expect("kept");
        assert!(floor_at < later_at, "floor landed in the wrong table:\n{s}");
    }

    #[test]
    fn removing_a_key_leaves_the_rest() {
        let start = "[grounding]\nfloor = 2\nother = 7\n";
        let s = remove_key(start, KEY_GROUNDING_FLOOR);
        assert!(!s.contains("floor"), "{s}");
        assert!(s.contains("other = 7"), "{s}");
    }

    #[test]
    fn set_refuses_an_unknown_key() {
        let e = set(Scope::Global, None, "grounding.nonesuch", "1").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert!(e.to_string().contains("unknown setting"), "{e}");
    }

    #[test]
    fn set_refuses_a_floor_below_one() {
        // The bound the flag already enforces: a floor of 0 leaves
        // nothing ever owed, which is the switched-off flag the design
        // rejected. It must not be reachable through the file either.
        let e = set(Scope::Global, None, KEY_GROUNDING_FLOOR, "0").unwrap_err();
        assert!(e.to_string().contains("at least 1"), "{e}");
        let e = set(Scope::Global, None, KEY_GROUNDING_FLOOR, "two").unwrap_err();
        assert!(e.to_string().contains("whole number"), "{e}");
    }

    #[test]
    fn the_model_key_refuses_a_credential() {
        // The gap this closes is at the value level, not the key level:
        // there is no `verify.api_key` to set, but a plain string key
        // would accept one pasted into the nearest thing to it.
        for credential in [
            "sk-or-v1-0123456789abcdef0123456789abcdef",
            "sk-ant-api03-aaaaaaaaaaaaaaaaaaaaaaaa",
            "sk-proj-abcdefghijklmnopqrstuvwxyz",
            "0123456789abcdef0123456789abcdef",
        ] {
            assert!(
                !accepted(&Accepts::ModelName, credential),
                "accepted a credential-shaped value"
            );
        }
        for model in ["openai/gpt-5.6-luna", "anthropic/claude-opus-5", "meta/llama-3.1:70b"] {
            assert!(accepted(&Accepts::ModelName, model), "refused `{model}`");
        }
    }

    #[test]
    fn the_model_refusal_does_not_echo_what_it_refused() {
        // Repeating a pasted credential into a terminal, a log or a bug
        // report is the harm the rule exists to prevent.
        let secret = "sk-or-v1-should-never-appear-in-output";
        let e = set(Scope::Global, None, KEY_VERIFY_MODEL, secret).unwrap_err();
        let msg = e.to_string();
        assert!(!msg.contains(secret), "the refusal echoed the value: {msg}");
        assert!(msg.contains("vendor/model"), "{msg}");
    }

    #[test]
    fn the_verify_keys_accept_what_they_say_they_accept() {
        assert!(accepted(&Accepts::Bool, "true"));
        assert!(accepted(&Accepts::Bool, "FALSE"));
        assert!(!accepted(&Accepts::Bool, "yes"));

        assert!(accepted(&Accepts::OneOf(VERIFY_APPROACHES), "direct"));
        assert!(accepted(&Accepts::OneOf(VERIFY_APPROACHES), "split"));
        assert!(!accepted(&Accepts::OneOf(VERIFY_APPROACHES), "both"));
        // The three-call re-derivable pipeline the design names is not
        // built. Refused by name rather than accepted and quietly served
        // by one of the two that are.
        assert!(!accepted(&Accepts::OneOf(VERIFY_APPROACHES), "extract"));
        // And the default is the measured one.
        assert_eq!(VERIFY_APPROACHES[0], "split");

        assert!(accepted(&Accepts::SubsetOf(VERIFY_VERBS), "claim"));
        assert!(accepted(&Accepts::SubsetOf(VERIFY_VERBS), "fact, claim ,prose"));
        assert!(accepted(&Accepts::SubsetOf(VERIFY_VERBS), ""));
        assert!(!accepted(&Accepts::SubsetOf(VERIFY_VERBS), "claim,render"));

        // Below the floor a timeout stops bounding anything useful.
        assert!(!accepted(&Accepts::IntAtLeast(1000), "50"));
    }

    #[test]
    fn there_is_no_api_key_setting_and_the_refusal_says_what_there_is() {
        let e = set(Scope::Global, None, "verify.api_key", "anything").unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("unknown setting"), "{msg}");
        // The refusal lists the known keys, so an author who tried is
        // told what they could have written instead.
        assert!(msg.contains(KEY_VERIFY_MODEL), "{msg}");
    }

    #[test]
    fn what_set_writes_is_valid_toml() {
        // `read_key` strips optional quotes, so tetel round-trips its own
        // file either way. The file is still called config.toml, still
        // invites hand-editing, and `model = openai/gpt-5.6-luna` is not
        // TOML — an editor in TOML mode or a later real parser would be
        // right to reject it.
        let s = write_key("", KEY_VERIFY_MODEL, "openai/gpt-5.6-luna", Scope::Global);
        assert!(s.contains("model = \"openai/gpt-5.6-luna\""), "{s}");
        let s = write_key("", KEY_VERIFY_VERBS, "fact,claim", Scope::Global);
        assert!(s.contains("verbs = \"fact,claim\""), "{s}");
        // Numbers and booleans stay bare, which is also what TOML wants.
        assert!(write_key("", KEY_GROUNDING_FLOOR, "3", Scope::Global).contains("floor = 3"));
        assert!(
            write_key("", KEY_VERIFY_ENABLED, "true", Scope::Global).contains("enabled = true")
        );
        // And what it writes, it reads back.
        let dir = std::env::temp_dir().join(format!("tetel-toml-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, write_key("", KEY_VERIFY_MODEL, "openai/gpt-5.6-luna", Scope::Global))
            .unwrap();
        assert_eq!(read_key(&path, KEY_VERIFY_MODEL).as_deref(), Some("openai/gpt-5.6-luna"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_number_too_large_for_its_consumer_is_rejected_not_silently_dropped() {
        // Validating wider than the consumer parses is a silent hole: the
        // value would resolve as `File` rather than `Rejected`, fail the
        // consumer's own parse, and fall back to a default with no
        // warning — because the warning is gated on `Rejected`.
        assert!(!accepted(&Accepts::IntAtLeast(1), "5000000000"));
        assert!(accepted(&Accepts::IntAtLeast(1), "5"));
    }

    #[test]
    fn the_credential_rule_holds_on_every_path_that_could_print_a_value() {
        // Two of three is worth nothing: an author who pastes a key into
        // the file by hand meets the refusal on *read*, not on write.
        assert!(hides_rejected_value(KEY_VERIFY_MODEL));
        assert!(!hides_rejected_value(KEY_GROUNDING_FLOOR));
        assert!(!hides_rejected_value("nonesuch"));
    }

    #[test]
    fn a_quoted_number_is_still_a_number() {
        assert_eq!(strip_value(" \"3\" "), "3");
        assert_eq!(strip_value("3 # a trailing note"), "3");
    }
}
