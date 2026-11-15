# Graine

Graine is a Rust torrent-management service with an Axum API and a React dashboard. Rust remains responsible for qBittorrent interaction, torrent scoring, hardlink-aware storage accounting, retention ranking, and eviction decisions. React only renders API data.

## Local development without the server filesystem

The repository has two separate local workflows. Neither needs `/data/shared`, a qBittorrent password, an SSH tunnel, or access to the production server.

### 1. Dashboard development using the server's historical data

Copy the old application state files into `data/legacy/`. From the old Graine working directory on your machine:

```sh
./scripts/import-legacy.sh /path/to/copied/server/state
```

The script copies any of these files that are present:

- `analytics.json`
- `activity.jsonl`
- `history.jsonl`
- `deletions.jsonl`

Alternatively, place them manually in `data/legacy/`. The SQLite import runs on startup. It is transactional and idempotent; source files are never modified. If no legacy files exist yet, Graine defers the import marker, so you can copy them later and restart. Malformed JSON or incompatible analytics versions stop the import with an error rather than dropping records.

The legacy exports provide analytics, activity, disk history, and deletion history, but they do not contain the current torrent list or current ranking. Graine now builds an initial historical dashboard from those records. It labels that data as imported and leaves the current ranking empty until a fixture scan or real management scan runs. Copy `analytics.json` as well as the JSONL files when available: it stores the lifetime baseline used for the all-time leaderboard. Without it, historical activity events still import, but pre-observation lifetime totals cannot be reconstructed from JSONL alone.

### 2. Full Rust management-cycle development using a safe fixture

Initialize a local filesystem dataset:

```sh
./scripts/init-fixtures.sh
```

It creates small files in `fixtures/disk/`, including two real hardlink cases:

- A movie shared by two torrent save paths (same inode, two links).
- An episode with an additional hardlink outside the managed torrent paths (same inode, external link).

`fixtures/qbittorrent.json` provides a local qBittorrent snapshot with four example torrents. Graine reads the fixture directly; it does not contact qBittorrent. Fixture mode always forces dry-run and the delete function explicitly refuses to run in fixture mode.

Run one complete Rust calculation:

```sh
cargo run -- --config config.dev.toml --once --dry
```

This exercises torrent filtering, file metadata inspection, inode clustering, scoring, ranking, disk accounting, watermark decisions, and dashboard serialization against a real local filesystem. `df` measures the filesystem containing `fixtures/disk`, not the server disk. The run updates the latest dashboard preview, but does not persist analytics/history or delete anything.

The fixture deliberately uses actual local hardlinks; a JSON-only mock of inode values would not test the OS metadata behavior Graine relies on.

### 3. Start the backend and frontend

Install the development dependencies:

```sh
nix develop
cd web && pnpm install && cd ..
```

The backend config has `management_enabled = false`, so starting it cannot trigger a qBittorrent scan. It serves the imported dashboard from SQLite:

```sh
# Terminal 1
watchexec --restart --watch src --watch Cargo.toml -- cargo run -- --config config.dev.toml

# Terminal 2
cd web && pnpm dev
```

Open `http://localhost:5173`. Vite proxies `/api` to Axum on `127.0.0.1:3000`. Frontend changes hot-reload; backend changes restart the Rust process. Set `GRAINE_API_TARGET` if Axum listens elsewhere.

To populate the ranking from the fixture, run the one-shot fixture command from workflow 2. Then refresh the dashboard. You can stop and restart the dashboard-only backend independently; it does not scan on startup.

### Local state and reset

Development data is stored separately from production:

- `data/legacy/`: copied source JSON/JSONL files (kept unchanged).
- `data/dev.sqlite3`: imported history and current dashboard payload.
- `fixtures/disk/`: generated local file/hardlink test dataset.
- `fixtures/qbittorrent.json`: local qBittorrent response fixture.

To start the development database from scratch and re-import the copied files, stop Graine and remove only the development DB files:

```sh
rm -f data/dev.sqlite3 data/dev.sqlite3-wal data/dev.sqlite3-shm
```

Then restart the backend. Do not delete `data/legacy/` unless you have another copy of the source data.

## Configuration and runtime behavior

The standard configuration path is `/etc/graine/config.toml`. Override it with `--config PATH`. The qBittorrent password can be supplied with `password_file` in TOML or `--password-file PATH`; `QBIT_PW` remains supported.

`interval_seconds` defaults to 3600 (one hour). `management_enabled` defaults to `true`; set it to `false` for dashboard-only operation. The worker runs immediately when enabled, then at the configured interval. It does not overlap its own management cycles.

```sh
# Start the production-style service (requires a real config and password file).
graine --config /etc/graine/config.toml

# Run exactly one real management cycle.
graine --config /etc/graine/config.toml --once

# Run exactly one dry management cycle.
graine --config /etc/graine/config.toml --once --dry
```

## Nix development shell

```sh
nix develop
```

The shell provides Rust/Cargo, rustfmt, Clippy, rust-analyzer, Node.js, pnpm, SQLite CLI, and Watchexec. Node and pnpm are not needed by the production Rust service.

## Build

```sh
cd web
pnpm install
pnpm build
cd ..
cargo build --release
```

The repository must contain its generated `Cargo.lock` before `nix build`. Axum serves the compiled `web/dist` directory and `/api/*` from the same origin; Node.js is not needed at runtime.

## Important distinction: SSH tunnel vs filesystem access

An SSH tunnel to qBittorrent exposes only its HTTP API. The management cycle also runs `df` against `disk_path` and calls `stat` on torrent files. For a faithful local management test, the relevant files and hardlinks must exist locally. SSHFS is not recommended for this application because its inode/hardlink behavior does not reliably reproduce the local filesystem semantics Graine uses. The fixture approach above does.

## API

- `GET /api/health` — process health.
- `GET /api/dashboard` — latest Rust-generated dashboard payload, including overview metrics, disk history, tracker history, ranking, leaderboards, and eviction history.

Page loads and API requests never trigger a management scan.
