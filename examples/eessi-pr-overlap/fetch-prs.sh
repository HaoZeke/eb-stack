#!/usr/bin/env bash
# Collect the easystack changes of every open pull request as data for
# `eb-stack pr-overlap`. Read-only: it lists pull requests, fetches their head
# commits as git objects and reads blobs out of them. Nothing from a pull
# request is checked out or executed.
#
# Usage: fetch-prs.sh <owner/repo> <eessi-version> <out-dir>
# Needs: gh (authenticated with any read token), git, python3, curl.
# Run inside a clone of the repository whose `origin` is <owner/repo>.
set -euo pipefail

repo=$1
version=$2
out=$3
mkdir -p "$out/prs" "$out/commit-trees"
prefix="easystacks/software.eessi.io/${version}/"

git fetch --quiet --no-tags origin main
gh pr list --repo "$repo" --state open --limit 500 \
  --json number,headRefOid,title >"$out/open.json"

python3 - "$out/open.json" <<'PY' | while read -r number sha; do
import json, sys
for pr in json.load(open(sys.argv[1])):
    print(pr["number"], pr["headRefOid"])
PY
  # Changed files, from the API so no diff of fork history is needed.
  mapfile -t files < <(gh api --paginate "repos/${repo}/pulls/${number}/files" \
    --jq ".[] | select(.filename | startswith(\"${prefix}\")) | select(.filename | endswith(\".yml\")) | .filename")
  [[ ${#files[@]} -eq 0 ]] && continue
  # GitHub keeps every pull request head, fork or not, at refs/pull/N/head.
  git fetch --quiet --no-tags --depth=1 origin "refs/pull/${number}/head:refs/remotes/pr/${number}"
  dir="$out/raw/${number}"
  mkdir -p "$dir"
  i=0
  for f in "${files[@]}"; do
    git show "origin/main:${f}" >"$dir/${i}.base" 2>/dev/null || rm -f "$dir/${i}.base"
    git show "refs/remotes/pr/${number}:${f}" >"$dir/${i}.head" 2>/dev/null || rm -f "$dir/${i}.head"
    printf '%s\n' "$f" >"$dir/${i}.path"
    i=$((i + 1))
  done
  printf '%s\n' "$sha" >"$dir/head_sha"
done

# One JSON document per pull request, in the shape `pr-overlap --pr` reads.
python3 - "$out" <<'PY'
import json, os, sys
out = sys.argv[1]
titles = {p["number"]: p["title"] for p in json.load(open(f"{out}/open.json"))}
raw = f"{out}/raw"
for number in sorted(os.listdir(raw) if os.path.isdir(raw) else [], key=int):
    d = f"{raw}/{number}"
    def read(p):
        return open(p).read() if os.path.exists(p) else None
    stacks = []
    i = 0
    while os.path.exists(f"{d}/{i}.path"):
        stacks.append({"path": read(f"{d}/{i}.path").strip(),
                       "base": read(f"{d}/{i}.base"), "head": read(f"{d}/{i}.head")})
        i += 1
    doc = {"number": int(number), "head_sha": read(f"{d}/head_sha").strip(),
           "title": titles.get(int(number)), "easystacks": stacks}
    json.dump(doc, open(f"{out}/prs/{number}.json", "w"), indent=1, sort_keys=True)
PY

# Easyconfigs for every from-commit pin a head easystack names.
# `include-easyblocks-from-commit` ends in `from-commit`, so the option name
# has to be matched on its own. Those commits live in easybuild-easyblocks and
# are not part of the easyconfig robot path.
#
# EasyBuild's from-commit robot path is the whole easyconfigs tree at that
# commit: fetch_files_from_commit downloads the repository tarball and symlinks
# every subdirectory of easybuild/easyconfigs, then copies the touched files
# on top. A tree of only the changed files drops dependencies that already
# existed at the commit and are not in the release robot yet.
cat "$out"/raw/*/*.head 2>/dev/null \
  | grep -oE '(^|[^A-Za-z0-9_-])from-commit:[[:space:]]*[0-9a-f]{40}' \
  | grep -oE '[0-9a-f]{40}' | sort -u \
  | while read -r commit; do
      dest="$out/commit-trees/${commit}"
      [[ -f "$dest/.full-easyconfigs" ]] && continue
      rm -rf "$dest"
      mkdir -p "$dest"
      # A pin that is not an easybuild-easyconfigs commit is recorded by
      # pr-overlap as unresolved. It must not abort the rest of the fetch.
      ball=$(mktemp)
      if ! curl -fsSL "https://codeload.github.com/easybuilders/easybuild-easyconfigs/tar.gz/${commit}" -o "$ball"; then
        echo "no easyconfigs commit ${commit}" >&2
        rm -f "$ball"
        continue
      fi
      # `head` closes the pipe early; under `pipefail` that must not look like a bad commit.
      first=$(tar -tzf "$ball" | head -n 1 || true)
      if [[ "$first" != easybuild-easyconfigs-* ]]; then
        echo "no easyconfigs commit ${commit}" >&2
        rm -f "$ball"
        rm -rf "$dest"
        continue
      fi
      tar -xzf "$ball" -C "$dest" --strip-components=1 --wildcards '*/easybuild/easyconfigs/*'
      rm -f "$ball"
      if [[ ! -d "$dest/easybuild/easyconfigs" ]]; then
        echo "no easyconfig files for from-commit ${commit}" >&2
        rm -rf "$dest"
        continue
      fi
      : >"$dest/.full-easyconfigs"
    done
ls "$out/prs" | wc -l | xargs echo "pull requests with ${version} easystack changes:"
