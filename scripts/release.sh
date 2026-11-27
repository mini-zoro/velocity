#!/bin/sh
# usage: scripts/release.sh <x.y.z>
#
# Opens the PR that releases Velocity <x.y.z>: it bumps the workspace version on a
# fresh branch from origin/main and asks GitHub to merge it once the required
# checks pass. After the merge, .github/workflows/auto-release.yml waits for main's
# CI and the GUI end-to-end suite on that commit, then builds, signs and publishes
# the release. Nothing is published if any of those fail.
set -eu

version=${1:-}
case "$version" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "usage: $0 <x.y.z>" >&2; exit 2 ;;
esac
if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "version must be x.y.z, got '$version'" >&2
  exit 2
fi

root=$(git rev-parse --show-toplevel)
cd "$root"
git fetch -q origin main --tags

current=$(git show origin/main:Cargo.toml | sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p')
if [ "$(printf '%s\n%s\n' "$current" "$version" | sort -V | tail -1)" != "$version" ] || [ "$current" = "$version" ]; then
  echo "version $version is not newer than main's $current" >&2
  exit 1
fi
if git ls-remote --exit-code --tags origin "refs/tags/v$version" >/dev/null 2>&1; then
  echo "tag v$version already exists" >&2
  exit 1
fi

branch="release-$version"
work=$(mktemp -d "${TMPDIR:-/tmp}/velocity-release.XXXXXX")
cleanup() {
  git worktree remove --force "$work/tree" >/dev/null 2>&1 || true
  rmdir "$work" 2>/dev/null || true
}
trap cleanup EXIT
git worktree add -q -b "$branch" "$work/tree" origin/main
cd "$work/tree"

# Only the [workspace.package] version; dependency versions stay as they are.
perl -0pi -e 's/(\[workspace\.package\]\n(?:[^\[]*?\n)?version = ")[^"]*(")/${1}'"$version"'${2}/' Cargo.toml
bumped=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
if [ "$bumped" != "$version" ]; then
  echo "could not set the workspace version (still '$bumped')" >&2
  exit 1
fi
# Workspace members inherit the version; refresh their entries in Cargo.lock only.
cargo update --workspace --offline -q
git diff --stat

git commit -q -am "Version $version"
git push -q -u origin "$branch"
gh pr create --head "$branch" --base main --title "Version $version" \
  --body "Releases Velocity $version. After this merges, the Auto release workflow waits for main's CI and the GUI end-to-end suite on the merge commit, then builds, signs and publishes \`v$version\`."
gh pr merge "$branch" --auto --merge
echo "Opened the release PR for $version; it merges when the required checks pass."
