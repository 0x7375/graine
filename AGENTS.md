# Graine — project guide

## Purpose

Graine manages completed qBittorrent torrents to maximize useful private-tracker seeding while keeping filesystem usage below configured limits. It periodically ranks related torrents, identifies which deletions can reclaim disk space, removes the least valuable eligible clusters when the filesystem crosses its high watermark, and exposes a dashboard.

Incomplete torrents are intentionally outside Graine's ranking and deletion logic. They remain under the download client's / Cleanuparr workflow.

## Stack

- Rust, Axum, Tokio and ureq for the single backend service.
- React, TypeScript, Vite, Tailwind CSS, shadcn/ui and Lucide for the frontend.
- SQLite via rusqlite for persistent state, activity, history, deletion batches and the latest dashboard payload.
- Unix/Linux filesystem metadata (`dev`, inode, hardlink count, file length, allocated blocks) and `df` for actual filesystem usage.
- `web/dist` is built by pnpm and served by Axum through tower-http. Node.js is not a runtime dependency.

## Source layout

```text
src/
  main.rs          # CLI, configuration loading, service startup
  server.rs        # Axum routes, static frontend serving and worker scheduler
  engine.rs        # One complete management cycle; orchestration only
  config.rs        # TOML schema, defaults, validation, tracker matching
  model.rs         # Domain, persisted-state and API/dashboard types
  db.rs            # SQLite schema, legacy JSONL import, persistence
  qbittorrent.rs   # qBittorrent authentication/API plus read-only local JSON fixture source
  storage.rs       # df, file metadata, inode clustering, reclaimable cost and score
  analytics.rs     # Counters, transfer events, periods and leaderboard calculations
  dashboard.rs     # Domain-to-dashboard row/history adapters
web/
  src/             # React and TypeScript application
  components.json  # shadcn/ui configuration
```

## Runtime and scheduling

The default configuration path is `/etc/graine/config.toml`; `--config PATH` overrides it. The qBittorrent password is read from `password_file`, overridden by `--password-file PATH`; `QBIT_PW` remains a compatibility fallback.

When `management_enabled` is true, the worker runs immediately at service startup and then every `interval_seconds` (default 3600). A single sequential worker owns the cycle; it never starts a second cycle while the first is still executing. Missed intervals are skipped. Set `management_enabled = false` for dashboard-only development; `--once` still explicitly executes one cycle and exits. `dry_run` and `--dry` preview decisions without deleting torrents or persisting analytics, activity events or history.

`fixture_path` selects a local JSON qBittorrent snapshot with torrent and per-hash file records. Fixture mode must always force dry-run, validate that every fixture file resolves beneath `disk_path`, avoid qBittorrent HTTP/password access, and refuse deletion. Keep fixture paths relative to the local fixture root. `scripts/init-fixtures.sh` creates actual local hardlinks; do not replace inode values with fake JSON metadata.

The Axum server starts independently of qBittorrent availability. API requests read the most recently stored dashboard payload and never initiate a torrent scan.

## Retention invariants — do not change without explicit instruction

1. Only complete managed torrents (`progress >= 1.0`) are inspected and ranked.
2. Torrents sharing any inode are joined into one cluster. Deletion decisions apply to the entire cluster, including cross-seeds sharing hardlinked data.
3. `cost` is logical file size that can actually be reclaimed after removing every torrent in the cluster. An inode is reclaimable when the number of distinct paths represented by the cluster is at least its hardlink count.
4. `disk_bytes` counts allocated filesystem blocks, deduplicated by inode, for chart attribution. `protected_disk_bytes` counts allocated blocks whose external hardlinks keep the data on disk after the cluster is removed.
5. Cluster category is attributed to the oldest non-cross-seed torrent's base category; if there is no original, use the oldest cluster member's base category.
6. Grace is global. A cluster remains protected while any original/anchor torrent has seeded for less than `grace_days`; clusters with no original use their members as the fallback.
7. Best-first order is: grace-protected first, then higher tracker priority, then higher score. Eviction walks that order in reverse.
8. The global filesystem watermark is the only storage limit. If `df` usage exceeds `high_watermark`, Graine evicts eligible clusters until projected usage reaches `low_watermark`. Grace clusters and clusters with `cost == 0` cannot be evicted.
9. Tracker priority affects ranking order; unconfigured tracker priority defaults to zero. Hiding unconfigured trackers is a presentation/analytics filter, not a torrent-management filter.
10. Dry runs must not persist analytics, append activity/history/deletion records, or delete torrents. The latest dashboard preview may be exposed to the frontend.

