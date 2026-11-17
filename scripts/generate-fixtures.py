#!/usr/bin/env python3
"""Deterministic, safe synthetic Graine dataset. Python standard library only.

Creates qBittorrent-like JSON, real local hardlinks, and a separate SQLite
history database. All files remain inside fixtures/disk/demo and data/fixture.sqlite3.
Never contacts qBittorrent or touches production state.
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import random
import shutil
import sqlite3
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DISK = ROOT / 'fixtures' / 'disk'
DEMO = DISK / 'demo'
SNAPSHOT = ROOT / 'fixtures' / 'qbittorrent.generated.json'
DB = ROOT / 'data' / 'fixture.sqlite3'
GIB = 1024 ** 3
DAY = 86400
RNG = random.Random(37061)

ADJECTIVES = ['Northern', 'Silent', 'Golden', 'Last', 'Hidden', 'Distant', 'Broken', 'Winter', 'Blue',
              'Paper', 'Electric', 'Little', 'Second', 'Midnight', 'Forgotten', 'Quiet', 'Lost']
NOUNS = ['Harbor', 'Signal', 'Garden', 'Arrival', 'Orbit', 'Promise', 'Museum', 'Memory', 'Bridge',
         'Station', 'Riders', 'Valley', 'Ocean', 'City', 'Journey', 'Pilots', 'Stories', 'Season']
TRACKERS = ['https://tk.v3x.tw/announce', 'https://tk.c411.tw/announce',
            'https://tk.tr4ker.net/announce', 'https://announce.v3x.club/announce',
            'https://unconfigured.example/announce']
CATEGORIES = ['radarr', 'sonarr', 'autobrr']

# Distinct logical trackers per cluster. The two V3X announce hosts must not
# create two V3X torrents in the same synthetic cluster.
TRACKER_BY_GROUP = [
    'https://tk.v3x.tw/announce',
    'https://tk.c411.tw/announce',
    'https://tk.tr4ker.net/announce',
    'https://unconfigured.example/announce',
]



def file_hash(seed: str) -> str:
    return hashlib.sha1(seed.encode('utf-8')).hexdigest()


def small_sparse_file(path: Path, length: int) -> None:
    """Use logical media sizes without allocating hundreds of GB locally."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('wb') as stream:
        stream.write(b'Graine synthetic fixture\n' * 128)  # some real allocated blocks
        stream.truncate(length)


def named_torrent(category: str, index: int) -> str:
    adjective = ADJECTIVES[(index * 7 + 3) % len(ADJECTIVES)]
    noun = NOUNS[(index * 5 + 2) % len(NOUNS)]
    if category == 'sonarr':
        return f'{adjective}.{noun}.S{1 + index % 6:02d}E{1 + index % 18:02d}.1080p.WEB.x265-GRAINE'
    if category == 'autobrr':
        return f'{adjective}.{noun}.Collection.{2020 + index % 7}.MULTi.1080p-GRAINE'
    return f'{adjective}.{noun}.{2014 + index % 13}.2160p.WEB-DL.x265-GRAINE'


def tracker_host(url: str) -> str:
    return url.split('://', 1)[1].split('/', 1)[0]


