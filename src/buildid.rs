//! Which build produced this output, and whether the process producing it
//! is still the build that is installed.
//!
//! # The defect this exists for
//!
//! `cargo install` replaces the binary **by rename**, which gives the new
//! file a new inode and leaves an already-running process holding the old
//! one. A long-lived `tetel mcp` server therefore keeps serving whatever
//! build it started with, arbitrarily far behind the installed one, and
//! nothing in the protocol or in any tool result says so. This is not
//! theoretical: it has been recorded three times — twice on 2026-08-07,
//! the second of which produced a **wrong** `check` verdict that was
//! believed and relayed as fact, and once on 2026-08-06 with two server
//! processes serving a build sixteen commits stale.
//!
//! It is the worst shape of failure this project knows: it does not
//! error, it answers. And `lonic-designer`, `lonic-grounder` and
//! `lonic-design-attacker` reach tetel through MCP and have no other read
//! path, so a stale server means every agent in the authoring loop grades
//! with an old checker and none of them can see it from inside.
//!
//! # Detection, not refresh
//!
//! A server cannot reload itself — the harness owns the process, and no
//! hook or handshake inside tetel can make it pick up a new binary. So
//! this module does not try. It answers two questions instead:
//!
//! - [`label`] — *which build produced this output.* A verdict that names
//!   its checker can be disbelieved; one that does not, cannot. Naming it
//!   is what would have caught 2026-08-07 at the moment it happened
//!   rather than three exchanges later, when the CLI was finally run as a
//!   tiebreak.
//! - [`freshness`] — *is the file this process was launched from still
//!   this process.* If not, the caller is talking to a build that is no
//!   longer installed.
//!
//! # How identity is computed
//!
//! A SHA-256 over the running executable's own bytes, read once at
//! startup and cached. This was picked over a compile-time identity
//! stamped in by a build script because the comparison needs both sides
//! in one namespace: the on-disk side can then be read as a *file*,
//! whereas a compiled-in stamp could only be recovered from the installed
//! binary by executing it, which means spawning a process on a path that
//! may by then be something else entirely.
//!
//! The version string alone is not enough and never was. Every stale
//! build recorded above carried `0.1.0`, exactly like the installed one.
//!
//! # Why the capture must happen early
//!
//! [`capture`] is called at server startup rather than lazily on first
//! use. On Linux `std::env::current_exe()` reads `/proc/self/exe`, which
//! after a rename-replace resolves to the old inode with a `(deleted)`
//! suffix — a path that no longer exists. Resolving it while the file is
//! still the one we were launched from gets a usable path, and every
//! later re-read goes to that path, which is where the replacement lands.
//! On macOS the path is recorded at exec and stays usable either way; the
//! early call costs nothing there and makes the two platforms agree.
//!
//! # Known limits
//!
//! **The label identifies an executable file, not a source revision.**
//! `cargo install` post-processes what `cargo build --release` produced —
//! on this machine, `target/release/tetel` and the installed
//! `~/.cargo/bin/tetel` built from the same commit in the same command
//! carry different digests. So two labels differing does not by itself
//! prove the *source* differed; it proves the two answers came out of two
//! different files, which is the question that was actually being asked
//! when this was wanted. Comparing outputs from the same path — the CLI
//! and the MCP server both launched from `~/.cargo/bin/tetel`, which is
//! the case that has bitten — is exact.
//!
//! Correspondingly, [`freshness`] only ever compares a path against
//! itself over time, never one path against another. It cannot tell you
//! that a server launched from `target/debug` is behind
//! `~/.cargo/bin` — those are two installations, not one that moved.
//!
//! If the executable cannot be named or read — a chroot, a hardened
//! sandbox, a `current_exe` that fails — this reports
//! [`Freshness::Undeterminable`] with the reason, and [`label`] says the
//! build is unidentifiable. It never guesses, and never reports
//! [`Freshness::Current`] on a failure to look: an unreadable record is
//! not a matching one, the same rule `snapshot::check` follows for
//! provenance.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

/// Hex characters of the digest shown in a label. Twelve is enough that
/// two builds of this crate colliding is not a thing that happens, and
/// short enough to sit at the end of a line a human reads.
const SHORT: usize = 12;

struct SelfImage {
    path: PathBuf,
    digest: String,
    len: u64,
    mtime: Option<SystemTime>,
}

enum SelfId {
    Known(SelfImage),
    Unavailable(String),
}

static SELF: OnceLock<SelfId> = OnceLock::new();

fn digest_of(path: &Path) -> std::io::Result<(String, u64, Option<SystemTime>)> {
    let bytes = std::fs::read(path)?;
    let meta = std::fs::metadata(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok((format!("{:x}", hasher.finalize()), meta.len(), meta.modified().ok()))
}

fn self_id() -> &'static SelfId {
    SELF.get_or_init(|| match std::env::current_exe() {
        Err(e) => SelfId::Unavailable(format!("this process cannot name its own executable ({e})")),
        Ok(path) => match digest_of(&path) {
            Err(e) => SelfId::Unavailable(format!("{} could not be read ({e})", path.display())),
            Ok((digest, len, mtime)) => SelfId::Known(SelfImage { path, digest, len, mtime }),
        },
    })
}

