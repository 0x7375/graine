# Graine: generic Docker deployment (Linux / NixOS / UGREEN OS)

## Build and publishing

Commit `Dockerfile`, `.dockerignore`, `.github/workflows/docker.yml`, `Cargo.lock` and
`web/pnpm-lock.yaml` in the GitHub repository. Push to `main`. GitHub Actions publishes
`ghcr.io/<owner>/<repo>:latest`. The two lockfiles are required for reproducible builds.
If missing locally, run `cargo generate-lockfile` and `cd web && pnpm install` first.

## First deployment

Copy this `deploy` directory to the machine running qBittorrent. Replace the image name
in `compose.yaml`, if needed. Adjust `config.toml`: especially qBittorrent URL, trackers,
and disk_path. For qBittorrent on the same Linux machine, `network_mode: host` means
`127.0.0.1` refers to the host. For remote qBittorrent, use its reachable address.

This example assumes the *same absolute file paths* inside and outside the container,
`/data/shared`. If qBittorrent saves elsewhere on UGREEN OS, adapt both sides of
that volume mapping and `disk_path`. Paths from qBittorrent's API must remain valid
inside the container. The bind is intentionally read-only: Graine only stats files;
qBittorrent deletes its own data via its API. **Read-only does not prevent an API
request from deleting torrents.**

Run from the deploy directory:

```sh
mkdir -p data/legacy
chmod 700 data
printf '%s\n' 'YOUR_QBITTORRENT_WEBUI_PASSWORD' > qbittorrent-password
chmod 600 qbittorrent-password
# Stop the OLD timer/service before the final data copy and never run both workers.
# Copy analytics.json, activity.jsonl, history.jsonl and deletions.jsonl
# from the server to data/legacy/ (not into Git).
docker compose pull
docker compose up -d
docker compose logs -f graine
curl -f http://127.0.0.1:3000/api/health
```

The first start imports legacy data to SQLite and serves the dashboard, **without
running the management worker** (`management_enabled = false`). The original JSONL
files are retained. The server's clock and the filesystem's accounting are unchanged.

## Controlled testing, then enable the worker

```sh
# Preview a single run (no deletion; fixture mode must NOT be enabled in production):
docker compose run --rm graine --config /etc/graine/config.toml --once --dry
```

Review logs; note that `--dry` does not persist new snapshots. Afterwards, change
`management_enabled = true` but keep `dry_run = true`, then restart and inspect the
new logs. Finally set `dry_run = false` and restart when ready. The worker runs once
at startup, then every `interval_seconds` (default 3600 seconds).

```sh
docker compose up -d --force-recreate
```

For upgrades:

```sh
docker compose pull && docker compose up -d
```

## Important caveats

- `network_mode: host` works on Linux Docker hosts, including typical NixOS/UGREEN OS
  setups; it is not universally equivalent on Docker Desktop. The HTTP service binds
  to all host interfaces on port 3000; apply firewall rules or a reverse proxy with
  authentication rather than exposing it publicly. For access only through a proxy,
  consider binding the app to `127.0.0.1:3000` instead.
- Keep `data/`, `qbittorrent-password`, and config files with secrets out of Git.
- SQLite and its WAL/SHM files must live on a persistent local filesystem. Back up
  using the SQLite backup API or `sqlite3 ... '.backup ...'`, not by copying the
  live DB file alone.
- Ensure `df` and file metadata in the container refer to the same underlying
  physical filesystem as qBittorrent. Never substitute SSHFS for this purpose.
- On hardware other than x86_64, configure multi-architecture GitHub Actions builds.
  The included workflow builds linux/amd64 on the default GitHub runner.
