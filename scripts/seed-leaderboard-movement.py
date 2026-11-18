#!/usr/bin/env python3
"""Seed historical rank changes for the synthetic dashboard only.

Run after the first fixture --once --dry cycle, then run another dry cycle.
That second cycle computes real rank deltas from these deliberately shuffled
historical standings. No changes to production ranking or sorting algorithms.
"""
from __future__ import annotations

import json
import sqlite3
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DB = ROOT / "data" / "fixture.sqlite3"
PERIODS = ("all", "week", "month", "year")
# A mix of increases, decreases and unchanged rows, visible near the top.
SWAPS = ((0, 1), (2, 5), (3, 8), (6, 10), (11, 15), (16, 22), (24, 29), (30, 38))


def ordered(torrents: list[dict]) -> list[dict]:
    # Match src/engine.rs:period_ranks, including the deterministic hash tie-break.
    return sorted(torrents, key=lambda row: (-(row["uploaded_bytes"] - row["downloaded_bytes"]), row["hash"]))


def reordered_positions(size: int) -> list[int]:
    previous = list(range(size))
    for a, b in SWAPS:
        if b < size:
            previous[a], previous[b] = previous[b], previous[a]
    return previous


def previous_ranks(torrents: list[dict]) -> dict[str, int]:
    rows = ordered(torrents)
    positions = reordered_positions(len(rows))
    return {row["hash"]: positions[i] + 1 for i, row in enumerate(rows)}


def starts(timestamp: int) -> dict[str, int]:
    utc = datetime.fromtimestamp(timestamp, tz=timezone.utc)
    day = utc.replace(hour=0, minute=0, second=0, microsecond=0)
    return {
        # Match src/util.rs:period_starts. Rust currently starts weeks on Sunday
        # (1970-01-01 was Thursday, hence the +4 modulo 7).
        "week": int((day - timedelta(days=(day.weekday() + 1) % 7)).timestamp()),
        "month": int(day.replace(day=1).timestamp()),
        "year": int(day.replace(month=1, day=1).timestamp()),
    }


def get_dashboard(conn: sqlite3.Connection) -> dict:
    result = conn.execute("SELECT payload FROM latest_dashboard WHERE id = 1").fetchone()
    if result is None:
        raise RuntimeError("Run the first fixture cycle before seeding leaderboard movement")
    data = json.loads(result[0])
    for period in PERIODS:
        if not data.get("periods", {}).get(period, {}).get("torrents"):
            raise RuntimeError(f"Missing torrent leaderboard for {period}")
    return data


def seed() -> None:
    with sqlite3.connect(DB) as conn:
        dashboard = get_dashboard(conn)
        now = int(dashboard["generated_at"])
        boundaries = starts(now)
        for period, boundary in boundaries.items():
            ranks = previous_ranks(dashboard["periods"][period]["torrents"])
            conn.execute(
                """INSERT OR REPLACE INTO leaderboard_baselines
                   (period, period_start, captured_at, rankings_json)
                   VALUES (?, ?, ?, ?)""",
                (period, boundary, max(boundary, now - 2 * 86400), json.dumps(ranks)),
            )

        # All-time compares against the previous *dashboard*, not a SQL baseline.
        # Swap complete upload/download pairs so the Rust calculation sees a
        # plausible previous ranking while retaining a valid full dashboard.
        rows = ordered(dashboard["periods"]["all"]["torrents"])
        for a, b in SWAPS:
            if b < len(rows):
                left, right = rows[a], rows[b]
                for field in ("uploaded_bytes", "downloaded_bytes"):
                    left[field], right[field] = right[field], left[field]
        dashboard["generated_at"] = max(1, now - 3600)
        conn.execute(
            "UPDATE latest_dashboard SET generated_at = ?, payload = ? WHERE id = 1",
            (dashboard["generated_at"], json.dumps(dashboard)),
        )
    print("Seeded fixture leaderboard baselines for all time, week, month and year.")


def verify() -> None:
    with sqlite3.connect(DB) as conn:
        dashboard = get_dashboard(conn)
        for period in PERIODS:
            changes = dashboard.get("rank_changes", {}).get(period, {})
            up = sum(value > 0 for value in changes.values())
            down = sum(value < 0 for value in changes.values())
            if up == 0 or down == 0:
                raise RuntimeError(f"Fixture {period} ranking has no visible up/down changes: +{up}, -{down}")
            print(f"Fixture {period} leaderboard: {up} up, {down} down.")


def main() -> None:
    if len(sys.argv) > 2 or (len(sys.argv) == 2 and sys.argv[1] != "--verify"):
        raise SystemExit("Usage: seed-leaderboard-movement.py [--verify]")
    if not DB.is_file() or DB.is_symlink():
        raise RuntimeError(f"Fixture SQLite database not found: {DB}")
    if len(sys.argv) == 2:
        verify()
    else:
        seed()


if __name__ == "__main__":
    main()
