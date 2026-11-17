#!/usr/bin/env python3
"""Sanity-check generated qBittorrent data, actual hardlinks and demo SQLite."""
import json
import sqlite3
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DISK = ROOT / 'fixtures/disk'
with (ROOT / 'fixtures/qbittorrent.generated.json').open() as stream:
    fixture = json.load(stream)

torrents = fixture['torrents']
files = fixture['files']
assert len(torrents) >= 150
assert len({t['hash'] for t in torrents}) == len(torrents)
assert len(files) == len(torrents)

# All completed managed torrents must be clusterable using real file identities.
managed = [t for t in torrents if t['progress'] == 1.0 and t['category'].split('.')[0] in ('radarr', 'sonarr', 'autobrr')]
parent = {t['hash']: t['hash'] for t in managed}

def find(x):
    if parent[x] != x:
        parent[x] = find(parent[x])
    return parent[x]

seen_inodes = {}
protected = 0
for torrent in torrents:
    for file in files[torrent['hash']]:
        path = DISK / torrent['save_path'] / file['name']
        assert path.is_file(), path
        st = path.stat()
        assert st.st_size > 0
        key = (st.st_dev, st.st_ino)
        if torrent['hash'] in parent:
            if key in seen_inodes:
                parent[find(torrent['hash'])] = find(seen_inodes[key])
            else:
                seen_inodes[key] = torrent['hash']

members_by_root = defaultdict(list)
for torrent in managed:
    members_by_root[find(torrent['hash'])].append(torrent)
assert len(members_by_root) == 90, len(members_by_root)
assert any(len(group) >= 4 for group in members_by_root.values())
assert any(len(group) == 1 for group in members_by_root.values())
assert any(t['category'].endswith('.cross') for t in managed)
# Synthetic clusters contain at most one torrent per logical tracker.
def logical_tracker(url):
    host = url.split('://', 1)[1].split('/', 1)[0]
    if host in ('tk.v3x.tw', 'announce.v3x.club'):
        return 'V3X'
    if host == 'tk.c411.tw':
        return 'C411'
    if host == 'tk.tr4ker.net':
        return 'TR4KER'
    return host

for group in members_by_root.values():
    labels = [logical_tracker(t['tracker']) for t in group]
    assert len(labels) == len(set(labels)), labels

for path in (DISK / 'demo/library').rglob('*'):
    if path.is_file():
        assert path.stat().st_nlink >= 2
        protected += 1
assert protected >= 7

with sqlite3.connect(ROOT / 'data/fixture.sqlite3') as conn:
    counts = {name: conn.execute(f'SELECT count(*) FROM {name}').fetchone()[0]
        for name in ['app_state','activity_events','history_snapshots','deletion_batches','management_runs']}
assert counts['activity_events'] > 1000, counts
assert counts['history_snapshots'] >= 30
assert counts['deletion_batches'] >= 3
assert counts['management_runs'] >= 30
print(f'Fixture verified: {len(torrents)} torrents, {len(members_by_root)} groups; '
      f'{counts["activity_events"]} activity events, {counts["history_snapshots"]} disk snapshots, '
      f'{counts["management_runs"]} execution entries.')
