use crate::{
    config::{Config, base_category, tracker_priority},
    model::{Cluster, Entry, FileRef, Torrent},
    qbittorrent::{ConnectionInfo, get_torrent_files},
    util::DAY_IN_SEC,
};
use anyhow::{Context, Result, bail};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Command,
};

/// `(used, total)` bytes of the filesystem holding `path`, like `df` reports them.
pub(crate) fn disk_usage(path: &str) -> Result<(u64, u64)> {
    let output = Command::new("df")
        .args(["-B1", "--output=used,avail", path])
        .output()
        .context("cannot run df")?;

    if !output.status.success() {
        bail!(
            "df failed for {path}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .nth(1)
        .and_then(|line| {
            let mut values = line.split_whitespace().map(|value| value.parse::<u64>());
            match (values.next(), values.next()) {
                (Some(Ok(used)), Some(Ok(available))) => Some((used, used + available)),
                _ => None,
            }
        })
        .with_context(|| format!("cannot parse df output for {path}"))
}

/// Files of a torrent with their on-disk identity. An API failure aborts the run
/// (nothing gets deleted); a torrent whose files cannot be read is skipped.
pub(crate) fn torrent_files(
    conn: &ConnectionInfo,
    torrent: &Torrent,
) -> Result<Option<Vec<FileRef>>> {
    let files = get_torrent_files(conn, &torrent.hash)?;
    if files.is_empty() {
        eprintln!("WARN: no file list for {}, skipping", torrent.name);
        return Ok(None);
    }

    let mut output = Vec::with_capacity(files.len());
    for file in files {
        let path = Path::new(&torrent.save_path).join(&file.name);
        match fs::metadata(&path) {
            Ok(metadata) => output.push(FileRef {
                dev: metadata.dev(),
                ino: metadata.ino(),
                nlink: metadata.nlink(),
                size: metadata.len(),
                allocated_bytes: metadata.blocks().saturating_mul(512),
                path,
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                eprintln!(
                    "WARN: cannot stat {}: {error}, skipping {}",
                    path.display(),
                    torrent.name
                );
                return Ok(None);
            }
        }
    }

    if output.is_empty() {
        eprintln!("WARN: no file of {} exists on disk, skipping", torrent.name);
        return Ok(None);
    }

    Ok(Some(output))
}

/// Path-compressed union-find lookup.
pub(crate) fn find(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

pub(crate) fn build_cluster(cfg: &Config, entries: Vec<Entry>) -> Cluster {
    // inode -> (hardlink count, logical size, allocated bytes, distinct cluster paths)
    let mut inodes: HashMap<(u64, u64), (u64, u64, u64, HashSet<PathBuf>)> = HashMap::new();
    for entry in &entries {
        for file in &entry.files {
            inodes
                .entry((file.dev, file.ino))
                .or_insert((file.nlink, file.size, file.allocated_bytes, HashSet::new()))
                .3
                .insert(file.path.clone());
        }
    }

    let data_bytes = inodes
        .values()
        .map(|(_, size, _, _)| *size)
        .fold(0_u64, u64::saturating_add);
    let cost = inodes
        .values()
        .filter(|(nlink, _, _, paths)| paths.len() as u64 >= *nlink)
        .map(|(_, size, _, _)| *size)
        .fold(0_u64, u64::saturating_add);
    let disk_bytes = inodes
        .values()
        .map(|(_, _, allocated_bytes, _)| *allocated_bytes)
        .fold(0_u64, u64::saturating_add);
    let protected_disk_bytes = inodes
        .values()
        .filter(|(nlink, _, _, paths)| (paths.len() as u64) < *nlink)
        .map(|(_, _, allocated_bytes, _)| *allocated_bytes)
        .fold(0_u64, u64::saturating_add);

    let uploaded = entries
        .iter()
        .map(|entry| entry.torrent.uploaded)
        .fold(0_u64, u64::saturating_add);
    let seeding_time = entries
        .iter()
        .map(|entry| entry.torrent.seeding_time)
        .max()
        .unwrap_or(0);
    let days = seeding_time as f32 / DAY_IN_SEC as f32;

    let anchors: Vec<&Entry> = entries
        .iter()
        .filter(|entry| !entry.torrent.is_cross())
        .collect();
    let category_entry = anchors
        .iter()
        .copied()
        .min_by_key(|entry| entry.torrent.added_on)
        .or_else(|| entries.iter().min_by_key(|entry| entry.torrent.added_on))
        .expect("a cluster cannot be empty");
    let category = base_category(&category_entry.torrent.category).to_string();

    let grace_seconds = cfg.grace_days.saturating_mul(DAY_IN_SEC);
    let is_grace = if anchors.is_empty() {
        entries
            .iter()
            .any(|entry| entry.torrent.seeding_time < grace_seconds)
    } else {
        anchors
            .iter()
            .any(|entry| entry.torrent.seeding_time < grace_seconds)
    };

    let now = crate::util::now_unix();
    let upload_rate: f64 = entries
        .iter()
        .map(|entry| {
            let seeding_days = (entry.torrent.seeding_time as f64 / DAY_IN_SEC as f64).max(1.0);
            let inactive_days =
                now.saturating_sub(entry.torrent.last_activity) as f64 / DAY_IN_SEC as f64;
            let activity_factor = 1.0 / (1.0 + inactive_days / 7.0);
            (entry.torrent.uploaded as f64 / seeding_days) * activity_factor
        })
        .sum();

    let score = if cost == 0 {
        f64::INFINITY
    } else {
        upload_rate / cost as f64
    };
    let priority = entries
        .iter()
        .map(|entry| tracker_priority(cfg, &entry.torrent.tracker))
        .max()
        .unwrap_or(0);

    Cluster {
        torrents: entries.into_iter().map(|entry| entry.torrent).collect(),
        category,
        priority,
        cost,
        data_bytes,
        disk_bytes,
        protected_disk_bytes,
        uploaded,
        days,
        is_grace,
        score,
        rank: 0,
    }
}

/// Best first: grace, then higher tracker priority, then higher score.
pub(crate) fn compare_clusters(a: &Cluster, b: &Cluster) -> Ordering {
    b.is_grace
        .cmp(&a.is_grace)
        .then(b.priority.cmp(&a.priority))
        .then(b.score.total_cmp(&a.score))
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, model::FileRef};

    fn config() -> Config {
        toml::from_str(
            r#"
            url = "http://localhost:8080"
            disk_path = "/data/shared"
            managed_categories = ["radarr", "sonarr"]
            grace_days = 7

            [[trackers]]
            name = "Low"
            hosts = ["low.example"]
            priority = 2

            [[trackers]]
            name = "High"
            hosts = ["high.example"]
            priority = 10
            "#,
        )
        .expect("test config should deserialize")
    }

    fn entry(
        hash: &str,
        category: &str,
        added_on: u64,
        seeding_time: u64,
        uploaded: u64,
        tracker: &str,
        inode: u64,
        hardlinks: u64,
        path: &str,
    ) -> Entry {
        Entry {
            torrent: Torrent {
                hash: hash.to_string(),
                name: hash.to_string(),
                category: category.to_string(),
                save_path: "/data/shared".to_string(),
                progress: 1.0,
                seeding_time,
                added_on,
                last_activity: crate::util::now_unix(),
                uploaded,
                downloaded: 0,
                tracker: format!("https://{tracker}/announce"),
            },
            files: vec![FileRef {
                dev: 1,
                ino: inode,
                nlink: hardlinks,
                size: 100,
                allocated_bytes: 80,
                path: PathBuf::from(path),
            }],
        }
    }

    #[test]
    fn cluster_keeps_existing_cost_category_grace_priority_and_score_semantics() {
        let cfg = config();
        let cluster = build_cluster(
            &cfg,
            vec![
                entry("radarr", "radarr", 10, 10 * DAY_IN_SEC, 1_000, "low.example", 7, 2, "/data/shared/a"),
                entry("sonarr", "sonarr", 20, 2 * DAY_IN_SEC, 2_000, "high.example", 7, 2, "/data/shared/b"),
            ],
        );

        assert_eq!(cluster.category, "radarr");
        assert!(cluster.is_grace);
        assert_eq!(cluster.priority, 10);
        assert_eq!(cluster.cost, 100);
        assert_eq!(cluster.data_bytes, 100);
        assert_eq!(cluster.disk_bytes, 80);
        assert_eq!(cluster.protected_disk_bytes, 0);
        assert_eq!(cluster.uploaded, 3_000);
        assert!((cluster.score - 11.0).abs() < 0.01);
    }

    #[test]
    fn externally_hardlinked_cluster_has_zero_reclaimable_cost() {
        let cluster = build_cluster(
            &config(),
            vec![
                entry("a", "radarr", 10, 10 * DAY_IN_SEC, 1_000, "low.example", 7, 3, "/data/shared/a"),
                entry("b", "radarr.cross", 20, 10 * DAY_IN_SEC, 2_000, "low.example", 7, 3, "/data/shared/b"),
            ],
        );

        assert_eq!(cluster.cost, 0);
        assert_eq!(cluster.protected_disk_bytes, 80);
        assert!(cluster.score.is_infinite());
    }
}
