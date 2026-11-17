# graine

Graine manages completed qBittorrent torrents under a filesystem-wide storage limit. It ranks inode-linked torrent groups, protects recent seeds, removes eligible low-ranking groups when the disk fills up, and serves a web dashboard.

The Rust service handles the qBittorrent API, filesystem metadata, retention decisions, SQLite persistence and an hourly worker. Axum serves the API and compiled React frontend from the same process. The frontend uses React, TypeScript, Vite, Tailwind CSS and shadcn/ui.

## How it works

- Only **completed torrents** in `managed_categories` are managed. `<category>.cross` is automatically included.
- Torrents sharing a hardlinked file are grouped into **one cluster**, including cross-seeds. A cluster is the unit of ranking and eviction.
- **Cost** is the logical size of unique inodes whose hardlinks are all accounted for within the cluster. Files with external hardlinks are not considered reclaimable. Graine does not count the same file once per cross-seed.
- A cluster remains in **grace** while any original (non-cross-seed) has seeded for fewer than `grace_days`. If there are no originals, all members are considered. Grace clusters are not evicted.
- The order is grace protection first, then configured tracker priority, then score. Score uses lifetime upload rate adjusted for inactivity, divided by reclaimable logical size. Zero-cost groups are never evicted.
- Graine uses `df` to determine overall filesystem occupancy. Above `high_watermark`, it evicts eligible groups, worst first, until projected usage reaches `low_watermark` or no more eligible groups remain.

**Storage caveat:** reclaimable *logical* bytes are not a promise of physical blocks freed. Hardlinks are deduplicated, but Btrfs reflinks, compression and snapshots can affect actual free space. The filesystem's occupancy is measured independently with `df`. Graine is designed for Linux with ext4 or Btrfs; ZFS-specific accounting is not implemented.

## Dashboard

- **Overview:** tracker net-upload history (total / regular / cross-seed), searchable ranking, per-cluster details and grace indicator.
- **Storage:** current usage and stacked category history (Radarr, Sonarr, Autobrr and unclassified).
- **Leaderboard:** up to 100 torrents for the selected period, including deleted torrents retained in SQLite. Rank changes compare with the previous run for *All time* or the first recorded baseline of the current week/month/year.
- **History:** scheduled, manual and command-line runs, including their outcome, duration and linked torrent deletions. Filter by **All**, **Deletions** or **Errors**. Older SQLite snapshots remain available as archived entries.

The interface follows the browser language (French or English) and light/dark preference. Its refresh icon **starts a management cycle**, not just a page reload; it disables while one is running. The automatic worker and manual action cannot overlap.

## Install with Docker Compose

A published image is available as `ghcr.io/0x7375/graine:latest` once the GitHub Actions publishing workflow has completed. See [`deploy/`](deploy/) for an example `compose.yaml` and `config.toml` suitable for a Linux Docker or Podman host (including NixOS and UGREEN NAS devices with Docker support).

On the host:

1. Copy `deploy/compose.yaml` and `deploy/config.toml` into your chosen directory.
2. Adjust the qBittorrent URL, tracker groups, categories, and absolute `disk_path`. Torrent paths returned by qBittorrent must also exist inside Graine's container; preserve them with a read-only bind mount.
3. Create a file containing **only** the qBittorrent WebUI password and mount it at the configured `password_file`.
4. Persist `/var/lib/graine` for SQLite. Use `management_enabled = false` to disable the scheduled worker and keep `dry_run = true` while testing: manual runs are still possible through the button.
5. Run `docker compose pull && docker compose up -d` and inspect `docker compose logs -f graine`.

The example uses Linux host networking so `127.0.0.1` can reach a local qBittorrent WebUI. A bridged container also works, but requires an accessible qBittorrent address, Axum listening on a container interface, and a published web port. See [`deploy/README.md`](deploy/README.md).

**Security:** `POST /api/run` can trigger *real torrent deletions* and has no application-level authentication. Never expose Graine to an untrusted network. Put it behind a protected reverse proxy or restrict access at the host/firewall level. A read-only volume mount does **not** stop Graine from asking qBittorrent to delete files through its API.

## Configuration

Copy [`config.example.toml`](config.example.toml) and adjust it. By default the executable reads `/etc/graine/config.toml`; override with `--config PATH`. Passwords are read from `password_file`, `--password-file PATH`, or `QBIT_PW` (fallback).

