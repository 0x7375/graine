#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 /path/to/old/graine/working-directory" >&2
  exit 2
fi

source_dir="$(realpath "$1")"
target_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/data/legacy"
mkdir -p "$target_dir"

copied=0
for name in analytics.json activity.jsonl history.jsonl deletions.jsonl; do
  if [[ -f "$source_dir/$name" ]]; then
    cp -p -- "$source_dir/$name" "$target_dir/$name"
    printf 'Copied %s\n' "$name"
    copied=$((copied + 1))
  fi
done

if [[ $copied -eq 0 ]]; then
  echo "No Graine state files found in $source_dir" >&2
  exit 1
fi

echo "Legacy state copied to $target_dir"
echo "The SQLite import happens on the next Graine startup. Original source files were not modified."
