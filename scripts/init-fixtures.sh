#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/fixtures/disk"
mkdir -p \
  "$root/radarr/movie-a" \
  "$root/sonarr/cross-a" \
  "$root/sonarr/episode-b" \
  "$root/autobrr/pack-c" \
  "$root/library"

movie="$root/radarr/movie-a/movie.mkv"
if [[ ! -e "$movie" ]]; then
  dd if=/dev/zero of="$movie" bs=1M count=64 status=none
fi
# Same inode in two managed torrent save paths: this is a real cross-seed fixture.
ln -f "$movie" "$root/sonarr/cross-a/movie.mkv"

episode="$root/sonarr/episode-b/episode.mkv"
if [[ ! -e "$episode" ]]; then
  dd if=/dev/zero of="$episode" bs=1M count=32 status=none
fi
# A third hardlink outside the managed torrent paths makes the data non-reclaimable.
ln -f "$episode" "$root/library/episode.mkv"

pack="$root/autobrr/pack-c/sample.bin"
if [[ ! -e "$pack" ]]; then
  dd if=/dev/zero of="$pack" bs=1M count=16 status=none
fi

printf 'Fixture filesystem ready: %s\n' "$root"
printf 'Hardlink checks:\n'
stat -c '  %n inode=%i links=%h size=%s' \
  "$movie" \
  "$root/sonarr/cross-a/movie.mkv" \
  "$episode" \
  "$root/library/episode.mkv"
