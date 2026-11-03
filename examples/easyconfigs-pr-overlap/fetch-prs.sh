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
# GitHub returns at most this many items on one page.
page_size=100
# GitHub lists at most this many changed files for one pull request.
file_cap=3000
# Pages of open pull requests read before the list is taken as unending.
open_page_limit=1000
# Pages of changed files read before the list is taken as unending.
file_page_limit=40

repo=$1
out=${2:?usage: fetch-prs.sh <owner/repo> <out-dir>}
# Rewrite outputs from scratch on every run, so closed pull requests do not
# persist from stale raw/ and prs/ directories.
rm -rf "$out/raw" "$out/prs" "$out/trunc"
mkdir -p "$out/prs" "$out/trunc"
rm -f "$out/refspecs"
prefix="easybuild/easyconfigs/"

git fetch --quiet --no-tags origin develop
# Every open pull request, 100 per page. Each page is requested on its own,
# and a short page is the end. There is no stop at 500.
: >"$out/open.jsonl"
page=1
while true; do
  if ! chunk=$(gh api --method GET \
      "repos/${repo}/pulls?state=open&per_page=${page_size}&page=${page}" \
      --jq '.[] | {number, headRefOid: .head.sha, title}' 2>"$out/.gh-err"); then
    cat "$out/.gh-err" >&2
    echo "fetch-prs.sh: listing open pull requests failed on page ${page}" >&2
    exit 1
  fi
  if [[ -z "${chunk//[$' \t\n']/}" ]]; then
    break
  fi
  printf '%s\n' "$chunk" >>"$out/open.jsonl"
  count=0
  while IFS= read -r line; do
    [[ -n "$line" ]] || continue
    count=$((count + 1))
  done <<<"$chunk"
  if (( count < page_size )); then
    break
  fi
  page=$((page + 1))
  if (( page > open_page_limit )); then
    echo "fetch-prs.sh: open pull request list did not end" >&2
    exit 1
  fi
done
rm -f "$out/.gh-err"
python3 - "$out/open.jsonl" "$out/open.json" <<'PY'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
json.dump(rows, open(sys.argv[2], "w"))
print("open pull requests:", len(rows))
PY

# Matching easyconfig paths for pull request $1, one per line.
# Pages are 100 files. GitHub stops at 3000: a later page answers 422, or
# another page is non-empty after 3000 names. The pull request is kept and
# $out/trunc/$1 is created. Paths past that cap are not kept.
pr_changed_files() {
  local number=$1
  local page=1
  local total=0
  local err="$out/.gh-err"
  while true; do
    if ! chunk=$(gh api --method GET \
        "repos/${repo}/pulls/${number}/files?per_page=${page_size}&page=${page}" \
        --jq '.[] | .filename' 2>"$err"); then
      if (( page > 1 )) && grep -Eqi "${file_cap}|pagination is limited|maximum number of files|too many files" "$err"; then
        : >"$out/trunc/${number}"
        return 0
      fi
      cat "$err" >&2
      echo "fetch-prs.sh: file list for pull request ${number} failed on page ${page}" >&2
      return 1
    fi
    if [[ -z "${chunk//[$' \t\n']/}" ]]; then
      return 0
    fi
    local count=0
    local name
    while IFS= read -r name; do
      [[ -n "$name" ]] || continue
      count=$((count + 1))
      case "$name" in
        "${prefix}"*.eb) printf '%s\n' "$name" ;;
      esac
    done <<<"$chunk"
    total=$((total + count))
    if (( count < page_size )); then
      return 0
    fi
    if (( total >= file_cap )); then
      page=$((page + 1))
      if ! chunk=$(gh api --method GET \
          "repos/${repo}/pulls/${number}/files?per_page=${page_size}&page=${page}" \
          --jq '.[] | .filename' 2>"$err"); then
        if grep -Eqi "${file_cap}|pagination is limited|maximum number of files|too many files" "$err"; then
          : >"$out/trunc/${number}"
          return 0
        fi
        cat "$err" >&2
        echo "fetch-prs.sh: file list for pull request ${number} failed on page ${page}" >&2
        return 1
      fi
      if [[ -n "${chunk//[$' \t\n']/}" ]]; then
        : >"$out/trunc/${number}"
      fi
      return 0
    fi
    page=$((page + 1))
    if (( page > file_page_limit )); then
      echo "fetch-prs.sh: file list for pull request ${number} did not end" >&2
      return 1
    fi
  done
}

python3 - "$out/open.json" <<'PY' | while read -r number sha; do
import json, sys
for pr in json.load(open(sys.argv[1])):
    print(pr["number"], pr["headRefOid"])
PY
  # Changed files, from the API so no diff of fork history is needed.
  dir="$out/raw/${number}"
  mkdir -p "$dir"
  if ! pr_changed_files "$number" >"$dir/names"; then
    exit 1
  fi
  if [[ -f "$out/trunc/${number}" ]]; then
    mv "$out/trunc/${number}" "$dir/truncated"
  fi
  mapfile -t files <"$dir/names"
  if [[ ${#files[@]} -eq 0 && ! -f "$dir/truncated" ]]; then
    rm -rf "$dir"
    continue
  fi
  i=0
  for f in "${files[@]}"; do
    printf '%s\n' "$f" >"$dir/${i}.path"
    i=$((i + 1))
  done
  printf '%s\n' "$sha" >"$dir/head_sha"
  # GitHub keeps every pull request head, fork or not, at refs/pull/N/head.
  if [[ ${#files[@]} -gt 0 ]]; then
    printf '+refs/pull/%s/head:refs/remotes/pr/%s\n' "$number" "$number" >>"$out/refspecs"
  fi
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

# Base text from develop, head text from the fetched pull request ref.
for dir in "$out"/raw/*/; do
  [[ -d $dir ]] || continue
  dir=${dir%/}
  number=${dir##*/}
  i=0
  while [[ -f "$dir/${i}.path" ]]; do
    f=$(<"$dir/${i}.path")
    git show "origin/develop:${f}" >"$dir/${i}.base" 2>/dev/null || rm -f "$dir/${i}.base"
    git show "refs/remotes/pr/${number}:${f}" >"$dir/${i}.head" 2>/dev/null || rm -f "$dir/${i}.head"
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
    easyconfigs = []
    i = 0
    while os.path.exists(f"{d}/{i}.path"):
        easyconfigs.append({"path": read(f"{d}/{i}.path").strip(),
                            "base": read(f"{d}/{i}.base"), "head": read(f"{d}/{i}.head")})
        i += 1
    doc = {"number": int(number), "head_sha": read(f"{d}/head_sha").strip(),
           "title": titles.get(int(number)), "easyconfigs": easyconfigs}
    if os.path.exists(f"{d}/truncated"):
        doc["file_list_truncated"] = True
    json.dump(doc, open(f"{out}/prs/{number}.json", "w"), indent=1, sort_keys=True)
PY
rm -rf "$out/trunc"
rm -f "$out/.gh-err"
shopt -s nullglob
prs=("$out"/prs/*.json)
shopt -u nullglob
echo "pull requests with easyconfig changes: ${#prs[@]}"
