#!/bin/sh
#
# comic-book installer for macOS and Linux.
#
# Downloads a prebuilt binary from GitHub Releases and installs it into a
# directory on your PATH (default: ~/.local/bin, or /usr/local/bin as root).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/jjangsangy/ComicBook/main/scripts/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version v0.1.0 --install-dir "$HOME/bin"
#   ./scripts/install.sh --help
#
set -eu

REPO="jjangsangy/ComicBook"
BIN="comic-book"

version="${COMIC_BOOK_VERSION:-latest}"
target="${COMIC_BOOK_TARGET:-}"
install_dir="${COMIC_BOOK_INSTALL_DIR:-}"

warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
	cat <<EOF
Install the $BIN CLI from GitHub Releases ($REPO).

Usage: install.sh [OPTIONS]

Options:
  -v, --version <TAG>      Release tag to install, e.g. v0.1.0 (default: latest)
  -t, --target <TRIPLE>    Force a Rust target triple instead of auto-detecting
  -d, --install-dir <DIR>  Directory to install the binary into
                           (default: ~/.local/bin, or /usr/local/bin as root)
  -h, --help               Show this help

Environment:
  COMIC_BOOK_VERSION       Same as --version
  COMIC_BOOK_TARGET        Same as --target
  COMIC_BOOK_INSTALL_DIR   Same as --install-dir

Examples:
  curl -fsSL https://raw.githubusercontent.com/$REPO/main/scripts/install.sh | sh
  ./install.sh --version v0.1.0
  ./install.sh --install-dir "\$HOME/bin"
EOF
}

while [ $# -gt 0 ]; do
	case "$1" in
	-v | --version)
		[ $# -ge 2 ] || die "--version requires a value"
		version="$2"
		shift 2
		;;
	--version=*)
		version="${1#*=}"
		shift
		;;
	-t | --target)
		[ $# -ge 2 ] || die "--target requires a value"
		target="$2"
		shift 2
		;;
	--target=*)
		target="${1#*=}"
		shift
		;;
	-d | --install-dir | --dir)
		[ $# -ge 2 ] || die "--install-dir requires a value"
		install_dir="$2"
		shift 2
		;;
	--install-dir=* | --dir=*)
		install_dir="${1#*=}"
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		usage >&2
		die "unknown option: $1"
		;;
	esac
done

# --- Detect the platform ------------------------------------------------------

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
Darwin) os="apple-darwin" ;;
Linux) os="unknown-linux" ;;
*)
	die "unsupported operating system: $os (on Windows, run scripts/install.ps1 instead)"
	;;
esac

case "$arch" in
x86_64 | amd64) cpu="x86_64" ;;
arm64 | aarch64) cpu="aarch64" ;;
*) die "unsupported architecture: $arch" ;;
esac

# Candidate target triples, most preferred first. Linux prefers the statically
# linked musl build so the binary runs on any distribution regardless of its
# glibc version, and falls back to the glibc build if musl is unavailable.
if [ -n "$target" ]; then
	candidates="$target"
elif [ "$os" = "apple-darwin" ]; then
	candidates="$cpu-$os"
else
	candidates="$cpu-$os-musl $cpu-$os-gnu"
fi

# --- Resolve download location ------------------------------------------------

if [ "$version" = "latest" ]; then
	base_url="https://github.com/$REPO/releases/latest/download"
else
	case "$version" in
	v*) tag="$version" ;;
	*) tag="v$version" ;;
	esac
	base_url="https://github.com/$REPO/releases/download/$tag"
fi

# --- Download helpers ---------------------------------------------------------

download() {
	# Non-fatal: returns non-zero on failure so callers can fall back.
	url="$1"
	out="$2"
	if command -v curl >/dev/null 2>&1; then
		curl -fsSL --retry 3 --proto '=https' --tlsv1.2 -o "$out" "$url"
	elif command -v wget >/dev/null 2>&1; then
		wget -q -O "$out" "$url"
	else
		die "either curl or wget is required to download $BIN"
	fi
}

sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{print $1}'
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | awk '{print $1}'
	else
		return 1
	fi
}

verify_checksum() {
	# $1: local path of the downloaded archive
	# $2: remote URL of the published .sha256 checksum
	archive="$1"
	checksum_url="$2"
	if ! download "$checksum_url" "$archive.sha256" 2>/dev/null; then
		warn "no checksum published for $(basename "$archive"); skipping verification"
		return 0
	fi

	expected="$(awk 'NR == 1 {print $1}' "$archive.sha256")"
	actual="$(sha256_of "$archive")" || {
		warn "no sha256 tool found; skipping verification"
		return 0
	}

	[ -n "$expected" ] || die "malformed checksum file for $(basename "$archive")"
	if [ "$actual" != "$expected" ]; then
		die "checksum verification failed for $(basename "$archive")"
	fi
	printf '  checksum verified (%s)\n' "$expected"
}

# --- Fetch --------------------------------------------------------------------

# Honour $TMPDIR explicitly: the OS default temp directory is not writable in
# some sandboxes and containers.
if ! tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/$BIN.XXXXXX")"; then
	die "could not create a temporary directory (set TMPDIR to a writable location)"
fi
trap 'rm -rf "$tmp_dir"' EXIT INT TERM HUP

archive=""
selected=""
# shellcheck disable=SC2086 # word splitting is intentional: candidates is a list
for candidate in $candidates; do
	printf 'Downloading %s (%s, %s)...\n' "$BIN" "$version" "$candidate"
	url="$base_url/$BIN-$candidate.tar.gz"
	if download "$url" "$tmp_dir/$BIN-$candidate.tar.gz"; then
		archive="$tmp_dir/$BIN-$candidate.tar.gz"
		selected="$candidate"
		break
	fi
	printf '  %s is not available for this release, trying the next target\n' "$candidate"
done

if [ -z "$archive" ]; then
	die "no prebuilt binary for $os/$cpu (tried: $candidates). See https://github.com/$REPO/releases"
fi

verify_checksum "$archive" "$base_url/$BIN-$selected.tar.gz.sha256"

mkdir -p "$tmp_dir/unpack"
if ! tar -xzf "$archive" -C "$tmp_dir/unpack"; then
	die "failed to extract $(basename "$archive")"
fi
[ -f "$tmp_dir/unpack/$BIN" ] || die "$(basename "$archive") did not contain the $BIN binary"

# --- Install ------------------------------------------------------------------

if [ -z "$install_dir" ]; then
	if [ "$(id -u)" -eq 0 ]; then
		install_dir="/usr/local/bin"
	else
		install_dir="$HOME/.local/bin"
	fi
fi

mkdir -p "$install_dir" || die "cannot create $install_dir (pass --install-dir or run with sudo)"

if command -v install >/dev/null 2>&1; then
	install -m 0755 "$tmp_dir/unpack/$BIN" "$install_dir/$BIN" || die "cannot write to $install_dir"
else
	cp -f "$tmp_dir/unpack/$BIN" "$install_dir/$BIN" || die "cannot write to $install_dir"
	chmod 0755 "$install_dir/$BIN"
fi

printf '\nInstalled %s to %s\n' "$BIN" "$install_dir/$BIN"
if installed_version="$("$install_dir/$BIN" --version 2>/dev/null)"; then
	printf '%s (%s)\n' "$installed_version" "$selected"
fi

# --- PATH guidance ------------------------------------------------------------

case ":${PATH:-}:" in
*":$install_dir:"*) ;;
*)
	cat <<EOF

$install_dir is not on your PATH yet. Add it to your shell profile:

  # ~/.zshrc (zsh) or ~/.bashrc / ~/.profile (bash, sh)
  export PATH="$install_dir:\$PATH"

Then restart your shell or open a new terminal.

EOF
	;;
esac