The score sums each member torrent's lifetime upload-per-seeding-day estimate, applies a per-torrent inactivity factor `1 / (1 + inactive_days / 7)`, and divides by cluster reclaimable cost. A zero-cost cluster receives an infinite score because it cannot reclaim storage. Do not alter score formulas or sorting semantics during presentation/backend migration.

## Persistent data

The SQLite database stores:

- `analytics_state`: lifetime tracker and torrent counters and last-seen counters.
- `activity_events`: observed qBittorrent upload/download deltas, deduplicated by event ID.
- `history_snapshots`: full disk-usage snapshots including category usage for the stacked disk chart.
- `deletion_batches`: deletion batches and per-cluster metadata; observed `df` reclamation belongs to the batch, not an individual cluster.
- `latest_dashboard`: most recent dashboard response served by the API.

At startup, `db.rs` imports `analytics.json`, `activity.jsonl`, `history.jsonl` and `deletions.jsonl` from `legacy_data_dir` in a single transaction. If no source files exist, defer the import marker so files can be copied later. Once any source exists, copy all available source files before startup; the import is guarded by a metadata marker and leaves source files untouched. Invalid or unsupported records stop import; never silently discard them. Existing compatibility rules remain: old history may lack `category_usage`, and old deletion records may use `pool` as an alias for `category`. When no current dashboard exists, `engine.rs` seeds a historical dashboard from the imported activity/analytics/history/deletion data. Legacy exports do not include a current torrent ranking, so do not invent one.

## Dashboard contract

Rust owns all calculations. React renders the API payload and does not reimplement torrent scoring, ranking, filesystem accounting or eviction decisions.

Preserve the following information and visual semantics:

- General statistics and current run status.
- Torrent ranking as a table, including cluster, tracker, category, status, priority, score, cost, data size, uploaded bytes, seeding days and torrent/cross-seed counts.
- Historical disk usage as a stacked area/line chart with Radarr, Sonarr, Autobrr and Other/unclassified attribution, plus high/low watermark references.
- Tracker performance as a cumulative net-transfer line chart with total, regular and cross-seed metrics, and all/year/month/week periods.
- All-time and period torrent leaderboards as sortable/comparable table data.
- Eviction history with cluster details, estimated reclamation and observed batch-level reclamation.

The stacked disk chart uses allocated blocks from completed torrent clusters, deduplicated by inode. `Other` is the remainder of actual `df` usage and includes incomplete torrents, unmanaged files and data not attributed to completed managed clusters. Old snapshots without category attribution are shown as Other/unclassified.

## Development and verification

- `pnpm --dir web dev` starts Vite with API proxying to Axum at `127.0.0.1:3000` by default.
- `config.dev.toml` disables the periodic worker and points at local SQLite, legacy-data and fixture paths.
- Use `scripts/import-legacy.sh /path/to/copied/server/state` to copy old state files into `data/legacy/` before starting the backend.
- Use `scripts/init-fixtures.sh` then `cargo run -- --config config.dev.toml --once --dry` to exercise the full Rust cycle against local files and real hardlinks.
- Start the dashboard-only backend with `watchexec --restart --watch src --watch Cargo.toml --watch config.dev.toml -- cargo run -- --config config.dev.toml`; use `pnpm --dir web dev` for Vite.
- Never point fixture mode at production data and never test eviction against a real qBittorrent instance.
- `pnpm --dir web typecheck` and `pnpm --dir web build` verify the frontend.
- `cargo fmt --check`, `cargo check`, `cargo test` and `cargo clippy --all` verify Rust.

The source dump used to reconstruct this working tree did not include `Cargo.lock`; generate and commit a fresh lockfile using the project's Rust toolchain before deployment.