def build_torrents(now: int):
    torrents: list[dict] = []
    file_map: dict[str, list[dict]] = {}
    stats = {'clusters': 0, 'cross_seeds': 0, 'externally_protected': 0,
             'multifile': 0, 'chained': 0, 'incomplete': 0, 'unmanaged': 0, 'grace': 0}

    def add_torrent(group: int, ordinal: int, category: str, name: str, base_files: list[Path],
                    tracker: str, age_days: int, seeded_days: int, upload_gib: int,
                    download_gib: int, inactive_days: int, *, incomplete: bool = False,
                    unmanaged: bool = False):
        label = f'{group:04d}:{ordinal:02d}'
        h = file_hash(label)
        location = DEMO / 'torrents' / h[:12]
        location.mkdir(parents=True, exist_ok=True)
        items = []
        for source in base_files:
            target = location / source.name
            os.link(source, target)
            items.append({'name': source.name})
        current = {
            'hash': h, 'name': name, 'category': ('manual' if unmanaged else category + ('.cross' if ordinal else '')),
            'save_path': str(location.relative_to(DISK)),
            'progress': 0.34 if incomplete else 1.0,
            'seeding_time': seeded_days * DAY,
            'added_on': now - age_days * DAY,
            'last_activity': now - inactive_days * DAY,
            'uploaded': upload_gib * GIB,
            'downloaded': download_gib * GIB,
            'tracker': tracker,
        }
        torrents.append(current)
        file_map[h] = items
        return current

    for group in range(90):
        category = CATEGORIES[group % len(CATEGORIES)]
        name = named_torrent(category, group)
        # Includes singletons, pairs, triples and large multi-tracker groups.
        members = [1, 1, 1, 2, 2, 3, 3, 4][group % 8]
        is_multifile = group % 7 == 0
        num_files = 3 if is_multifile else 1
        stats['multifile'] += int(is_multifile)
        logical_gib = RNG.randint(1, 14)
        sources = []
        for file_index in range(num_files):
            src = DEMO / 'sources' / f'{group:03d}' / f'{name}.{file_index + 1}.mkv'
            small_sparse_file(src, max(512 * 1024, logical_gib * GIB // num_files))
            sources.append(src)
        # Original source path is a hardlink outside managed torrent locations;
        # remove it so it doesn't inadvertently protect every fixture.
        for member_idx in range(members):
            torrent_tracker = TRACKER_BY_GROUP[(group + member_idx) % len(TRACKER_BY_GROUP)]
            # Union-find must merge transitive sharing: A ↔ B ↔ C, even if A and C
            # share no inode directly. Use this scenario in some multi-file groups.
            files_for_member = sources
            if is_multifile and members >= 3 and group % 2 == 1:
                if member_idx == 0:
                    files_for_member = sources[:1]
                elif member_idx == 1:
                    files_for_member = sources[:2]
                elif member_idx == 2:
                    files_for_member = sources[1:]
                else:
                    files_for_member = sources[-1:]
            seeded = RNG.randint(1, 220) if member_idx == 0 else RNG.randint(1, 150)
            age = seeded + RNG.randint(7, 90)
            inactive = RNG.choice([0, 1, 2, 4, 8, 16, 32, 70])
            uploaded = RNG.randint(4, 5500) if member_idx == 0 else RNG.randint(1, 1700)
            add_torrent(group, member_idx, category, name + (f' [{tracker_host(torrent_tracker)}]' if member_idx else ''),
                        files_for_member, torrent_tracker, age, seeded, uploaded, RNG.randint(0, 50), inactive)
            if member_idx > 0:
                stats['cross_seeds'] += 1
            if member_idx == 0 and seeded < 7:
                stats['grace'] += 1
        # Remove the source path, leaving exactly one path per torrent unless deliberately protected.
        for src in sources:
            if group % 13 == 0:
                external = DEMO / 'library' / f'{group:03d}' / src.name
                external.parent.mkdir(parents=True, exist_ok=True)
                os.link(src, external)
            src.unlink()
        stats['externally_protected'] += int(group % 13 == 0)
        stats['chained'] += int(is_multifile and members >= 3 and group % 2 == 1)
        stats['clusters'] += 1

    # Incomplete torrents and unmanaged categories should be ignored by Ranking.
    for i in range(12):
        p = DEMO / 'sources' / 'extras' / f'incomplete-{i}.bin'
        small_sparse_file(p, 128 * 1024 * 1024)
        is_unmanaged = i >= 8
        add_torrent(1000 + i, 0, 'sonarr', f'Pending.Season.{i + 1}.WEB-GRAINE', [p], TRACKERS[i % 4],
                    6, 1, 0, 0, 0, incomplete=not is_unmanaged, unmanaged=is_unmanaged)
        p.unlink()
        stats['unmanaged' if is_unmanaged else 'incomplete'] += 1

    return torrents, file_map, stats


def seed_database(torrents: list[dict], now: int) -> None:
    DB.parent.mkdir(parents=True, exist_ok=True)
    # This is a dedicated, documented fixture DB, never data/dev.sqlite3.
    for suffix in ['', '-wal', '-shm']:
        dest = Path(str(DB) + suffix)
        if dest.is_symlink():
            raise RuntimeError(f'Refusing to overwrite symlink {dest}')
        if dest.exists():
            dest.unlink()
    conn = sqlite3.connect(DB)
    conn.executescript('''
        CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE app_state (key TEXT PRIMARY KEY, json TEXT NOT NULL);
        CREATE TABLE activity_events (seq INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT UNIQUE NOT NULL,
                                      timestamp INTEGER NOT NULL, payload TEXT NOT NULL);
        CREATE TABLE history_snapshots (seq INTEGER PRIMARY KEY AUTOINCREMENT, timestamp INTEGER NOT NULL, payload TEXT NOT NULL);
        CREATE TABLE deletion_batches (seq INTEGER PRIMARY KEY AUTOINCREMENT, batch_id TEXT UNIQUE NOT NULL,
                                       timestamp INTEGER NOT NULL, payload TEXT NOT NULL);
        CREATE TABLE management_runs (id INTEGER PRIMARY KEY AUTOINCREMENT, started_at INTEGER NOT NULL,
            finished_at INTEGER, duration_ms INTEGER, trigger TEXT NOT NULL, mode TEXT NOT NULL,
            status TEXT NOT NULL, summary_json TEXT, snapshot_json TEXT, deletion_batch_json TEXT, error TEXT);
    ''')
    conn.execute('INSERT INTO metadata VALUES (?,?)', ('management_run_history_backfill_v1', 'complete'))
    state = {'version': 2, 'tracking_started_at': now - 65 * DAY, 'trackers': {}, 'torrents': {}}
    visible = [t for t in torrents if t['category'] != 'manual' and t['progress'] >= 1.0]
    for torrent in visible:
        host = tracker_host(torrent['tracker'])
        last_seen = now - RNG.randint(0, 1800)
        state['torrents'][torrent['hash']] = {
            'name': torrent['name'], 'tracker': host, 'category': torrent['category'],
            'added_on': torrent['added_on'], 'first_seen_at': state['tracking_started_at'],
            'last_seen_at': last_seen, 'all_time_uploaded_bytes': torrent['uploaded'],
            'all_time_downloaded_bytes': torrent['downloaded'],
            'last_uploaded_counter': torrent['uploaded'], 'last_downloaded_counter': torrent['downloaded'],
        }
        totals = state['trackers'].setdefault(host, {
            'total': {'uploaded_bytes': 0, 'downloaded_bytes': 0},
            'regular': {'uploaded_bytes': 0, 'downloaded_bytes': 0},
            'cross_seed': {'uploaded_bytes': 0, 'downloaded_bytes': 0},
        })
        group = 'cross_seed' if torrent['category'].endswith('.cross') else 'regular'
        for part in ('total', group):
            totals[part]['uploaded_bytes'] += torrent['uploaded']
            totals[part]['downloaded_bytes'] += torrent['downloaded']
        # Activity is deliberately smaller than lifetime counters, preserving an all-time baseline.
        observed_up = torrent['uploaded'] // 3
        observed_down = torrent['downloaded'] // 3
        for day in range(48):
            if (int(torrent['hash'][:4], 16) + day) % 4 != 0:
                continue
            ts = now - (48 - day) * DAY + (int(torrent['hash'][:6], 16) % 12) * 3600
            amount_up = observed_up // 36
            amount_down = observed_down // 36
            if not amount_up and not amount_down:
                continue
            payload = {
                'event_id': f'fixture:{torrent["hash"]}:{day}', 'timestamp': ts,
                'hash': torrent['hash'], 'name': torrent['name'],
                'tracker': host, 'category': torrent['category'],
                'is_cross_seed': torrent['category'].endswith('.cross'),
                'uploaded_bytes': amount_up, 'downloaded_bytes': amount_down,
            }
            conn.execute('INSERT INTO activity_events(event_id,timestamp,payload) VALUES (?,?,?)',
                         (payload['event_id'], ts, json.dumps(payload)))

    # Keep deleted torrents in all-time history even though they are absent from the fixture API.
    for i in range(12):
        h = file_hash(f'deleted:{i}')
        host = tracker_host(TRACKER_BY_GROUP[i % 4])
        category = CATEGORIES[i % 3]
        up, down = (400 + i * 180) * GIB, i * GIB
        state['torrents'][h] = {'name': f'Removed.{named_torrent(category, i + 200)}',
            'tracker': host, 'category': category, 'added_on': now - 170 * DAY,
            'first_seen_at': now - 65 * DAY, 'last_seen_at': now - (2 + i) * DAY,
            'all_time_uploaded_bytes': up, 'all_time_downloaded_bytes': down,
            'last_uploaded_counter': up, 'last_downloaded_counter': down}
        for part in ('total', 'regular'):
            state['trackers'][host][part]['uploaded_bytes'] += up
            state['trackers'][host][part]['downloaded_bytes'] += down
    conn.execute('INSERT INTO app_state VALUES (?,?)', ('analytics_state', json.dumps(state)))

    # Disk history and execution history with some deletions and errors.
    # Use the local filesystem's real size to avoid abrupt jumps when a fixture dry-run appends a current snapshot.
    data = subprocess.check_output(['df', '-B1', '--output=used,avail', str(DISK)], text=True).strip().splitlines()[1].split()
    disk_used, disk_avail = int(data[0]), int(data[1])
    disk_total = disk_used + disk_avail
    for i in range(48):
        timestamp = now - (48 - i) * DAY
        delta = int(disk_total * (0.025 * math.sin(i / 7) + 0.006 * math.sin(i / 2)))
        usage = max(0, min(disk_total, disk_used - delta))
        radarr = int(usage * (0.28 + 0.03 * math.sin(i / 9)))
        sonarr = int(usage * (0.20 + 0.02 * math.cos(i / 5)))
        autobrr = int(usage * 0.12)
        snapshot = {
            'timestamp': timestamp, 'disk_used_bytes': usage, 'disk_total_bytes': disk_total,
            'disk_used_percent': usage / disk_total * 100, 'projected_used_bytes': usage,
            'projected_used_percent': usage / disk_total * 100,
            'high_watermark_percent': 85., 'low_watermark_percent': 82.,
            'reclaimable_bytes': 84 * GIB, 'protected_bytes': 13 * GIB,
            'estimated_reclaimed_bytes': 0, 'actual_reclaimed_bytes': None,
            'category_usage': {'radarr': radarr, 'sonarr': sonarr, 'autobrr': autobrr},
        }
        conn.execute('INSERT INTO history_snapshots(timestamp,payload) VALUES (?,?)', (timestamp, json.dumps(snapshot)))
        batch = None
        if i in (7, 18, 33, 42):
            clusters = []
            for member in range(1 + i % 3):
                size = (2 + (i + member) % 10) * GIB
                clusters.append({
                    'name': f'Removed.{named_torrent(CATEGORIES[member % 3], i * 3 + member)}',
                    'cross_seed_count': member % 3, 'torrent_names': [f'Original {member}'] + [f'Cross-seed {c}' for c in range(member % 3)],
                    'tracker_hosts': ['C411', 'TR4KER'], 'category': CATEGORIES[member % 3],
                    'reason': 'Disk watermark', 'priority': 10, 'score': round(0.5 + member / 3, 2),
                    'estimated_reclaimed_bytes': size, 'uploaded_bytes': (50 + 8 * member) * GIB,
                    'seeding_days': 30.0,
                })
            total = sum(entry['estimated_reclaimed_bytes'] for entry in clusters)
            batch = {'batch_id': f'fixture-{timestamp}', 'timestamp': timestamp,
                     'estimated_reclaimed_bytes': total, 'observed_reclaimed_bytes': None,
                     'clusters': clusters}
            conn.execute('INSERT INTO deletion_batches(batch_id,timestamp,payload) VALUES (?,?,?)',
                         (batch['batch_id'], timestamp, json.dumps(batch)))
        failed = i in (20, 35)
        conn.execute('''INSERT INTO management_runs(started_at,finished_at,duration_ms,trigger,mode,status,
                         snapshot_json,deletion_batch_json,error)
                        VALUES (?,?,?,?,?,?,?,?,?)''',
                     (timestamp, timestamp + 3, 1200 + (i % 8) * 700,
                      'manual' if i % 11 == 0 else 'scheduled', 'dry' if i % 12 == 0 else 'live',
                      'error' if failed else 'ok', None if failed else json.dumps(snapshot),
                      json.dumps(batch) if batch else None,
                      'Fixture: simulated qBittorrent timeout' if failed else None))
    conn.commit()
    conn.close()


def main():
    if DISK.is_symlink() or DB.parent.is_symlink() or SNAPSHOT.is_symlink():
        raise RuntimeError('Refusing to generate fixtures through a symlinked directory or output')
    if DEMO.is_symlink():
        raise RuntimeError(f'Refusing to replace symlink {DEMO}')
    if DEMO.exists():
        shutil.rmtree(DEMO)  # generated fixture subtree only
    DEMO.mkdir(parents=True)
    now = int(time.time())
    torrents, files, stats = build_torrents(now)
    SNAPSHOT.write_text(json.dumps({'torrents': torrents, 'files': files}, indent=2) + '\n')
    seed_database(torrents, now)
    # Cross-seeds genuinely share an inode; externally linked files survive the entire group.
    example = torrents[0]
    path = DISK / example['save_path'] / files[example['hash']][0]['name']
    assert path.stat().st_nlink >= 2
    print(f'Generated {len(torrents)} torrents across {stats["clusters"]} managed groups, '
          f'{stats["cross_seeds"]} cross-seeds, {stats["externally_protected"]} protected groups.')
    print(f'Extra cases: {stats["multifile"]} multi-file groups, {stats["chained"]} transitive groups, '
          f'{stats["incomplete"]} incomplete and {stats["unmanaged"]} unmanaged torrents.')
    print('All file contents and names are synthetic. No network connection is used.')
    print(f'Fixture DB: {DB} (dedicated; data/dev.sqlite3 is untouched)')


if __name__ == '__main__':
    main()
