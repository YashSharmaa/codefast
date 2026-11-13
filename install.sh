#!/bin/sh
# Installs the codefast binary for this machine.
#
#   curl -fsSL https://raw.githubusercontent.com/YashSharmaa/codefast/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/YashSharmaa/codefast/main/install.sh | sh -s -- --bin-dir /opt/bin
#
# It downloads one release asset, checks its SHA-256 against the SHA256SUMS file of the same
# release, and puts the binary into ~/.local/bin (or the directory you name). It never uses
# sudo and never edits a shell profile.

set -u

release_base=${CODEFAST_RELEASE_BASE:-https://github.com/YashSharmaa/codefast/releases}
version=${CODEFAST_VERSION:-latest}
bin_dir=${CODEFAST_INSTALL_DIR:-}
dry_run=0

usage() {
    cat <<'EOF'
Usage: install.sh [--version VERSION] [--bin-dir DIR] [--dry-run]

  --version VERSION  release to install, for example 0.1.1 (default: latest)
  --bin-dir DIR      directory to install into (default: ~/.local/bin)
  --dry-run          print every action without downloading or writing anything
  -h, --help         show this text

Environment: CODEFAST_VERSION, CODEFAST_INSTALL_DIR and CODEFAST_RELEASE_BASE (the release
URL prefix, default https://github.com/YashSharmaa/codefast/releases) set the same things.
EOF
}

err() {
    printf 'install.sh: %s\n' "$*" >&2
}

while [ $# -gt 0 ]; do
    case $1 in
    --version)
        [ $# -ge 2 ] || { err "--version needs a value"; exit 2; }
        version=$2
        shift 2
        ;;
    --version=*)
        version=${1#--version=}
        shift
        ;;
    --bin-dir)
        [ $# -ge 2 ] || { err "--bin-dir needs a value"; exit 2; }
        bin_dir=$2
        shift 2
        ;;
    --bin-dir=*)
        bin_dir=${1#--bin-dir=}
        shift
        ;;
    --dry-run)
        dry_run=1
        shift
        ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        err "unknown argument: $1"
        usage >&2
        exit 2
        ;;
    esac
done

if [ -z "$bin_dir" ]; then
    if [ -z "${HOME:-}" ]; then
        err "HOME is not set: pass --bin-dir DIR"
        exit 2
    fi
    bin_dir=$HOME/.local/bin
fi
case $bin_dir in
/*) ;;
*) bin_dir=$(pwd)/$bin_dir ;;
esac
[ -n "$version" ] || version=latest
version=${version#v}

# Prints the Rust target triple of the release asset for this machine.
detect_target() {
    os=$(uname -s 2>/dev/null) || os=unknown
    arch=$(uname -m 2>/dev/null) || arch=unknown
    case $arch in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) return 1 ;;
    esac
    case $os in
    Darwin)
        # A shell translated by Rosetta reports x86_64 on Apple silicon.
        if [ "$arch" = x86_64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ]; then
            arch=aarch64
        fi
        printf '%s-apple-darwin\n' "$arch"
        ;;
    Linux) printf '%s-unknown-linux-musl\n' "$arch" ;;
    *) return 1 ;;
    esac
}

# fetch URL FILE
fetch() {
    if command -v curl >/dev/null 2>&1; then
        case $1 in
        https://*) curl -fsSL --proto '=https' --proto-redir '=https' --connect-timeout 10 --max-time 300 -o "$2" "$1" ;;
        *) curl -fsSL --connect-timeout 10 --max-time 300 -o "$2" "$1" ;;
        esac
    elif command -v wget >/dev/null 2>&1; then
        wget -q -T 20 -O "$2" "$1"
    else
        err "neither curl nor wget is installed"
        return 1
    fi
}

# sha256_of FILE: prints the lowercase hex digest.
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        out=$(sha256sum "$1") || return 1
    elif command -v shasum >/dev/null 2>&1; then
        out=$(shasum -a 256 "$1") || return 1
    elif command -v openssl >/dev/null 2>&1; then
        out=$(openssl dgst -sha256 -r "$1") || return 1
    else
        err "no SHA-256 tool found (sha256sum, shasum or openssl)"
        return 1
    fi
    digest=${out%%[!0-9a-f]*}
    [ "${#digest}" -eq 64 ] || return 1
    printf '%s\n' "$digest"
}

# sum_for SUMS_FILE NAME: prints the digest that a `sha256sum`-format file lists for NAME.
sum_for() {
    cr=$(printf '\r')
    while read -r digest name || [ -n "$digest" ]; do
        name=${name%"$cr"}
        case $name in
        "$2" | "*$2" | "./$2")
            case $digest in
            *[!0-9a-f]*) return 1 ;;
            esac
            [ "${#digest}" -eq 64 ] || return 1
            printf '%s\n' "$digest"
            return 0
            ;;
        esac
    done <"$1"
    return 1
}

# unpublished URL: true while URL still names the stand-in owner that this repository carries
# until it has a public home. That name is somebody else's account on the forge, so nothing is
# ever fetched from it. The name is put together here, in two halves, so that replacing the
# stand-in everywhere leaves this test alone and turns it off.
unpublished() {
    standin=OWN
    standin=${standin}ER
    case $1 in
    *"/$standin/"*) return 0 ;;
    esac
    return 1
}

if ! target=$(detect_target); then
    err "there is no prebuilt codefast for $(uname -s 2>/dev/null) $(uname -m 2>/dev/null)"
    err "build it from source instead: cargo install --git https://github.com/YashSharmaa/codefast"
    exit 1
fi

asset=codefast-$target.tar.gz
if [ "$version" = latest ]; then
    from=$release_base/latest/download
else
    from=$release_base/download/v$version
fi
dest=$bin_dir/codefast

on_path=0
case ":${PATH:-}:" in
*":$bin_dir:"*) on_path=1 ;;
esac

if [ "$dry_run" = 1 ]; then
    echo "dry run: nothing is downloaded or written"
    if unpublished "$from"; then
        echo "note:           this copy of codefast has no published release yet; without --dry-run this script stops here"
    fi
    echo "platform:       $target"
    echo "would download: $from/$asset"
    echo "would download: $from/SHA256SUMS"
    echo "would verify:   SHA-256 of $asset against its line in SHA256SUMS"
    echo "would unpack:   $asset in a temporary directory"
    [ -d "$bin_dir" ] || echo "would create:   $bin_dir"
    if [ -e "$dest" ]; then
        echo "would replace:  $dest"
    else
        echo "would install:  $dest"
    fi
    echo "would run:      $dest --version"
    if [ "$on_path" = 0 ]; then
        echo "would tell you: $bin_dir is not on PATH (no shell profile is edited)"
    fi
    exit 0
fi

if unpublished "$from"; then
    err "this copy of codefast has no published release to download yet"
    err "build it from source instead (cargo build --release), or name a release with CODEFAST_RELEASE_BASE"
    exit 1
fi

tmp=
staged=
cleanup() {
    [ -n "$staged" ] && rm -f "$staged"
    [ -n "$tmp" ] && rm -rf "$tmp"
    staged=
    tmp=
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

if ! tmp=$(mktemp -d "${TMPDIR:-/tmp}/codefast-install.XXXXXX"); then
    tmp=
    err "cannot create a temporary directory"
    exit 1
fi

echo "downloading $from/$asset"
if ! fetch "$from/$asset" "$tmp/$asset"; then
    err "could not download $from/$asset"
    exit 1
fi
if ! fetch "$from/SHA256SUMS" "$tmp/SHA256SUMS"; then
    err "could not download $from/SHA256SUMS"
    exit 1
fi
if ! want=$(sum_for "$tmp/SHA256SUMS" "$asset"); then
    err "SHA256SUMS has no entry for $asset"
    exit 1
fi
if ! got=$(sha256_of "$tmp/$asset"); then
    err "could not compute the SHA-256 of the download"
    exit 1
fi
if [ "$got" != "$want" ]; then
    err "checksum mismatch for $asset: expected $want, got $got"
    exit 1
fi
echo "checksum ok ($got)"

if ! mkdir "$tmp/x" || ! tar -xzf "$tmp/$asset" -C "$tmp/x"; then
    err "could not unpack $asset"
    exit 1
fi
if [ ! -f "$tmp/x/codefast" ]; then
    err "$asset does not contain a codefast binary"
    exit 1
fi

if ! mkdir -p "$bin_dir"; then
    err "cannot create $bin_dir (choose another directory with --bin-dir; this script does not use sudo)"
    exit 1
fi
# Copy next to the destination first, then rename: the replacement is atomic and works while
# an older codefast is running.
staged=$bin_dir/.codefast.$$
if ! cp "$tmp/x/codefast" "$staged" || ! chmod 755 "$staged"; then
    err "cannot write $dest (choose another directory with --bin-dir; this script does not use sudo)"
    exit 1
fi
# Try the new binary before it replaces anything: an asset that does not run here must not
# cost you the codefast you already have.
if ! installed=$("$staged" --version </dev/null 2>/dev/null); then
    err "the downloaded codefast does not run on this machine; nothing was installed or replaced"
    exit 1
fi
if ! mv -f "$staged" "$dest"; then
    err "cannot write $dest (choose another directory with --bin-dir; this script does not use sudo)"
    exit 1
fi
staged=
echo "installed $installed at $dest"

if [ "$on_path" = 0 ]; then
    echo
    echo "$bin_dir is not on your PATH. Add it in your shell profile, for example:"
    echo "  export PATH=\"$bin_dir:\$PATH\""
fi
echo
echo "next: 'codefast setup' shows how it would register with the coding harnesses found on this machine,"
echo "      'codefast setup --all --apply' does it, and 'codefast doctor' checks the result"