| Option | Meaning |
| --- | --- |
| `url` | qBittorrent WebUI base URL |
| `disk_path` | Filesystem containing the torrent data; absolute paths must agree with qBittorrent |
| `managed_categories` | Managed categories, automatically including `.cross` variants |
| `trackers` | Logical tracker names, announce hosts, priorities |
| `high_watermark` / `low_watermark` | Begin eviction above the high threshold, target the low threshold |
| `grace_days` | Seeding grace period for original torrents |
| `interval_seconds` | Worker frequency; default **3600** seconds |
| `management_enabled` | Enable hourly worker (and one cycle immediately on startup); default true |
| `dry_run` | Preview decisions without deleting or persisting analytics/history |
| `database_path` | Persistent SQLite database path |
| `listen` / `web_dir` | Axum bind address and compiled React directory |
| `fixture_path` | Optional local JSON fixture; always read-only, never connects to qBittorrent |

```sh
# Continuous service
./target/release/graine --config ./config.toml

# Run one management cycle
./target/release/graine --config ./config.toml --once

# Preview once, without deleting torrents
./target/release/graine --config ./config.toml --once --dry
```

### SQLite and backups

Graine keeps analytics, torrent history, execution history, ranking baselines, and the most recent dashboard in one SQLite database. No JSON/JSONL import is performed by current versions. Removed torrents remain in historical leaderboard data.

For a consistent backup, use SQLite's online backup command. This works while the service is running:

```sh
sqlite3 /var/lib/graine/graine.sqlite3 ".backup '/path/you/can/write/graine-backup.sqlite3'"
```

Do not copy only a live `.sqlite3` file while WAL transactions may be pending. Do not commit your database, credentials, downloaded data or torrent names to a public repository.

## Local development

Use the Nix development shell, or install Rust, Node.js, pnpm and Watchexec yourself:

```sh
nix develop
pnpm --dir web install
make backend-dev    # terminal 1: Axum on http://127.0.0.1:3000
make frontend-dev   # terminal 2: Vite on http://localhost:5173
```

`config.dev.toml` sets `management_enabled = false`, so starting the backend does not run a scheduled scan; fixture mode forces dry-run for any manually triggered scan. Vite proxies `/api` requests to Axum.

For **realistic dashboard data**, back up SQLite on the server, copy that backup to `data/dev.sqlite3` (with the local backend stopped), then start the two development processes. No live server or filesystem mount is needed to view existing dashboard data.

For **UI demos and retention tests**, generate a complete synthetic dataset:

```sh
make fixture-run          # regenerate 90 groups / 201 torrents, history and isolated SQLite; runs --once --dry
make fixture-backend-dev  # serve the synthetic dashboard at localhost:3000 (terminal 1)
make frontend-dev         # Vite frontend at localhost:5173 (terminal 2)
```

The generator (`scripts/generate-fixtures.py`) and `make fixture-check` use only Python's standard library. It creates invented media titles and tracker counters, single/multi-torrent groups, multiple cross-seeds, multi-file torrents, external hardlinks, grace-protected groups, incomplete/unmanaged torrents, historical uploads, disk usage, failed runs and deletion batches. Actual local hardlinks exercise the inode logic. Media files are sparse (large logical sizes, minimal disk use); **the figures are fictional and are not physical space measurements**. It writes only to `fixtures/disk/demo/`, the ignored `fixtures/qbittorrent.generated.json` and the separate `data/fixture.sqlite3`. Regeneration replaces these generated files; it does not touch `data/dev.sqlite3` or production data.

Fixture mode never contacts qBittorrent and refuses deletion. Do not use SSHFS to test hardlinks or inode accounting: its inode/link metadata may not match the server filesystem.

## Build and checks

```sh
make check             # formatting, cargo check/test/clippy and TypeScript
make build             # Vite production build and cargo release build
```

For deployment, the [Dockerfile](Dockerfile) builds both the frontend and Rust binary; Node.js is **not** required in the runtime image. GitHub Actions publishes Docker images to GHCR on pushes to `main` and version tags. Commit **both** `Cargo.lock` and `web/pnpm-lock.yaml` for reproducible builds. Nix can also provide the development shell; the OCI image is the primary deployment artifact.

## API

| Endpoint | Function |
| --- | --- |
| `GET /api/health` | Health check |
| `GET /api/dashboard` | Latest dashboard payload |
| `GET /api/run/status` | Worker activity and last result |
| `POST /api/run` | Start a manual cycle; `202` accepted or `409` if one is running |
| `GET /api/runs?limit=30&before=ID&filter=all` | Paginated run history; filters: `all`, `deletions`, `errors` |

Page loads do not start a management cycle. Only the scheduled worker, `--once`, or `POST /api/run` can do so.
