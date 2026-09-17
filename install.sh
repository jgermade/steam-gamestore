#!/usr/bin/env bash
# Build gamestore and install it where both the shell and Steam can find it.
#
# Nothing here touches the system: the binary goes under a prefix in the home
# directory, which is what makes this work unchanged on an immutable SteamOS as
# well as on an ordinary Arch or CachyOS install.
set -euo pipefail

prefix="${HOME}/.local"
profile="release"

usage() {
    cat <<'USAGE'
Usage: ./install.sh [--prefix DIR] [--debug]

  --prefix DIR   Install into DIR/bin (default: ~/.local)
  --debug        Build without optimizations, for a faster edit-run loop
  -h, --help     This message
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) prefix="${2:?--prefix needs a directory}"; shift 2 ;;
        --debug) profile="debug"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "install.sh: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if ! command -v cargo >/dev/null 2>&1; then
    echo "install.sh: cargo is not on PATH, and gamestore is built from source." >&2
    echo >&2
    if command -v pacman >/dev/null 2>&1; then
        # CachyOS, SteamOS and everything else Arch-shaped.
        echo "  sudo pacman -S --needed rustup && rustup default stable" >&2
    else
        echo "  https://rustup.rs" >&2
    fi
    exit 1
fi

root="$(cd "$(dirname "$0")" && pwd)"
cd "$root"

echo "==> Building gamestore (${profile})"
if [ "$profile" = "release" ]; then
    cargo build --release --locked -p gamestore-cli
else
    cargo build --locked -p gamestore-cli
fi

built="${root}/target/${profile}/gamestore"
destination="${prefix}/bin/gamestore"

echo "==> Installing to ${destination}"
install -Dm755 "$built" "$destination"

echo
"$destination" info || true
echo

case ":${PATH}:" in
    *":${prefix}/bin:"*) ;;
    *)
        echo "Note: ${prefix}/bin is not on your PATH. Add it, for this shell and the next:"
        echo
        echo "  echo 'export PATH=\"${prefix}/bin:\$PATH\"' >> ~/.bashrc"
        echo "  export PATH=\"${prefix}/bin:\$PATH\""
        echo
        ;;
esac

cat <<'NEXT'
Next:

  gamestore login           log in to GOG (needs client credentials, see the README)
  gamestore steam-install   add gamestore to Steam as a tile, then restart Steam

NEXT
