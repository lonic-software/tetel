#!/bin/sh
# tetel installer — macOS, Linux, and Git Bash on Windows.
#
#   curl -fsSL https://raw.githubusercontent.com/lonic-software/tetel/main/install.sh | sh
#
# Environment overrides:
#   TETEL_VERSION      install a specific tag, e.g. v0.1.0 (default: latest release)
#   TETEL_INSTALL_DIR  where to put the binary            (default: ~/.local/bin)
#   TETEL_REPO         GitHub repo slug                   (default: lonic-software/tetel)
#   TETEL_BASE_URL     full base URL for the assets (mirrors / air-gapped setups);
#                      overrides TETEL_REPO/TETEL_VERSION entirely. Asset names
#                      carry no version, so nothing else is needed.
#   TETEL_NO_VERIFY    set to 1 to install without a checksums.txt (or a sha256
#                      tool) to verify against
set -eu

REPO="${TETEL_REPO:-lonic-software/tetel}"
VERSION="${TETEL_VERSION:-latest}"
INSTALL_DIR="${TETEL_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
err() { printf 'install.sh: error: %s\n' "$*" >&2; exit 1; }

# ── detect platform ─────────────────────────────────────────────
os=$(uname -s)
case "$os" in
    Darwin)               suffix="apple-darwin";       ext="tar.gz" ;;
    Linux)                suffix="unknown-linux-musl"; ext="tar.gz" ;;
    MINGW*|MSYS*|CYGWIN*) suffix="pc-windows-msvc";    ext="zip" ;;
    *) err "unsupported OS: $os" ;;
esac

arch=$(uname -m)
case "$arch" in
    arm64|aarch64) arch="aarch64" ;;
    x86_64|amd64)  arch="x86_64" ;;
    *) err "unsupported architecture: $arch" ;;
esac
if [ "$suffix" = "pc-windows-msvc" ] && [ "$arch" = "aarch64" ]; then
    err "no Windows ARM build yet — build from source: cargo install --path ."
fi

target="${arch}-${suffix}"
asset="tetel-${target}.${ext}"
if [ -n "${TETEL_BASE_URL:-}" ]; then
    base="$TETEL_BASE_URL"
elif [ "$VERSION" = "latest" ]; then
    base="https://github.com/${REPO}/releases/latest/download"
else
    base="https://github.com/${REPO}/releases/download/${VERSION}"
fi

# ── download ────────────────────────────────────────────────────
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$1" -O "$2"
    else
        err "need curl or wget"
    fi
}

say "downloading ${base}/${asset}"
fetch "${base}/${asset}" "${tmp}/${asset}" \
    || err "download failed — does a release exist for ${target}?"

# ── verify ──────────────────────────────────────────────────────
# Refuse to install unverified unless asked to: every release this repo
# publishes carries checksums.txt, so its absence means something is wrong.
unverified() {
    [ "${TETEL_NO_VERIFY:-}" = "1" ] \
        || err "$1 — refusing to install unverified; set TETEL_NO_VERIFY=1 to override"
    say "warning: $1; installing UNVERIFIED (TETEL_NO_VERIFY=1)"
}
if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
    unverified "neither sha256sum nor shasum is available"
elif ! fetch "${base}/checksums.txt" "${tmp}/checksums.txt"; then
    unverified "could not fetch ${base}/checksums.txt"
else
    grep " ${asset}\$" "${tmp}/checksums.txt" > "${tmp}/asset.sum" \
        || err "${asset} missing from checksums.txt"
    (
        cd "$tmp"
        if command -v sha256sum >/dev/null 2>&1; then
            sha256sum -c asset.sum >/dev/null
        else
            shasum -a 256 -c asset.sum >/dev/null
        fi
    ) || err "checksum verification FAILED — refusing to install"
    say "checksum ok"
fi

# ── unpack + install ────────────────────────────────────────────
bin="tetel"
case "$ext" in
    tar.gz) tar -xzf "${tmp}/${asset}" -C "$tmp" ;;
    zip)
        bin="tetel.exe"
        command -v unzip >/dev/null 2>&1 || err "unzip is required"
        unzip -q "${tmp}/${asset}" -d "$tmp"
        ;;
esac
[ -f "${tmp}/${bin}" ] || err "archive did not contain ${bin}"

dest="${INSTALL_DIR}/${bin}"
upgrading=""
[ -e "$dest" ] && upgrading=1

# Stage beside the destination, then rename over it. A `tetel mcp` server
# may be running from $dest, and writing into a running executable fails on
# Linux (ETXTBSY); a rename leaves the server on the old file and puts the
# new one at the path. Windows refuses even the rename onto a running .exe
# but allows renaming the running one away, so it goes aside first — to a
# fresh name, since an earlier aside may itself still be running — and comes
# back if the final rename fails.
mkdir -p "$INSTALL_DIR"
staged="${INSTALL_DIR}/.${bin}.new.$$"
cp "${tmp}/${bin}" "$staged"
chmod 755 "$staged"
aside=""
if [ "$ext" = "zip" ] && [ -e "$dest" ]; then
    aside="${dest}.old-$(date +%s)-$$"
    mv -f "$dest" "$aside" || { rm -f "$staged"; err "could not move ${dest} aside"; }
fi
if ! mv -f "$staged" "$dest"; then
    [ -n "$aside" ] && { mv -f "$aside" "$dest" || true; }
    rm -f "$staged"
    err "could not replace ${dest}"
fi
if [ -n "$aside" ]; then
    # Best effort: asides a server still runs from stay locked and are skipped.
    for old in "${dest}".old-*; do
        [ "$old" = "$aside" ] || rm -f "$old" 2>/dev/null || true
    done
fi

say "installed ${dest} — $("$dest" --version 2>/dev/null || echo 'tetel')"

case ":$PATH:" in
    *":${INSTALL_DIR}:"*)
        # On PATH, but something earlier may win — e.g. an older
        # `cargo install` in ~/.cargo/bin.
        found=$(command -v "$bin" 2>/dev/null || true)
        if [ -n "$found" ] && [ "$found" != "$dest" ]; then
            say ""
            say "warning: \`${bin}\` on your PATH resolves to ${found}, not ${dest}."
            say "Remove the other copy, or put ${INSTALL_DIR} earlier on your PATH."
        fi
        ;;
    *)
        say ""
        say "note: ${INSTALL_DIR} is not on your PATH. Add this to your shell profile:"
        say "    export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
esac

# Runtime dependencies — tetel needs these to run, not to install.
# Warn, don't fail: the user may be installing before setting up the rest.
missing=""
command -v git >/dev/null 2>&1 || missing="git"
command -v grep >/dev/null 2>&1 || missing="${missing:+$missing and }grep"
if [ -n "$missing" ]; then
    say ""
    say "warning: $missing not found on PATH. tetel runs git to pin observations"
    say "to a commit and grep for its searches — install $missing before using it."
    case "$suffix" in
        pc-windows-msvc) say "On Windows, Git for Windows provides both: https://gitforwindows.org" ;;
    esac
fi

say ""
if [ -n "$upgrading" ]; then
    say "Upgraded. A \`tetel mcp\` server that was already running keeps the old build"
    say "and refuses tool calls until restarted — restart Claude Code to pick this one up."
else
    say "To use it from Claude Code:"
    say "    claude mcp add --scope user tetel -- tetel mcp"
fi
