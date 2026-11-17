# graine — contributor guidance

## Overview

Graine is a Rust service with an Axum API and React dashboard. Rust is the source of truth for qBittorrent management, ranking, filesystem metadata and SQLite persistence. The frontend renders Rust-provided data and does not implement domain calculations.

## Source layout

- `src/config.rs`: TOML schema, validation, tracker/category matching.
- `src/model.rs`: domain and serialized API types; maintain compatibility with stored SQLite JSON.
- `src/qbittorrent.rs`: HTTP integration and read-only local fixture mode.
- `src/storage.rs`: filesystem metadata, hardlink clustering and score.
- `src/analytics.rs`: persisted counters and time-period statistics.
- `src/engine.rs`: one management cycle, grace, ranking and eviction decisions.
- `src/db.rs`: SQLite schema, run records and history pagination.
- `src/dashboard.rs`: Rust-domain-to-dashboard serialization.
- `src/server.rs`: Axum endpoints, worker scheduling and frontend serving.
- `web/src/`: React dashboard, translation and design system.
- `deploy/`: portable Docker Compose example.

## Retention rules

1. Manage only completed torrents in configured categories (including `.cross` variants).
2. Group all torrents sharing inodes; deletion is **per cluster** and must account for cross-seeds sharing hardlinks.
3. Cost is the **logical size** of uniquely reclaimable inodes, excluding data with external hardlinks. Never count the same inode multiple times. `df` is used separately to observe filesystem usage and watermarks, not to attribute a per-torrent deletion size.
4. Grace protection applies to originals; use all members only if no original exists. The least-seeded anchor determines the remaining grace period.
5. Best-first order: grace, then priority, then score. Eviction proceeds in reverse order, skipping grace and zero-cost clusters.
6. The score sums each torrent's uploaded bytes / max(seeding days, 1) weighted by `1/(1+inactive_days/7)` and divides by cluster cost. Preserve this unless a feature explicitly requires change.
7. `dry_run` and fixture mode must never issue deletion commands or persist analytics and activity history. Fixture files must stay within its local disk root.
8. Scheduled and manually triggered runs share a single execution guard; the default interval is 3600 seconds.

## Data and UI

- SQLite owns analytics, events, disk history, deletion batches, management runs, period baselines and the latest dashboard. Do **not** re-add JSON/JSONL import logic.
- Historical SQLite records may lack newer optional fields. Preserve deserialization aliases/defaults for compatible versions.
- Keep user-visible copy minimal. The four views are Overview, Storage, Leaderboard, History. Ranking is a cluster table; Leaderboard is a per-torrent table capped at 100 rows; History is a paginated list of runs with server-side `all/deletions/errors` filters.
- Grace status in Ranking must use the engine's actual grace decision. Details should show individual members and cross-seeds but never imply separate reclaimable sizes for hardlinked copies.
- UI follows the browser's French/English language, with dates in day/month/year format and a muted green light/dark theme. Do not introduce unrelated new explanatory copy or change the style system without request.

## Development / verification

- `nix develop` provides Rust, pnpm, Node.js, SQLite and Watchexec; `make backend-dev` and `make frontend-dev` start the separate local dev servers.
- `config.dev.toml` disables scheduled work and points at local data. `make fixture-run` is dry and uses real local hardlinks.
- `make check` runs Rust format, check, tests, Clippy, and TypeScript checks; `make build` builds Vite and the release binary.
- Avoid production qBittorrent tests. New sqlite migrations must be additive and non-destructive. The source lockfiles (`Cargo.lock`, `web/pnpm-lock.yaml`) must be retained in Git.
- `POST /api/run` can delete torrents and has no authentication. Documentation must require a trusted network or access control at the reverse proxy.
