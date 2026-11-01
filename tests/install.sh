#!/bin/sh
# Local download fixtures: never fetch or execute a release binary.
set -eu
repo=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
root=$(mktemp -d)
trap 'rm -rf "$root"' 0
mkdir -p "$root/bin" "$root/downloads"
export FIXTURE="$root" PATH="$root/bin:$PATH"
cat > "$root/bin/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo "$TEST_OS" ;; -m) echo "$TEST_ARCH" ;; esac
EOF
cat > "$root/bin/sysctl" <<'EOF'
#!/bin/sh
echo "${TEST_ARM:-0}"
EOF
cat > "$root/bin/curl" <<'EOF'
#!/bin/sh
while [ "$#" -gt 0 ]; do
    case "$1" in
        https://*) url=$1 ;;
        -o) shift; output=$1 ;;
    esac
    shift
done
[ "${TEST_DOWNLOAD_FAIL:-0}" = 0 ] || exit 22
file=${url##*/}
printf '%s\n' "$file" >> "$FIXTURE/requests"
cp "$FIXTURE/downloads/$file" "$output"
EOF
cat > "$root/downloads/binary" <<'EOF'
#!/bin/sh
[ "$1" = install ] || exit 9
[ "${TEST_INSTALL_FAIL:-0}" = 0 ] || exit 10
printf 'installed\n' >> "$HOME/installed"
EOF
chmod +x "$root/bin/"*
for asset in palepale-aarch64-apple-darwin palepale-x86_64-unknown-linux-musl; do
    cp "$root/downloads/binary" "$root/downloads/$asset"
done
if command -v sha256sum >/dev/null 2>&1; then
    hash=$(sha256sum "$root/downloads/binary" | awk '{print $1}')
else
    hash=$(shasum -a 256 "$root/downloads/binary" | awk '{print $1}')
fi
manifest() {
    printf '%s  palepale-aarch64-apple-darwin\n%s  palepale-x86_64-unknown-linux-musl\n' "$hash" "$hash" > "$root/downloads/SHA256SUMS"
}
fresh() {
    HOME="$root/home $1"
    export HOME TEST_OS=Darwin TEST_ARCH=arm64 TEST_ARM=0 SHELL=/bin/zsh
    export TEST_DOWNLOAD_FAIL=0 TEST_INSTALL_FAIL=0
    unset ZDOTDIR XDG_CONFIG_HOME
    mkdir -p "$HOME"
    : > "$root/requests"
    manifest
}
run() { sh "$repo/data/install.sh" > "$root/output" 2>&1; }
reject() {
    if run; then cat "$root/output"; exit 1; fi
    [ ! -f "$HOME/installed" ]
    [ ! -f "$HOME/.zshrc" ]
}
fresh native
run
run
test "$(grep -c '^export PATH=' "$HOME/.zshrc")" = 1
grep -qx palepale-aarch64-apple-darwin "$root/requests"
fresh rosetta
export TEST_ARCH=x86_64 TEST_ARM=1
run
grep -qx palepale-aarch64-apple-darwin "$root/requests"
fresh intel
export TEST_ARCH=x86_64
reject
test ! -s "$root/requests"
fresh linux
export TEST_OS=Linux TEST_ARCH=x86_64 SHELL=/bin/bash
run
grep -qx palepale-x86_64-unknown-linux-musl "$root/requests"
test -f "$HOME/.bashrc"
fresh arm_linux
export TEST_OS=Linux TEST_ARCH=aarch64
reject
fresh fish
export SHELL=/usr/bin/fish
run
grep -q fish_add_path "$HOME/.config/fish/conf.d/palepale.fish"
fresh bash_login
export SHELL=/bin/bash
printf '# Existing profile\n' > "$HOME/.profile"
run
grep -q '^export PATH=' "$HOME/.profile"
test ! -f "$HOME/.bash_profile"
fresh corrupt
printf '%064d  palepale-aarch64-apple-darwin\n' 0 > "$root/downloads/SHA256SUMS"
reject
fresh missing
: > "$root/downloads/SHA256SUMS"
reject
fresh duplicate
cat "$root/downloads/SHA256SUMS" >> "$root/duplicate"
cat "$root/duplicate" >> "$root/downloads/SHA256SUMS"
reject
fresh download_failure
export TEST_DOWNLOAD_FAIL=1
reject
fresh install_failure
export TEST_INSTALL_FAIL=1
reject
printf 'Shell installer: 12 fixture scenarios passed.\n'
