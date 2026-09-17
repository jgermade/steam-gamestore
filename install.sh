#!/bin/sh
# Install gamestore, or update the one that is already here.
#
# Three ways in, and it works out which one it is:
#
#   ./install.sh                                  from a checkout: builds it
#   curl -fsSL <raw>/install.sh | sh              downloads the released binary
#   curl -fsSL <raw>/install.sh | sh -s -- --from-source
#
# Re-running the one-liner is how gamestore is updated. It looks at what is
# already installed first and asks before touching it: to update when there is a
# newer release, to reinstall when there is not. With no terminal to ask on it
# updates when there is something newer and leaves an up-to-date install alone.
# Building from source — a checkout, or --from-source — never asks: that is an
# explicit "install what is in front of me".
#
# Nothing here touches the system: the binary goes under a prefix in the home
# directory, which is what makes this work unchanged on an immutable SteamOS as
# well as on an ordinary Arch or CachyOS install.
#
# POSIX sh on purpose — it is piped into whatever /bin/sh happens to be.
set -eu

repository="jgermade/steam-gamestore"
target="x86_64-unknown-linux-gnu"

prefix="${HOME}/.local"
profile="release"
from_source=0
version="latest"
force=0

usage() {
    cat <<'USAGE'
Usage: install.sh [--prefix DIR] [--from-source] [--version TAG] [--debug] [--force]

  --prefix DIR     Install into DIR/bin (default: ~/.local)
  --from-source    Build from source even when a released binary is available
  --version TAG    Install a named release (default: the latest one)
  --debug          Build without optimizations, for a faster edit-run loop
  --force          Install without asking, even over an up-to-date install
  -h, --help       This message
USAGE
}

say() { printf '%s\n' "$*"; }
step() { printf '==> %s\n' "$*"; }
fail() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) prefix="${2:?--prefix needs a directory}"; shift 2 ;;
        --version) version="${2:?--version needs a tag}"; shift 2 ;;
        --from-source) from_source=1; shift ;;
        --debug) profile="debug"; from_source=1; shift ;;
        --force) force=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'install.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

destination="${prefix}/bin/gamestore"

# Where this script is, when it is a file at all. Piped into sh it is not, and
# `$0` is the shell's own name.
script_dir=""
if [ -n "${0:-}" ] && [ -f "$0" ]; then
    script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
fi

in_checkout=0
if [ -n "$script_dir" ] && [ -f "${script_dir}/Cargo.toml" ] && [ -f "${script_dir}/cli/Cargo.toml" ]; then
    in_checkout=1
fi

work=""
# Ends in a plain `if` on purpose: under `set -e` a trap whose last command fails
# takes the whole script's exit status down with it, and an empty $work is the
# ordinary case on the paths that never make a temporary directory.
cleanup() {
    if [ -n "$work" ]; then
        rm -rf "$work"
    fi
}
trap cleanup EXIT INT TERM

download() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        return 127
    fi
}

# Is there a terminal to ask a question on? Piped into sh, stdin is the script
# itself, so both the question and the answer go through /dev/tty or nowhere.
#
# The open is tried in a subshell because a redirection that fails on a special
# built-in — and `exec` is one — takes the whole shell down with it in dash,
# which is /bin/sh on Debian and on the CI runner.
interactive() {
    ( exec 3>/dev/tty ) 2>/dev/null
}

# Ask a yes/no question. $2 is what an empty answer — or an unreadable one —
# means. Only called once `interactive` has said there is a terminal.
ask() {
    case "$2" in
        yes) hint="[Y/n]" ;;
        *) hint="[y/N]" ;;
    esac

    printf '%s %s ' "$1" "$hint" >/dev/tty
    read -r answer </dev/tty || answer=""

    case "$answer" in
        [Yy]|[Yy][Ee][Ss]) return 0 ;;
        [Nn]|[Nn][Oo]) return 1 ;;
        *) [ "$2" = "yes" ] ;;
    esac
}

# The version of the gamestore already installed at $destination, if any.
installed_version() {
    [ -x "$destination" ] || return 0
    "$destination" --version 2>/dev/null | sed -n '1s/.* //p'
}

# The tag of the newest release, from the redirect `releases/latest` answers
# with. Empty when it cannot be worked out — offline, no release cut yet, a rate
# limit — which is not fatal: the caller asks a vaguer question instead.
latest_tag() {
    url="https://github.com/${repository}/releases/latest"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSLI -o /dev/null -w '%{url_effective}' "$url" 2>/dev/null \
            | sed -n 's#.*/releases/tag/##p'
    elif command -v wget >/dev/null 2>&1; then
        wget -qS --spider "$url" 2>&1 \
            | sed -n 's#.*[Ll]ocation: .*/releases/tag/##p' | tr -d '\r' | tail -n 1
    fi
}

# How $1 stands to $2: "newer", "older" or "same". A `v` prefix and anything
# after a dash (`1.2.0-rc1`) are ignored, and a missing field counts as zero, so
# `1.2` and `1.2.0` are the same version.
version_cmp() {
    left="${1#v}"; left="${left%%-*}"
    right="${2#v}"; right="${right%%-*}"

    while [ -n "$left" ] || [ -n "$right" ]; do
        l="${left%%.*}"; r="${right%%.*}"
        case "$l" in ''|*[!0-9]*) l=0 ;; esac
        case "$r" in ''|*[!0-9]*) r=0 ;; esac

        if [ "$l" -gt "$r" ]; then say newer; return 0; fi
        if [ "$l" -lt "$r" ]; then say older; return 0; fi

        case "$left" in *.*) left="${left#*.}" ;; *) left="" ;; esac
        case "$right" in *.*) right="${right#*.}" ;; *) right="" ;; esac
    done

    say same
}

