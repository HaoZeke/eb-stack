#!/usr/bin/env bash
# Collect the easyconfig changes of every open pull request as data for
# `eb-stack pr-overlap --kind easyconfigs`. Read-only: it lists pull
# requests, fetches their head commits as git objects and reads blobs
# out of them. Nothing from a pull request is checked out or executed.
#
# Usage: fetch-prs.sh <owner/repo> <out-dir>
# Needs: gh (authenticated with any read token), git, python3.
# Run inside a clone of easybuild-easyconfigs whose `origin` is <owner/repo>.
# The base text of a changed file is read from `develop`.
set -euo pipefail

repo=$1
out=$2
mkdir -p "$out/prs"
prefix="easybuild/easyconfigs/"

git fetch --quiet --no-tags origin develop
gh pr list --repo "$repo" --state open --limit 500 \
  --json number,headRefOid,title >"$out/open.json"

python3 - "$out/open.json" <<'PY' | while read -r number sha; do
import json, sys
for pr in json.load(open(sys.argv[1])):
    print(pr["number"], pr["headRefOid"])
PY
  # Changed files, from the API so no diff of fork history is needed.
  mapfile -t files < <(gh api --paginate "repos/${repo}/pulls/${number}/files" \
    --jq ".[] | select(.filename | startswith(\"${prefix}\")) | select(.filename | endswith(\".eb\")) | .filename")
  [[ ${#files[@]} -eq 0 ]] && continue
  # GitHub keeps every pull request head, fork or not, at refs/pull/N/head.
  git fetch --quiet --no-tags --depth=1 origin "refs/pull/${number}/head:refs/remotes/pr/${number}"
  dir="$out/raw/${number}"
  mkdir -p "$dir"
  i=0
  for f in "${files[@]}"; do
    git show "origin/develop:${f}" >"$dir/${i}.base" 2>/dev/null || rm -f "$dir/${i}.base"
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
    easyconfigs = []
    i = 0
    while os.path.exists(f"{d}/{i}.path"):
        easyconfigs.append({"path": read(f"{d}/{i}.path").strip(),
                            "base": read(f"{d}/{i}.base"), "head": read(f"{d}/{i}.head")})
        i += 1
    doc = {"number": int(number), "head_sha": read(f"{d}/head_sha").strip(),
           "title": titles.get(int(number)), "easyconfigs": easyconfigs}
    json.dump(doc, open(f"{out}/prs/{number}.json", "w"), indent=1, sort_keys=True)
PY
ls "$out/prs" | wc -l | xargs echo "pull requests with easyconfig changes:"
