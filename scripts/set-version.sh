#!/bin/sh
#
# Stamp the `[package] version` in Cargo.toml from a release tag.
#
# The release workflow calls this before building so a binary built from a tag
# reports that tag's version when run as `comic-book --version`. The tag name is
# authoritative: the manifest is overwritten rather than trusted to have been
# bumped by hand. Only the `version` line inside the `[package]` table is
# touched; dependency versions are never modified. Cargo.lock is refreshed
# separately by the workflow (`cargo update -p comic-book`) so the `--locked`
# release build still succeeds.
#
# Usage:
#   scripts/set-version.sh v0.2.4          # the leading "v" is optional
#   scripts/set-version.sh 0.2.0-rc.1
#   scripts/set-version.sh --check v0.2.4  # validate the tag, change nothing
#
set -eu

# Edit the manifest belonging to this script's checkout, not the caller's cwd.
repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
manifest="$repo_root/Cargo.toml"

check=0
if [ "${1:-}" = "--check" ]; then
	check=1
	shift
fi

version="${1:-}"
if [ -z "$version" ]; then
	printf 'error: usage: %s [--check] <version>\n' "$0" >&2
	exit 2
fi

# Release tags are `v`-prefixed; Cargo's own version field is not.
version="${version#v}"

# Cargo requires a full SemVer version such as `0.2.4` or `0.2.0-rc.1`.
if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'; then
	printf 'error: "%s" is not a valid SemVer version\n' "$version" >&2
	exit 2
fi

if [ "$check" = 1 ]; then
	printf 'valid release version: %s\n' "$version"
	exit 0
fi

# Rewrite only the first `version =` line in the `[package]` table, and fail if
# there is none, so a malformed manifest is never silently left stale.
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

if ! awk -v v="$version" '
	/^\[/ { in_package = ($0 ~ /^\[[[:space:]]*package[[:space:]]*\]/) }
	!done && in_package && /^version[[:space:]]*=/ {
		sub(/=.*/, "= \"" v "\"")
		done = 1
	}
	{ print }
	END { exit(done ? 0 : 1) }
' "$manifest" >"$tmp"; then
	printf 'error: no [package] version field found in %s\n' "$manifest" >&2
	exit 1
fi

mv "$tmp" "$manifest"
printf 'set %s version to %s\n' "$manifest" "$version"
