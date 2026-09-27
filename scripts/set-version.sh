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
# `--changelog` additionally rolls CHANGELOG.md's top `## [Unreleased]` section
# over to the released version: the heading is dated and retitled
# `## [<version>] - <YYYY-MM-DD>`, and the link references at the foot of the
# file are updated (`[Unreleased]` now compares from the new tag, and a
# `[<version>]` entry is added comparing from the previous release — the first
# dated heading below `Unreleased`). The flag is deliberately opt-in: only the
# `bump-version` job, which commits the released version back to the default
# branch, passes it. The per-target build jobs stamp a throwaway checkout and
# must leave the changelog alone.
#
# Usage:
#   scripts/set-version.sh v0.2.4               # the leading "v" is optional
#   scripts/set-version.sh 0.2.0-rc.1
#   scripts/set-version.sh --check v0.2.4       # validate the tag, change nothing
#   scripts/set-version.sh --changelog v0.2.4   # also roll CHANGELOG.md over
#
set -eu

# Edit the files belonging to this script's checkout, not the caller's cwd.
repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
manifest="$repo_root/Cargo.toml"
changelog="$repo_root/CHANGELOG.md"

check=0
update_changelog=0
while [ $# -gt 0 ]; do
	case "$1" in
	--check)
		check=1
		shift
		;;
	--changelog)
		update_changelog=1
		shift
		;;
	--check=* | --changelog=*)
		printf 'error: %s takes no value\n' "${1%%=*}" >&2
		exit 2
		;;
	--)
		shift
		break
		;;
	-*)
		printf 'error: unknown option: %s\n' "$1" >&2
		exit 2
		;;
	*)
		break
		;;
	esac
done

version="${1:-}"
if [ -z "$version" ]; then
	printf 'error: usage: %s [--check] [--changelog] <version>\n' "$0" >&2
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

# Roll the changelog's unreleased section over to this version, when asked.
if [ "$update_changelog" != 1 ]; then
	exit 0
fi

if [ ! -f "$changelog" ] || ! grep -Eq '^## \[Unreleased\]' "$changelog"; then
	printf 'notice: no ## [Unreleased] section in %s; leaving it unchanged\n' "$changelog" >&2
	exit 0
fi

# The previous release is the first dated heading below `## [Unreleased]`; its
# tag is the base of the new version's compare link. `v0.0.0` is a harmless
# placeholder for a changelog whose first release is still unreleased.
prev_tag="$(awk '
	/^## \[Unreleased\]/ { seen = 1; next }
	seen && /^## \[/ {
		line = $0
		sub(/^## \[/, "", line)
		sub(/\].*/, "", line)
		print "v" line
		exit
	}
' "$changelog")"
[ -n "$prev_tag" ] || prev_tag="v0.0.0"

# Reuse the repository URL the existing `[Unreleased]` link already points at so
# a fork's changelog is not rewritten to upstream.
base_url="$(awk '/^\[Unreleased\]:/ { line = $0; sub(/^\[Unreleased\]:[[:space:]]*/, "", line); sub(/\/compare\/.*/, "", line); print line; exit }' "$changelog")"
[ -n "$base_url" ] || base_url="https://github.com/jjangsangy/ComicBook"

today="$(date +%Y-%m-%d)"

changelog_tmp="$(mktemp)"
trap 'rm -f "$changelog_tmp"' EXIT

awk -v v="$version" -v date="$today" -v prev="$prev_tag" -v base="$base_url" '
	/^## \[Unreleased\]/ && !heading {
		print "## [" v "] - " date
		heading = 1
		next
	}
	/^\[Unreleased\]:/ && !link {
		print "[Unreleased]: " base "/compare/v" v "...HEAD"
		print "[" v "]: " base "/compare/" prev "...v" v
		link = 1
		next
	}
	{ print }
	END {
		# A changelog with the heading but no link reference still gets the
		# links, appended after whatever references it already has.
		if (heading && !link) {
			print "[Unreleased]: " base "/compare/v" v "...HEAD"
			print "[" v "]: " base "/compare/" prev "...v" v
		}
	}
' "$changelog" >"$changelog_tmp"

mv "$changelog_tmp" "$changelog"
printf 'released %s in %s (previous %s)\n' "$version" "$changelog" "$prev_tag"