# Decide what to do about an install that is already there, before anything is
# downloaded. Returns non-zero when there is nothing to do and the script should
# stop. Only the release path calls this; see the header for why.
confirm_release_install() {
    [ "$force" -eq 0 ] || return 0

    current="$(installed_version)"
    [ -n "$current" ] || return 0

    if [ "$version" = "latest" ]; then
        step "Looking up the latest release"
        candidate="$(latest_tag)"
    else
        candidate="$version"
    fi

    if [ -z "$candidate" ]; then
        say "gamestore ${current} is installed in ${destination}, and the latest release could not be looked up."
        interactive || return 0
        ask "Reinstall it?" yes
        return $?
    fi

    case "$(version_cmp "$candidate" "$current")" in
        newer)
            say "gamestore ${current} is installed in ${destination}, and ${candidate} is out."
            interactive || return 0
            ask "Update it to ${candidate}?" yes
            return $?
            ;;
        older)
            say "gamestore ${current} is installed in ${destination}, which is newer than ${candidate}."
            interactive || return 0
            ask "Downgrade it to ${candidate}?" no
            return $?
            ;;
        *)
            say "gamestore ${current} is installed in ${destination}, and it is the latest release."
            if ! interactive; then
                say "Nothing to do. Pass --force to reinstall it anyway."
                return 1
            fi
            ask "Reinstall it?" no
            return $?
            ;;
    esac
}

require_cargo() {
    command -v cargo >/dev/null 2>&1 && return 0

    say "cargo is not on PATH, and gamestore has to be built from source here."
    say ""
    if command -v pacman >/dev/null 2>&1; then
        # CachyOS, SteamOS and everything else Arch-shaped.
        say "  sudo pacman -S --needed rustup && rustup default stable"
    else
        say "  https://rustup.rs"
    fi
    return 1
}

# Build from the checkout in $1 and install what comes out.
build_and_install() {
    require_cargo || exit 1

    step "Building gamestore (${profile})"
    if [ "$profile" = "release" ]; then
        ( cd "$1" && cargo build --release --locked -p gamestore-cli )
    else
        ( cd "$1" && cargo build --locked -p gamestore-cli )
    fi

    step "Installing to ${destination}"
    install -Dm755 "$1/target/${profile}/gamestore" "$destination"
}

# Clone the repository into a temporary directory and build that.
clone_and_install() {
    command -v git >/dev/null 2>&1 || fail "git is needed to build from source, and is not installed"
    require_cargo || exit 1

    work="$(mktemp -d)"
    step "Cloning ${repository}"
    if [ "$version" = "latest" ]; then
        git clone --depth 1 "https://github.com/${repository}.git" "${work}/src"
    else
        git clone --depth 1 --branch "$version" "https://github.com/${repository}.git" "${work}/src"
    fi

    build_and_install "${work}/src"
}

# Download the released binary. Returns non-zero when there is nothing to
# download, so the caller can fall back to source without the script dying.
download_and_install() {
    asset="gamestore-${target}.tar.gz"
    if [ "$version" = "latest" ]; then
        base="https://github.com/${repository}/releases/latest/download"
    else
        base="https://github.com/${repository}/releases/download/${version}"
    fi

    work="$(mktemp -d)"
    step "Downloading ${asset} (${version})"
    download "${base}/${asset}" "${work}/${asset}" || return 1

    # The checksums are published next to the archive. A release without them is
    # not a reason to refuse, but a mismatch is.
    if download "${base}/SHA256SUMS" "${work}/SHA256SUMS" 2>/dev/null; then
        if command -v sha256sum >/dev/null 2>&1; then
            step "Checking sha256"
            ( cd "$work" && grep " ${asset}\$" SHA256SUMS | sha256sum -c - ) \
                || fail "the downloaded archive does not match its published checksum"
        fi
    else
        say "No SHA256SUMS published for this release; the archive was not verified."
    fi

    tar -xzf "${work}/${asset}" -C "$work" || return 1
    [ -f "${work}/gamestore" ] || return 1

    # A glibc-linked binary built on a newer distribution will not start on an
    # older one, and the failure is an unreadable loader error. Better to find
    # out here, where there is still source to fall back to.
    chmod +x "${work}/gamestore"
    "${work}/gamestore" --version >/dev/null 2>&1 || {
        say "The released binary does not run on this system; building from source instead."
        return 1
    }

    step "Installing to ${destination}"
    install -Dm755 "${work}/gamestore" "$destination"
}

if [ "$in_checkout" -eq 1 ] && [ "$version" = "latest" ]; then
    build_and_install "$script_dir"
elif [ "$from_source" -eq 1 ]; then
    clone_and_install
else
    confirm_release_install || exit 0

    if ! download_and_install; then
        say ""
        say "No released binary for ${target}; building from source."
        say ""
        rm -rf "$work"
        work=""
        clone_and_install
    fi
fi

say ""
"$destination" info || true
say ""

case ":${PATH}:" in
    *":${prefix}/bin:"*) ;;
    *)
        say "Note: ${prefix}/bin is not on your PATH. Add it, for this shell and the next:"
        say ""
        say "  echo 'export PATH=\"${prefix}/bin:\$PATH\"' >> ~/.bashrc"
        say "  export PATH=\"${prefix}/bin:\$PATH\""
        say ""
        ;;
esac

cat <<'NEXT'
Next:

  gamestore login           log in to GOG (needs client credentials: docs/gog-login.md)
  gamestore steam-install   add gamestore to Steam as a tile, then restart Steam

NEXT
