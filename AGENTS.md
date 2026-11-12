# Graine — project guide

## Purpose

Graine is a Rust command-line service that manages completed qBittorrent torrents to maximize useful private-tracker seeding while keeping filesystem usage below configured limits. It periodically ranks related torrents, identifies which deletions can actually reclaim disk space, removes the least valuable eligible clusters when the filesystem crosses its high watermark, and generates a static dashboard.

Incomplete torrents are intentionally outside Graine's ranking and deletion logic. They remain under the configured download client's / Cleanuparr workflow.

## Stack

- **Language/runtime:** Rust, built as a single CLI binary.
- **Configuration:** TOML parsed with `toml` and Serde.
- **Serialization/state:** Serde + `serde_json`; append-only JSON Lines (`.jsonl`) for activity, disk history, and deletion history.
- **qBittorrent integration:** qBittorrent Web API over HTTP using `ureq`.
- **Filesystem accounting:** Unix/Linux metadata (`dev`, inode, hardlink count, file length, allocated blocks) and `df` for actual filesystem usage.
- **Dashboard:** static HTML, CSS, and vanilla JavaScript. Rust embeds the assets at compile time with `include_str!`, injects serialized dashboard data into the HTML template, and writes the files to `/var/www/graine/` for Nginx to serve.
- **Deployment:** packaged and scheduled through the user's NixOS configuration. Keep the repository's existing `Cargo.toml`, `Cargo.lock`, and Nix packaging unless a task explicitly changes them.

## Source layout

```text
src/
  main.rs                 # CLI parsing and end-to-end orchestration
  config.rs               # TOML schema, validation, category/tracker matching
  model.rs                # Shared domain, persisted-state, and dashboard types
  util.rs                 # Time/date helpers, JSONL I/O, atomic writes, constants
  qbittorrent.rs          # qBittorrent authentication, queries, and deletion API
  storage.rs              # df, file metadata, inode clustering, reclaimable cost, score
  analytics.rs            # Persistent counters, transfer events, period aggregation
  dashboard.rs            # Dashboard row/history conversion and static asset generation
  dashboard/
    index.html
    style.css
    app.js
```

### Module responsibilities

- `main.rs` loads and validates configuration, authenticates to qBittorrent, filters managed categories, tracks analytics, builds inode-connected clusters, applies disk watermarks, performs deletions unless `--dry` is set, records history, and generates the dashboard.
- `config.rs` defines the global settings and tracker groups. It maps `.cross` categories to their base category, checks whether a torrent is managed, normalizes tracker hosts, resolves logical tracker labels/priorities, and decides whether a tracker appears in statistics.
- `model.rs` contains shared structs. Keep serialized field changes backward compatible where possible; use Serde defaults or aliases when old state files must remain readable.
- `util.rs` contains shared constants and infrastructure helpers: Unix time, week/month/year boundaries, atomic file replacement, and JSONL append/read.
- `qbittorrent.rs` is the only module that should own qBittorrent HTTP endpoint calls.
- `storage.rs` reads filesystem metadata, discovers torrent files, builds clusters based on `(device, inode)`, computes reclaimable and protected storage, scores clusters, and defines best-first ordering.
- `analytics.rs` updates lifetime counters and observed transfer deltas, persists analytics/activity state, filters unconfigured trackers from statistics, and builds leaderboard and tracker time-series data.
- `dashboard.rs` converts domain objects to dashboard records and embeds `dashboard/index.html`, `style.css`, and `app.js` into generated static files.

## Configuration and retention rules

The current configuration model has:

- `url`: qBittorrent Web UI base URL.
- `disk_path`: path on the filesystem whose usage Graine manages against.
- `high_watermark` / `low_watermark`: fractions in `[0, 1]`; `low_watermark` must be lower than `high_watermark`.
- `grace_days`: one global grace period shared by all managed categories (default 7 days).
- `managed_categories`: allowlist of qBittorrent categories Graine manages. A `<category>.cross` category is included automatically.
- `trackers`: logical tracker groups (`name`, `hosts`, `priority`). Multiple announce hosts can map to one logical tracker.
- `hide_unconfigured_trackers`: by default, unconfigured trackers remain managed but are omitted from tracker analytics/graphs and the torrent leaderboard.

There are no pools and no per-category or per-tracker grace periods. Do not reintroduce pool capacity logic unless the design explicitly changes.

