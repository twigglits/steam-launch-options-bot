#!/usr/bin/env bash
# Compute the next release version from conventional commits.
# (Adapted from the Sigo project's release pipeline.)
#
# Usage: scripts/next-version.sh [auto|patch|minor|major]   (default: auto)
# Prints the bare next version (e.g. "0.2.0") on stdout; rationale on stderr.
#
# Auto rules over commits since the last v* tag (semantic-release convention):
#   - breaking change ("type!:" subject or "BREAKING CHANGE" in body) -> major,
#     EXCEPT while still on 0.x, where breaking bumps minor; 1.0.0 is only
#     ever cut deliberately via the "major" override.
#   - any feat -> minor
#   - any fix, perf, or revert -> patch
#   - ONLY docs/chore/ci/refactor/test/style/build commits -> exit 3,
#     "no release needed" (an explicit patch/minor/major override still forces).
#
# Exit codes: 0 = version printed; 1 = guard failure (version/tag drift);
#             2 = bad usage; 3 = nothing release-worthy.
set -euo pipefail

bump="${1:-auto}"
[ -n "$bump" ] || bump="auto"
case "$bump" in
  auto|patch|minor|major) ;;
  *) echo "usage: $0 [auto|patch|minor|major]" >&2; exit 2 ;;
esac

# Cargo.toml's [package] table carries `version` before any dependency table,
# so the first match is the package's own.
current=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
[ -n "$current" ] || { echo "error: no version in Cargo.toml" >&2; exit 1; }

last_tag=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)
if [ -n "$last_tag" ]; then
  if [ "${last_tag#v}" != "$current" ]; then
    echo "error: Cargo.toml version ($current) != last tag ($last_tag); fix the drift before releasing" >&2
    exit 1
  fi
  range="$last_tag..HEAD"
else
  range="HEAD"
fi

count=$(git rev-list --count "$range")
if [ "$count" -eq 0 ]; then
  echo "no release needed: no commits since $last_tag" >&2
  exit 3
fi

IFS=. read -r major minor patch <<<"$current"

if [ "$bump" = "auto" ]; then
  # Capture once and grep herestrings rather than piping `git log | grep -q`:
  # under pipefail, grep -q exiting at the first match can SIGPIPE git (exit
  # 141), making a MATCHED condition read as false on CI runners.
  subjects=$(git log --format=%s "$range")
  bodies=$(git log --format=%B "$range")
  if grep -Eq '^[a-z]+(\([^)]*\))?!:' <<<"$subjects" ||
     grep -Eq '^BREAKING[ -]CHANGE:' <<<"$bodies"; then
    bump="major"
  elif grep -Eq '^feat(\([^)]*\))?:' <<<"$subjects"; then
    bump="minor"
  elif grep -Eq '^(fix|perf|revert)(\([^)]*\))?:' <<<"$subjects"; then
    bump="patch"
  else
    echo "no release needed: none of the $count commit(s) since ${last_tag:-the beginning} are feat/fix/perf/breaking" >&2
    exit 3
  fi
  if [ "$bump" = "major" ] && [ "$major" -eq 0 ]; then
    echo "note: breaking change on 0.x bumps minor; use an explicit 'major' argument to cut 1.0.0" >&2
    bump="minor"
  fi
  echo "auto: $count commit(s) since ${last_tag:-the beginning} -> $bump bump" >&2
fi

case "$bump" in
  major) next="$((major + 1)).0.0" ;;
  minor) next="$major.$((minor + 1)).0" ;;
  patch) next="$major.$minor.$((patch + 1))" ;;
esac

echo "$next"
