#!/bin/sh
#
# Print the release notes for a version, drawn from CHANGELOG.md.
#
# The notes cover the whole major.minor line of the version: its own changelog
# section — the `## [Unreleased]` heading at the top, retitled to the version and
# dated — followed by every older release that shares the same major and minor
# numbers, pre-releases included. A `0.2.6` release therefore carries the 0.2.5,
# 0.2.4 … 0.2.0-rc.1 entries as well as its own; a `0.3.0` release starts a fresh
# line and carries only itself.
#
# Headings are rewritten for a release page: the link-reference brackets are
# dropped (`## [0.2.5] - 2026-09-26` becomes `## 0.2.5 - 2026-09-26`), and the
# `[Unreleased]` heading becomes `## <version> - <YYYY-MM-DD>`. The `<!-- Links -->`
# appendix is not emitted. A changelog with nothing for the version prints
# nothing, which lets the release workflow fall back to GitHub's generated notes.
#
# Usage:
#   scripts/changelog-notes.sh v0.2.6          # the leading "v" is optional
#   scripts/changelog-notes.sh 0.2.6 > notes.md
#
set -eu

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
changelog="$repo_root/CHANGELOG.md"

version="${1:-}"
if [ -z "$version" ]; then
	printf 'error: usage: %s <version>\n' "$0" >&2
	exit 2
fi

# Release tags are `v`-prefixed; the changelog headings are not.
version="${version#v}"

if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+'; then
	printf 'error: "%s" is not a version\n' "$version" >&2
	exit 2
fi

# The line a release belongs to: its major and minor numbers.
major="${version%%.*}"
rest="${version#*.}"
minor="${rest%%.*}"
prefix="$major.$minor."

if [ ! -f "$changelog" ]; then
	printf 'error: %s not found\n' "$changelog" >&2
	exit 1
fi

today="$(date +%Y-%m-%d)"

awk -v version="$version" -v prefix="$prefix" -v today="$today" '
	# Everything before the first release section (the title and intro) is skipped.
	BEGIN { skip = 1 }

	# The link-reference appendix is not part of any release section.
	/^<!-- Links -->/ { exit }
	/^\[[^]]+\]:/ { next }

	/^## \[/ {
		label = $0
		sub(/^## \[/, "", label)
		sub(/\].*/, "", label)
		rest = $0
		sub(/^## \[[^]]*\]/, "", rest)

		if (label == "Unreleased") {
			print "## " version " - " today
			skip = 0
		} else if (index(label, prefix) == 1) {
			# `rest` already carries the original ` - <date>` suffix, if any.
			print "## " label rest
			skip = 0
		} else {
			skip = 1
		}
		next
	}

	{ if (!skip) print }
' "$changelog"
