use crate::{
    config::{Config, base_category, tracker_priority},
    model::{Cluster, Entry, FileRef, Torrent},
    qbittorrent::{ConnectionInfo, get_torrent_files},
    util::DAY_IN_SEC,
};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, exit},
};

/// `(used, total)` bytes of the filesystem holding `path`, like `df` reports them.
pub(crate) fn disk_usage(path: &str) -> (u64, u64) {
    let output = Command::new("df")
        .args(["-B1", "--output=used,avail", path])
        .output()
        .unwrap_or_else(|error| {
            eprintln!("Cannot run df: {error}");
            exit(1);
        });

    if !output.status.success() {
        eprintln!(
            "df failed for {path}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        exit(1);
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
        .unwrap_or_else(|| {
            eprintln!("Cannot parse df output for {path}");
            exit(1);
        })
}

/// Files of a torrent with their on-disk identity. An API failure aborts the run
/// (nothing gets deleted); a torrent whose files cannot be read is skipped.
pub(crate) fn torrent_files(conn: &ConnectionInfo, torrent: &Torrent) -> Option<Vec<FileRef>> {
    let files = get_torrent_files(conn, &torrent.hash);
    if files.is_empty() {
        eprintln!("WARN: no file list for {}, skipping", torrent.name);
        return None;
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
                return None;
            }
        }
    }

    if output.is_empty() {
        eprintln!("WARN: no file of {} exists on disk, skipping", torrent.name);
        return None;
    }

    Some(output)
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
