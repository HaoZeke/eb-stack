#!/usr/bin/env bash
# Collect the easystack changes of every open pull request as data for
# `eb-stack pr-overlap`. Read-only: it lists pull requests, fetches their head
# commits as git objects and reads blobs out of them. Nothing from a pull
# request is checked out or executed.
#
# Usage: fetch-prs.sh <owner/repo> <eessi-version> <out-dir> [pr-numbers]
# pr-numbers is an optional file of pull request numbers, one per line (or
# comma-separated). Without it, every open pull request is read. Closed
# numbers still resolve: GitHub keeps refs/pull/N/head.
# Needs: gh (authenticated with any read token), git, python3, curl.
# Run inside a clone of the repository whose `origin` is <owner/repo>.
set -euo pipefail

repo=$1
version=$2
out=${3:?usage: fetch-prs.sh <owner/repo> <eessi-version> <out-dir> [pr-numbers]}
list=${4:-}
# Rewrite raw/ and prs/ from scratch on every run, so a pull request that
# closed or stopped touching easystacks does not persist from a stale run,
# and so does its refspecs line. commit-trees/ and easyblocks/ are caches
# guarded by their marker files and survive reruns.
rm -rf "$out/raw" "$out/prs"
mkdir -p "$out/prs" "$out/commit-trees" "$out/easyblocks"
rm -f "$out/refspecs"
prefix="easystacks/software.eessi.io/${version}/"

git fetch --quiet --no-tags origin main
if [[ -n "$list" ]]; then
  python3 - "$list" <<'PY' >"$out/numbers.txt"
import sys
text = open(sys.argv[1]).read().replace(",", "\n")
for line in text.splitlines():
    line = line.strip()
    if line:
        print(line)
PY
  : >"$out/listed.tsv"
  while read -r number; do
    gh api --method GET "repos/${repo}/pulls/${number}" --jq '[.number, .head.sha, .title] | @tsv'
  done <"$out/numbers.txt" >>"$out/listed.tsv"
  python3 - "$out/listed.tsv" "$out/open.json" <<'PY'
import json, sys
rows = []
for line in open(sys.argv[1]):
    number, sha, title = line.rstrip("\n").split("\t", 2)
    rows.append({"number": int(number), "headRefOid": sha, "title": title})
json.dump(rows, open(sys.argv[2], "w"))
PY
else
  # Every open pull request, 100 per page, with no upper limit.
  gh api --method GET --paginate "repos/${repo}/pulls?state=open&per_page=100" \
    --jq '.[] | {number, headRefOid: .head.sha, title}' >"$out/open.jsonl"
  python3 - "$out/open.jsonl" "$out/open.json" <<'PY'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
json.dump(rows, open(sys.argv[2], "w"))
print("open pull requests:", len(rows))
PY
fi

python3 - "$out/open.json" <<'PY' | while read -r number sha; do
import json, sys
for pr in json.load(open(sys.argv[1])):
    print(pr["number"], pr["headRefOid"])