Retention invariants:

1. Only complete managed torrents (`progress >= 1.0`) are inspected and ranked.
2. Torrents sharing any inode are joined into one cluster. Deletion decisions apply to the entire cluster, including cross-seeds sharing hardlinked data.
3. `cost` is logical file size that can actually be reclaimed after removing every torrent in the cluster. An inode is reclaimable when the number of distinct paths represented by the cluster is at least its hardlink count.
4. `disk_bytes` counts allocated filesystem blocks, deduplicated by inode, for chart attribution. `protected_disk_bytes` counts allocated blocks whose external hardlinks keep the data on disk after the cluster is removed.
5. Cluster category is attributed to the oldest non-cross-seed torrent's base category; if there is no original, use the oldest cluster member's base category.
6. Grace is global. A cluster remains protected while any original/anchor torrent has seeded for less than `grace_days`; clusters with no original use their members as the fallback.
7. Best-first order is: grace-protected first, then higher tracker priority, then higher score. Eviction walks that order in reverse.
8. The global filesystem watermark is the only storage limit. If `df` usage exceeds `high_watermark`, Graine evicts eligible clusters until projected usage reaches `low_watermark`. Grace clusters and clusters with `cost == 0` cannot be evicted.
9. Tracker priority affects ranking order; unconfigured tracker priority defaults to zero. Hiding unconfigured trackers is a presentation/analytics filter, not a torrent-management filter.
10. `--dry` must not persist analytics, append history/deletion events, or delete torrents. The generated dashboard may include current-run preview data.

The score currently sums each member torrent's lifetime upload-per-seeding-day estimate, applies a per-torrent inactivity factor `1 / (1 + inactive_days / 7)`, and divides by cluster reclaimable cost. A zero-cost cluster receives an infinite score because it cannot reclaim storage. Keep score semantics and priority/grace ordering changes explicit and test them with representative cluster cases.

## Persistent data

The JSON/JSONL files are relative to the process working directory:

- `analytics.json`: all-time per-tracker and per-torrent counters, last-seen counters, and tracking start time.
- `activity.jsonl`: timestamped qBittorrent upload/download counter deltas. Event IDs are used to deduplicate retries.
- `history.jsonl`: one disk-usage snapshot per live run, including category usage for the stacked disk chart.
- `deletions.jsonl`: deletion batches and per-cluster metadata; observed `df` reclamation belongs to the batch, not to an individual cluster.

Compatibility matters: old history records may lack `category_usage`; old deletion records may use the former `pool` field, accepted as an alias for `category`. Do not silently discard existing analytics/history on parse or version errors.

## Dashboard data and behavior

- Dashboard data is serialized by Rust and exposed as `window.GRAINE_DATA` in `index.html`.
- `app.js` renders all charts, metrics, ranking, leaderboard, and eviction history. Keep DOM IDs in `index.html` synchronized with selectors in `app.js`.
- The tracker graph is cumulative net upload by configured tracker group. It supports total, regular, and cross-seed net metrics, with all-time/year/month/week periods. The all-time series starts from the observed lifetime-counter baseline and then adds timestamped daily deltas; transfer dates before Graine began observing counters cannot be reconstructed.
- The stacked disk chart shows Radarr, Sonarr, Autobrr, and Other. Category values use allocated blocks from completed torrent clusters, deduplicated by inode. `Other` is the remainder of actual `df` usage and therefore includes incomplete torrents, unmanaged files, and data not attributed to completed managed clusters. Old snapshots without category attribution are shown as Other/unclassified.
- Do not re-add the obsolete pool-capacity display or the reclaimed-this-run overview metric without an explicit request.

## Development and verification

From the repository root:

```sh
cargo fmt --check
cargo clippy --all
cargo test
node --check src/dashboard/app.js
node test-dashboard.js  # if the smoke-test script is present
```

Run the checks available in the current environment and report clearly which were not run. The Rust code uses Unix-specific filesystem metadata and expects `df` to be available. Do not claim cross-platform support without changing that design.

When changing the dashboard, verify that its embedded asset paths still match the `include_str!` declarations in `dashboard.rs` and that the Nix derivation includes the entire `src/dashboard/` directory. When changing persisted structs, consider upgrades from existing JSON/JSONL files before deploying.
