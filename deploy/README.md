# Deploy graine with Docker Compose

This directory is an example deployment for Linux Docker or Podman hosts, including NixOS and UGREEN NAS devices that provide a Docker/Compose environment.

## Prepare the host

Copy `compose.yaml` and `config.toml` to a persistent folder and edit the following:

- `url`: qBittorrent's WebUI address, reachable from the container.
- `disk_path`: actual filesystem containing downloads.
- Bind mount: the **paths returned by qBittorrent** must exist inside the container at the same absolute location, including hardlinked files.
- `trackers` and `managed_categories`: match your own setup.
- Password: save the WebUI password alone in `qbittorrent-password`, then `chmod 600 qbittorrent-password`.

The example uses `network_mode: host` on Linux. This allows `127.0.0.1` in `config.toml` to refer to qBittorrent on the same host. For an isolated Docker network, remove host networking, make qBittorrent reachable by the container, change `listen` to `0.0.0.0:3000`, and publish the web port only to the desired host interface (for example `127.0.0.1:3000:3000` behind a host reverse proxy).

## First start

```sh
mkdir -p data
chmod 700 data
# Edit config.toml and create qbittorrent-password before starting.
docker compose pull
docker compose up -d
docker compose logs -f graine
```

The example config disables the **scheduled worker** (`management_enabled = false`) and enables **dry-run** (`dry_run = true`). The manual run API/button remains available but cannot delete while dry-run is enabled. It will serve data in `data/graine.sqlite3` if a database is present. For a new installation with an empty database, start with a **one-off dry run** to generate a preview:

```sh
docker compose run --rm graine --config /etc/graine/config.toml --once --dry
```

After inspecting the ranking and logs, enable `management_enabled = true` while keeping `dry_run = true`. The worker runs once at startup and then every `interval_seconds` (default one hour). Only set `dry_run = false` after validating decisions. Apply configuration changes with `docker compose up -d --force-recreate`.

**Never run an old management service or timer and the new container simultaneously.** No JSONL migration is needed; SQLite is the source of truth.

## Upgrade and backups

```sh
docker compose pull
docker compose up -d
```

Persist `./data` across container upgrades. Back up SQLite using `.backup`, not a plain copy of the active database file if WAL transactions may be pending:

```sh
sqlite3 data/graine.sqlite3 ".backup '/path/to/graine-backup.sqlite3'"
```

Use a destination directory writable by the user running the command. The exported database can also be used as `data/dev.sqlite3` on a development computer.

## Safety and portability

- `POST /api/run` is **unauthenticated** and can trigger deletions. Keep the dashboard on a trusted network or behind an authenticated reverse proxy; do not expose host port 3000 publicly without access control.
- `/data/shared` is mounted **read-only** in the example because Graine reads metadata. qBittorrent still performs deletions through its API; the mount does **not** prevent them.
- Use a local Linux filesystem for correct inode and hardlink metadata. Graine supports ext4 and Btrfs for its logical accounting; Btrfs reflinks, compression and snapshots may affect actual physical space freed.
- The example image tag is `latest` and the default GitHub Actions build targets `linux/amd64`. For ARM NAS systems, build an `arm64` image or extend the workflow to publish a multi-architecture image.
