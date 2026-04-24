#!/bin/sh
# shellcheck shell=dash
#
# akua install script — `curl -fsSL https://akua.dev/install | sh`
#
# Downloads a prebuilt `akua` binary from GitHub Releases into
# $AKUA_INSTALL/bin (defaulting to $HOME/.akua/bin), and prints the
# `export PATH=…` line to paste into your shell config.
#
# We deliberately don't edit ~/.bashrc / ~/.zshrc / ~/.config/fish / etc
# for you — Bun tried and the script ballooned to 300 LOC of shell-rc
# detection. Printing the one line users need to paste is cleaner.
#
# Optional args:
#   $1   version tag (e.g. `v0.1.0`); defaults to latest via GitHub redirect.
#
# Optional env:
#   AKUA_INSTALL       install root (default: $HOME/.akua)
#   AKUA_DOWNLOAD_BASE download host (default: github.com, for CDN mirrors)
#
# Keep this script simple and easily auditable. If something gets
# hairy, it probably belongs in `akua` itself, not here.

set -eu

main() {
    need_cmd curl
    need_cmd tar
    need_cmd uname

    local version
    version="${1:-}"

    local triple
    triple="$(detect_triple)"

    local resolved_version
    resolved_version="$(resolve_version "$version")"

    local base="${AKUA_DOWNLOAD_BASE:-https://github.com}"
    local asset="akua-${resolved_version}-${triple}.tar.gz"
    local url="${base}/cnap-tech/akua/releases/download/akua-${resolved_version}/${asset}"

    local install_root="${AKUA_INSTALL:-$HOME/.akua}"
    local bin_dir="${install_root}/bin"

    info "downloading akua ${resolved_version} (${triple})"
    info "  from  ${url}"
    info "  to    ${bin_dir}/akua"

    mkdir -p "$bin_dir" || error "cannot create ${bin_dir}"
    local tmpdir
    tmpdir="$(mktemp -d 2>/dev/null || mktemp -d -t 'akua')"
    trap 'rm -rf "$tmpdir"' EXIT

    curl -fsSL "$url" -o "$tmpdir/akua.tar.gz" \
        || error "download failed from ${url}"

    tar -xzf "$tmpdir/akua.tar.gz" -C "$tmpdir" \
        || error "extract failed (corrupt archive?)"

    [ -f "$tmpdir/akua" ] || error "archive did not contain the akua binary"

    mv "$tmpdir/akua" "$bin_dir/akua"
    chmod +x "$bin_dir/akua"

    success "installed akua ${resolved_version} to ${bin_dir}/akua"
    printf '\n'

    if ! echo ":$PATH:" | grep -q ":${bin_dir}:"; then
        info "add to your shell config:"
        printf '\n    export PATH="%s:$PATH"\n\n' "$bin_dir"
    fi

    info "verify:  ${bin_dir}/akua --version"
}

# ---------------------------------------------------------------------------
# Target detection
# ---------------------------------------------------------------------------

detect_triple() {
    local sysname machine triple
    sysname="$(uname -s)"
    machine="$(uname -m)"

    case "$sysname" in
        Linux)
            # Alpine uses musl, not glibc. We don't ship musl builds yet —
            # bail rather than give them a broken glibc binary that fails
            # at runtime with a confusing dynamic-linker error.
            if [ -f /etc/alpine-release ]; then
                error "Alpine/musl not yet supported. Build from source:\n\n    cargo install --git https://github.com/cnap-tech/akua akua-cli\n"
            fi
            case "$machine" in
                x86_64|amd64)  triple="x86_64-unknown-linux-gnu" ;;
                aarch64|arm64) triple="aarch64-unknown-linux-gnu" ;;
                *) error "unsupported linux arch: $machine" ;;
            esac
            ;;
        Darwin)
            # If we're running under Rosetta, prefer the native arm64
            # binary (Rosetta can run x86_64, but the arm64 one is faster
            # and matches what `uname -m` returned if run natively).
            if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
                machine="arm64"
            fi
            case "$machine" in
                x86_64)        triple="x86_64-apple-darwin" ;;
                arm64|aarch64) triple="aarch64-apple-darwin" ;;
                *) error "unsupported darwin arch: $machine" ;;
            esac
            ;;
        MINGW*|MSYS*|CYGWIN*)
            error "for Windows use:\n\n    powershell -c \"irm https://akua.cnap.tech/install.ps1 | iex\"\n"
            ;;
        *)
            error "unsupported OS: $sysname"
            ;;
    esac
    echo "$triple"
}

# ---------------------------------------------------------------------------
# Version resolution
# ---------------------------------------------------------------------------

resolve_version() {
    local input="$1"
    if [ -n "$input" ]; then
        # Accept `v0.1.0`, `0.1.0`, or `akua-v0.1.0` — normalise to `v0.1.0`.
        echo "$input" | sed -e 's|^akua-||' -e 's|^v\{0,1\}|v|'
        return
    fi
    # `releases/latest/download/...` redirects per-asset; to reconstruct
    # the asset URL we need the version. Cheapest way: follow the HEAD
    # redirect on `/releases/latest` itself.
    local location
    location="$(curl -fsSLI -o /dev/null -w '%{url_effective}\n' \
        https://github.com/cnap-tech/akua/releases/latest)"
    # URL ends with .../tag/akua-vX.Y.Z
    echo "$location" | sed -e 's|.*/tag/akua-||'
}

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

need_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        error "required command \`$1\` not found on PATH"
    fi
}

# Colour only when stdout is a TTY. Keep output scrape-friendly in CI.
if [ -t 1 ]; then
    _reset='\033[0m'
    _bold='\033[1m'
    _red='\033[31m'
    _green='\033[32m'
    _blue='\033[34m'
else
    _reset='' _bold='' _red='' _green='' _blue=''
fi

info()    { printf '%b→%b %s\n' "$_blue" "$_reset" "$1"; }
success() { printf '%b✓%b %s\n' "$_green" "$_reset" "$1"; }
error()   { printf '%b✗%b %b%s%b\n' "$_red" "$_reset" "$_bold" "$1" "$_reset" >&2; exit 1; }

main "$@"