PY
  # Changed files, from the API so no diff of fork history is needed.
  mapfile -t files < <(gh api --method GET --paginate "repos/${repo}/pulls/${number}/files" \
    --jq '.[] | .filename' | while IFS= read -r name; do
      case "$name" in
        *.eb) printf '%s\n' "$name" ;;
        "${prefix}"*.yml) printf '%s\n' "$name" ;;
      esac
    done)
  [[ ${#files[@]} -eq 0 ]] && continue
  dir="$out/raw/${number}"
  mkdir -p "$dir"
  i=0
  for f in "${files[@]}"; do
    printf '%s\n' "$f" >"$dir/${i}.path"
    i=$((i + 1))
  done
  printf '%s\n' "$sha" >"$dir/head_sha"
  # GitHub keeps every pull request head, fork or not, at refs/pull/N/head.
  printf '+refs/pull/%s/head:refs/remotes/pr/%s\n' "$number" "$number" >>"$out/refspecs"
done

# One shallow fetch for every head. A shallow fetch per pull request
# made a first run stop with git exit 128 ("shallow file has changed
# since we read it"). The single fetch runs with automatic gc off, since
# a gc can rewrite .git/shallow, and a failed fetch is retried twice.
if [[ -s "$out/refspecs" ]]; then
  for attempt in 1 2 3; do
    if git -c gc.auto=0 -c maintenance.auto=false fetch --quiet --no-tags \
      --depth=1 --stdin origin <"$out/refspecs"; then
      break
    fi
    if [[ $attempt -eq 3 ]]; then
      echo "fetch-prs.sh: git fetch failed 3 times" >&2
      exit 1
    fi
    sleep $((attempt * 5))
  done
fi

# Base text and blob from main, head text and blob from the fetched ref.
for dir in "$out"/raw/*/; do
  [[ -d $dir ]] || continue
  dir=${dir%/}
  number=${dir##*/}
  i=0
  while [[ -f "$dir/${i}.path" ]]; do
    f=$(<"$dir/${i}.path")
    git show "origin/main:${f}" >"$dir/${i}.base" 2>/dev/null || rm -f "$dir/${i}.base"
    git show "refs/remotes/pr/${number}:${f}" >"$dir/${i}.head" 2>/dev/null || rm -f "$dir/${i}.head"
    git rev-parse "origin/main:${f}" >"$dir/${i}.base_blob" 2>/dev/null || rm -f "$dir/${i}.base_blob"
    git rev-parse "refs/remotes/pr/${number}:${f}" >"$dir/${i}.head_blob" 2>/dev/null || rm -f "$dir/${i}.head_blob"
    i=$((i + 1))
  done
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
    stacks, extras = [], []
    i = 0
    while os.path.exists(f"{d}/{i}.path"):
        path = read(f"{d}/{i}.path").strip()
        entry = {"path": path, "base": read(f"{d}/{i}.base"), "head": read(f"{d}/{i}.head")}
        base_blob, head_blob = read(f"{d}/{i}.base_blob"), read(f"{d}/{i}.head_blob")
        if base_blob:
            entry["base_blob"] = base_blob.strip()
        if head_blob:
            entry["head_blob"] = head_blob.strip()
        if path.endswith(".eb"):
            entry["kind"] = "easyconfig"
            extras.append(entry)
        else:
            stacks.append(entry)
        i += 1
    doc = {"number": int(number), "head_sha": read(f"{d}/head_sha").strip(),
           "title": titles.get(int(number)), "easystacks": stacks, "files": extras}
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
# No head easystack naming a pin is a normal run: an empty glob or a grep that
# matches nothing exits 1, which must not abort the script. The subshell
# absorbs only that, so a real failure in the loop body below still stops us.
(
  compgen -G "$out/raw/*/*.head" >/dev/null || exit 0
  cat "$out"/raw/*/*.head \
    | grep -oE '(^|[^A-Za-z0-9_-])from-commit:[[:space:]]*[0-9a-f]{40}' \
    | grep -oE '[0-9a-f]{40}' | sort -u || [[ $? -eq 1 ]]
) \
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

# Easyblocks named by include-easyblocks-from-commit. EasyBuild includes only
# the files that commit touches, and it puts them ahead of the installed
# easyblocks package. The whole repository at that commit is not the same
# thing: every class would then look as if the commit had provided it.
(
  compgen -G "$out/raw/*/*.head" >/dev/null || exit 0
  cat "$out"/raw/*/*.head \
    | grep -oE 'include-easyblocks-from-commit:[[:space:]]*[0-9a-f]{40}' \
    | grep -oE '[0-9a-f]{40}' | sort -u || [[ $? -eq 1 ]]
) \
  | while read -r commit; do
      dest="$out/easyblocks/${commit}"
      [[ -f "$dest/.touched-files" ]] && continue
      rm -rf "$dest"
      mkdir -p "$dest"
      if ! touched=$(gh api --method GET "repos/easybuilders/easybuild-easyblocks/commits/${commit}" \
          --jq '.files[] | select(.status != "removed") | .filename'); then
        echo "no easyblocks commit ${commit}" >&2
        rm -rf "$dest"
        continue
      fi
      if [[ -z "$touched" ]]; then
        echo "easyblocks commit ${commit} touches no files" >&2
        rm -rf "$dest"
        continue
      fi
      ball=$(mktemp)
      if ! curl -fsSL "https://codeload.github.com/easybuilders/easybuild-easyblocks/tar.gz/${commit}" -o "$ball"; then
        echo "no easyblocks commit ${commit}" >&2
        rm -f "$ball"
        rm -rf "$dest"
        continue
      fi
      top=$(tar -tzf "$ball" | head -n 1 || true)
      top=${top%%/*}
      if [[ "$top" != easybuild-easyblocks-* ]]; then
        echo "no easyblocks commit ${commit}" >&2
        rm -f "$ball"
        rm -rf "$dest"
        continue
      fi
      args=()
      while IFS= read -r file; do
        [[ -n "$file" ]] && args+=("${top}/${file}")
      done <<<"$touched"
      if ! tar -xzf "$ball" -C "$dest" --strip-components=1 -- "${args[@]}"; then
        echo "could not extract easyblocks commit ${commit}" >&2
        rm -f "$ball"
        rm -rf "$dest"
        continue
      fi
      rm -f "$ball"
      : >"$dest/.touched-files"
    done
shopt -s nullglob
prs=("$out"/prs/*.json)
shopt -u nullglob
echo "pull requests with ${version} easystack changes: ${#prs[@]}"
