#!/bin/sh
# Keep execution at the end so a truncated piped download cannot start installation.
install_palepale() (
    set -eu
    fail() { printf 'palepale: %s\n' "$*" >&2; exit 1; }
    command -v curl >/dev/null 2>&1 || fail 'curl is required.'
    : "${HOME:?HOME must be set}"
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Darwin)
            # Rosetta reports x86_64 even on an Apple Silicon Mac.
            if [ "$arch" = arm64 ] || [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
                asset=palepale-aarch64-apple-darwin
            else
                fail 'macOS requires Apple Silicon (M-series). Intel Macs are not supported.'
            fi
            ;;
        Linux)
            [ "$arch" = x86_64 ] || fail "Linux requires x86_64; found $arch."
            asset=palepale-x86_64-unknown-linux-musl
            ;;
        *) fail "Unsupported OS: $os. On Windows, use data/install.ps1 in PowerShell." ;;
    esac
    if command -v sha256sum >/dev/null 2>&1; then
        checksum() { sha256sum "$1"; }
    elif command -v shasum >/dev/null 2>&1; then
        checksum() { shasum -a 256 "$1"; }
    else
        fail 'sha256sum or shasum is required.'
    fi
    temp=$(mktemp -d)
    trap 'rm -rf "$temp"' 0
    trap 'exit 1' HUP INT TERM
    base=https://api.palepale.tech/cli/latest
    printf 'Downloading %s...\n' "$asset"
    curl --proto '=https' --tlsv1.2 -fsSL --retry 3 "$base/SHA256SUMS" -o "$temp/SHA256SUMS"
    curl --proto '=https' --tlsv1.2 -fsSL --retry 3 "$base/$asset" -o "$temp/palepale"
    expected=$(awk -v file="$asset" '$2 == file || $2 == "*" file { print tolower($1) }' "$temp/SHA256SUMS")
    [ "${#expected}" -eq 64 ] || fail 'Missing or ambiguous release checksum.'
    case "$expected" in *[!0-9a-f]*) fail 'Invalid release checksum.' ;; esac
    actual=$(checksum "$temp/palepale" | awk '{ print tolower($1) }')
    [ "$actual" = "$expected" ] || fail 'Checksum mismatch. Please retry the installation.'
    chmod 755 "$temp/palepale"
    "$temp/palepale" install

    line='export PATH="$HOME/.local/bin:$PATH"'
    case "${SHELL:-/bin/sh}" in
        */zsh) profile="${ZDOTDIR:-$HOME}/.zshrc" ;;
        */bash)
            if [ "$os" = Darwin ]; then
                # Bash reads only the first existing login profile.
                if [ -f "$HOME/.bash_profile" ]; then profile="$HOME/.bash_profile"
                elif [ -f "$HOME/.bash_login" ]; then profile="$HOME/.bash_login"
                elif [ -f "$HOME/.profile" ]; then profile="$HOME/.profile"
                else profile="$HOME/.bash_profile"; fi
            else
                profile="$HOME/.bashrc"
            fi
            ;;
        */fish)
            profile="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/palepale.fish"
            line='fish_add_path "$HOME/.local/bin"'
            ;;
        */sh|*/dash|*/ksh) profile="$HOME/.profile" ;;
        *)
            printf 'Add %s/.local/bin to your shell PATH.\n' "$HOME"
            exit 0
            ;;
    esac
    mkdir -p "$(dirname "$profile")"
    if ! grep -Fqx "$line" "$profile" 2>/dev/null; then
        printf '\n# Palepale\n%s\n' "$line" >> "$profile"
    fi
    printf 'Open a new terminal and run palepale, or run "%s/.local/bin/palepale" now.\n' "$HOME"
)

install_palepale "$@"
