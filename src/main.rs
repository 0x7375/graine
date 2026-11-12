mod analytics;
mod config;
mod dashboard;
mod model;
mod qbittorrent;
mod storage;
mod util;

use crate::{
    analytics::{build_periods, track_analytics},
    config::{Config, is_managed_category},
    dashboard::{cluster_dashboard_row, deletion_batch, generate_dashboard, history_snapshot},
    model::{AnalyticsPeriod, Cluster, DashboardData, DeletionBatch, Entry, HistorySnapshot},
    qbittorrent::{ConnectionInfo, delete_torrents, get_torrents, login},
    storage::{build_cluster, compare_clusters, disk_usage, find, torrent_files},
    util::{
        ACTIVITY_FILE, DELETIONS_FILE, GIB, HISTORY_FILE, USAGE, append_jsonl, now_unix, read_jsonl,
    },
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    process::exit,
};

fn main() {
    let mut args = env::args();
    args.next();
    let (Some(config_path), dry, None) = (args.next(), args.next(), args.next()) else {
        eprintln!("{USAGE}");
        exit(1);
    };
    let is_dry = dry.as_deref() == Some("--dry");
    if dry.is_some() && !is_dry {
        eprintln!("{USAGE}");
        exit(1);
    }

    let config_text = fs::read_to_string(&config_path).unwrap_or_else(|error| {
        eprintln!("Cannot read {config_path}: {error}");
        exit(1);
    });
    let cfg: Config = toml::from_str(&config_text).unwrap_or_else(|error| {
        eprintln!("Bad config: {error}");
        exit(1);
    });
    if let Err(error) = cfg.validate() {
        eprintln!("Bad config: {error}");
        exit(1);
    }

    let password = env::var("QBIT_PW").unwrap_or_else(|_| {
        eprintln!("Error: QBIT_PW environment variable is missing.");
        exit(1);
    });
    let conn = ConnectionInfo {
        agent: ureq::agent(),
        url: cfg.url.clone(),
    };
    login(&conn, &password);

    let managed: Vec<_> = get_torrents(&conn)
        .into_iter()
        .filter(|torrent| is_managed_category(&cfg, &torrent.category))
        .collect();
    let managed_count = managed.len();
    let (analytics, current_activity) = track_analytics(&managed, is_dry);
    let stored_activity = read_jsonl::<model::ActivityEvent>(ACTIVITY_FILE);

    // Only completed torrents are ranked. Incomplete ones remain under Cleanuparr's control.
    let complete: Vec<_> = managed
        .iter()
        .filter(|torrent| torrent.progress >= 1.0)
        .cloned()
        .collect();
    let complete_count = complete.len();
    let mut entries = Vec::new();
    for torrent in complete {
        if let Some(files) = torrent_files(&conn, &torrent) {
            entries.push(Entry { torrent, files });
        }
    }

    // Union-find merges chains such as original -> cross-seed A -> cross-seed B.
    let mut parent: Vec<usize> = (0..entries.len()).collect();
    let mut owner: HashMap<(u64, u64), usize> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        for file in &entry.files {
            let key = (file.dev, file.ino);
            if let Some(other) = owner.get(&key).copied() {
                let a = find(&mut parent, index);
                let b = find(&mut parent, other);
                parent[a] = b;
            } else {
                owner.insert(key, index);
            }
        }
    }

    let mut by_root: HashMap<usize, Vec<Entry>> = HashMap::new();
    for (index, entry) in entries.into_iter().enumerate() {
        let root = find(&mut parent, index);
        by_root.entry(root).or_default().push(entry);
    }

    let mut clusters: Vec<Cluster> = by_root
        .into_values()
        .map(|group| build_cluster(&cfg, group))
        .collect();
    clusters.sort_by(compare_clusters);
    for (index, cluster) in clusters.iter_mut().enumerate() {
        cluster.rank = index + 1;
    }

    // The global disk watermark is the only storage limit. Grace-protected clusters and
    // clusters that cannot reclaim space are skipped; eligible clusters are evicted worst first.
    let (used_before, total) = disk_usage(&cfg.disk_path);
    let high = (cfg.high_watermark * total as f64) as u64;
    let low = (cfg.low_watermark * total as f64) as u64;
    let mut projected = used_before;
    let mut evict = HashSet::new();

    if projected > high {
        for index in (0..clusters.len()).rev() {
            if projected <= low {
                break;
            }

            let cluster = &clusters[index];
            if cluster.is_grace || cluster.cost == 0 {
                continue;
            }

            projected = projected.saturating_sub(cluster.cost);
            evict.insert(index);
        }
    }

    let estimated_reclaimed = used_before.saturating_sub(projected);
    let over_limit = projected > high;
    let mut final_kept: Vec<(&str, Cluster)> = Vec::new();
    let mut dropped: Vec<Cluster> = Vec::new();

    for (index, cluster) in clusters.into_iter().enumerate() {
        if evict.contains(&index) {
            dropped.push(cluster);
            continue;
        }

        let tag = if cluster.cost == 0 {
            "(FREE)"
        } else if cluster.is_grace {
            "(GRACE)"
        } else {
            "(GOOD)"
        };
        final_kept.push((tag, cluster));
    }

    let timestamp = now_unix();
    let mut ranking: Vec<_> = final_kept
        .iter()
        .map(|(tag, cluster)| cluster_dashboard_row(&cfg, tag, cluster, is_dry))
        .chain(
            dropped
                .iter()
                .map(|cluster| cluster_dashboard_row(&cfg, "(DEL-DISK)", cluster, is_dry)),
        )
        .collect();
    ranking.sort_by_key(|cluster| cluster.rank);

    // Summarize retained, completed torrent clusters. Shared inodes are counted once per cluster.
    let mut reclaimable_bytes = 0_u64;
    let mut protected_bytes = 0_u64;
    let mut category_usage = BTreeMap::<String, u64>::new();
    for (_, cluster) in &final_kept {
        reclaimable_bytes = reclaimable_bytes.saturating_add(cluster.cost);
        protected_bytes =
            protected_bytes.saturating_add(cluster.data_bytes.saturating_sub(cluster.cost));
        let category_bytes = category_usage.entry(cluster.category.clone()).or_default();
        *category_bytes = category_bytes.saturating_add(cluster.disk_bytes);
    }
    for cluster in &dropped {
        if is_dry {
            // Nothing is removed in a dry run, so all of its allocated blocks remain present.
            reclaimable_bytes = reclaimable_bytes.saturating_add(cluster.cost);
            protected_bytes =
                protected_bytes.saturating_add(cluster.data_bytes.saturating_sub(cluster.cost));
        }
        let category_bytes = category_usage.entry(cluster.category.clone()).or_default();
        let remaining_disk_bytes = if is_dry {
            cluster.disk_bytes
        } else {
            // Hardlinked library files survive torrent deletion and remain part of disk usage.
            cluster.protected_disk_bytes
        };
        *category_bytes = category_bytes.saturating_add(remaining_disk_bytes);
    }

    println!(
        "graine: {managed_count} managed torrents, {complete_count} complete, {} clusters",
        ranking.len()
    );
    println!(
        "disk: {:.1}% used; {:.1}% projected after decisions (high {:.0}%, low {:.0}%)",
        used_before as f64 / total as f64 * 100.0,
        projected as f64 / total as f64 * 100.0,
        cfg.high_watermark * 100.0,
        cfg.low_watermark * 100.0
    );
    if over_limit {
        eprintln!("WARNING: still above the high watermark; no eligible clusters remain");
    }

    let mut actual_reclaimed = None;
    let mut disk_after = (used_before, total);
    let mut deletions: Vec<DeletionBatch> = read_jsonl(DELETIONS_FILE);
    if !dropped.is_empty() && !is_dry {
        let to_delete: Vec<_> = dropped
            .iter()
            .flat_map(|cluster| cluster.torrents.iter().cloned())
            .collect();
        delete_torrents(&conn, &to_delete);
        disk_after = disk_usage(&cfg.disk_path);
        actual_reclaimed = Some(used_before.saturating_sub(disk_after.0));
        let batch = deletion_batch(&cfg, &dropped, timestamp, actual_reclaimed);
        append_jsonl(DELETIONS_FILE, std::slice::from_ref(&batch));
        deletions.push(batch);
        println!(
            "Deleted {} torrents; estimated {:.2} GiB, observed {:.2} GiB reclaimed.",
            to_delete.len(),
            estimated_reclaimed as f64 / GIB as f64,
            actual_reclaimed.unwrap_or(0) as f64 / GIB as f64
        );
    } else if !dropped.is_empty() {
        println!(
            "Dry run: {} torrents would be deleted.",
            dropped
                .iter()
                .map(|cluster| cluster.torrents.len())
                .sum::<usize>()
        );
    } else {
        println!("Nothing to delete");
        if !is_dry {
            actual_reclaimed = Some(0);
        }
    }

    let (used_after, total_after) = disk_after;
    let snapshot = history_snapshot(
        timestamp,
        used_after,
        total_after,
        projected,
        estimated_reclaimed,
        actual_reclaimed,
        reclaimable_bytes,
        protected_bytes,
        category_usage,
        cfg.high_watermark,
        cfg.low_watermark,
    );

    let mut history: Vec<HistorySnapshot> = read_jsonl(HISTORY_FILE);
    if !is_dry {
        append_jsonl(HISTORY_FILE, std::slice::from_ref(&snapshot));
    }
    history.push(snapshot);
    // Retain the complete history on disk, but keep the generated HTML a reasonable size.
    if history.len() > 1500 {
        let excess = history.len() - 1500;
        history.drain(0..excess);
    }
    if deletions.len() > 250 {
        let excess = deletions.len() - 250;
        deletions.drain(0..excess);
    }

    let periods = build_periods(
        &analytics,
        &stored_activity,
        &current_activity,
        timestamp,
        &cfg,
    );
    let mut periods = periods;
    for key in ["all", "year", "month", "week"] {
        periods.entry(key.to_string()).or_insert(AnalyticsPeriod {
            torrents: Vec::new(),
            tracker_history: Vec::new(),
        });
    }

    let run = model::RunSummary {
        timestamp,
        mode: if is_dry { "dry" } else { "live" }.to_string(),
        result: if over_limit { "over_limit" } else { "ok" }.to_string(),
        disk_used_bytes: used_after,
        disk_total_bytes: total_after,
        disk_used_percent: if total_after == 0 {
            0.0
        } else {
            used_after as f64 / total_after as f64 * 100.0
        },
        projected_used_bytes: projected,
        projected_used_percent: if total_after == 0 {
            0.0
        } else {
            projected as f64 / total_after as f64 * 100.0
        },
        high_watermark_percent: cfg.high_watermark * 100.0,
        low_watermark_percent: cfg.low_watermark * 100.0,
        estimated_reclaimed_bytes: estimated_reclaimed,
        actual_reclaimed_bytes: actual_reclaimed,
        managed_torrents: managed_count,
        complete_torrents: complete_count,
        clusters: ranking.len(),
    };

    let dashboard = DashboardData {
        generated_at: timestamp,
        run,
        history,
        reclaimable_bytes,
        protected_bytes,
        ranking,
        periods,
        deletions,
    };
    generate_dashboard(&dashboard);
    println!("Dashboard updated: {}", util::DASHBOARD_FILE);

    if over_limit {
        exit(2);
    }
}