/// Read and cache this process's own identity now. Idempotent.
///
/// Call this before a long-lived process starts serving. See the module
/// doc comment for why waiting until first use is not equivalent.
pub fn capture() {
    let _ = self_id();
}

/// Names the build that produced whatever output this label is attached
/// to — version plus a short digest of the running executable.
pub fn label() -> String {
    let version = env!("CARGO_PKG_VERSION");
    match self_id() {
        SelfId::Known(image) => format!("tetel {version} build {}", &image.digest[..SHORT]),
        SelfId::Unavailable(why) => {
            format!("tetel {version} build unidentifiable — {why}")
        }
    }
}

/// Whether the file this process was launched from is still this process.
pub enum Freshness {
    /// The executable on disk is byte-identical to the one running.
    Current,
    /// The executable on disk is a different build from the one running:
    /// this process is serving something that is no longer installed.
    Stale { running: String, installed: String, path: String },
    /// Neither could be established. Never conflated with `Current`.
    Undeterminable(String),
}

/// Compare the running build against the one now on disk at the path this
/// process was launched from.
///
/// Cheap in the common case: length and mtime are compared first and the
/// bytes are only re-hashed when one of them moved. Re-installing an
/// identical build therefore reports [`Freshness::Current`] rather than a
/// spurious staleness — a warning that fires when nothing changed is a
/// warning that stops being read.
pub fn freshness() -> Freshness {
    let image = match self_id() {
        SelfId::Known(image) => image,
        SelfId::Unavailable(why) => return Freshness::Undeterminable(why.clone()),
    };
    let path = image.path.display().to_string();
    match std::fs::metadata(&image.path) {
        Ok(meta) if meta.len() == image.len && meta.modified().ok() == image.mtime => {
            Freshness::Current
        }
        Ok(_) => match digest_of(&image.path) {
            Ok((digest, _, _)) if digest == image.digest => Freshness::Current,
            Ok((digest, _, _)) => Freshness::Stale {
                running: image.digest[..SHORT].to_string(),
                installed: digest[..SHORT].to_string(),
                path,
            },
            Err(e) => Freshness::Undeterminable(format!(
                "{path} changed since this process started but could not be re-read ({e})"
            )),
        },
        Err(e) => Freshness::Undeterminable(format!(
            "{path} was readable when this process started and is not now ({e}) — \
something moved or removed it"
        )),
    }
}

/// The text a stale server refuses with. Shared by every surface so the
/// two cannot drift into describing the same condition differently.
pub fn stale_guidance(running: &str, installed: &str, path: &str) -> String {
    format!(
        "this MCP server is running a build that is no longer installed.\n\n\
  running build:   {running}\n\
  installed build: {installed}\n\
  binary:          {path}\n\n\
`cargo install` replaces the binary by rename, so this process still holds the file it opened at \
startup while the path above now holds a different one. Every answer this process would return — \
`check` most of all — comes from the older build, and nothing in the answer would say so. That has \
already produced a failing `check` on a memo the installed build reports clean.\n\n\
A server cannot reload itself; the harness owns the process. Restart the client that launched it \
(for Claude Code, a full restart) and the next call runs the installed build. There is no override: \
a stale checker answering confidently is the whole of what this refusal prevents."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_that_has_not_been_replaced_is_current() {
        // The test binary is not rewritten underneath itself, so this is
        // the stuck-red half of the falsifier: the detector must stay
        // quiet when nothing happened.
        assert!(matches!(freshness(), Freshness::Current));
    }

    #[test]
    fn the_label_always_names_a_version_and_never_panics() {
        let l = label();
        assert!(l.starts_with(&format!("tetel {}", env!("CARGO_PKG_VERSION"))), "{l}");
    }

    #[test]
    fn a_label_from_a_readable_binary_carries_a_short_digest() {
        // Guards the slice: a digest shorter than `SHORT` would panic
        // rather than degrade, and this asserts the shape callers compare.
        let l = label();
        if !l.contains("unidentifiable") {
            let digest = l.rsplit(' ').next().unwrap();
            assert_eq!(digest.len(), SHORT, "{l}");
            assert!(digest.chars().all(|c| c.is_ascii_hexdigit()), "{l}");
        }
    }

    #[test]
    fn stale_guidance_names_both_builds_the_path_and_the_remedy() {
        let g = stale_guidance("aaaaaaaaaaaa", "bbbbbbbbbbbb", "/somewhere/tetel");
        assert!(g.contains("aaaaaaaaaaaa"));
        assert!(g.contains("bbbbbbbbbbbb"));
        assert!(g.contains("/somewhere/tetel"));
        assert!(g.contains("Restart"));
    }
}
