use crate::{
    analytics::{build_periods, track_analytics},
    config::{Config, is_managed_category},
    dashboard::{cluster_dashboard_row, deletion_batch, history_snapshot},
    db::Database,
    model::{
        AnalyticsPeriod, Cluster, DashboardData, DeletionBatch, Entry, HistorySnapshot, RunSummary,
    },
    qbittorrent::{ConnectionInfo, delete_torrents, get_torrents, login},
    storage::{build_cluster, compare_clusters, disk_usage, find, torrent_files},
    util::{GIB, now_unix},
};
use anyhow::{Context, Result};
use std::{collections::{BTreeMap, HashMap, HashSet}, path::Path};

/// Positions compare each period's leaderboard with the previous execution's
/// leaderboard for that same period. No baseline => no movement indicator.
fn rank_changes_from_previous(
    database: &Database,
    periods: &HashMap<String, AnalyticsPeriod>,
) -> Result<HashMap<String, HashMap<String, i32>>> {
    let Some(previous_json) = database.dashboard_json()? else {
        return Ok(HashMap::new());
    };
    let previous: serde_json::Value = serde_json::from_str(&previous_json)?;
    let Some(previous_periods) = previous.get("periods") else {
        return Ok(HashMap::new());
    };

    let mut changes = HashMap::new();
    for (period, current) in periods {
        let Some(previous_torrents) = previous_periods
            .get(period)
            .and_then(|entry| entry.get("torrents"))
            .and_then(|entries| entries.as_array()) else { continue };

        let mut old: Vec<(String, i128)> = previous_torrents.iter().filter_map(|entry| {
            Some((
                entry.get("hash")?.as_str()?.to_owned(),
                entry.get("uploaded_bytes")?.as_u64()? as i128
                    - entry.get("downloaded_bytes")?.as_u64()? as i128,
            ))
        }).collect();
        old.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let old_ranks: HashMap<&str, i32> = old.iter().enumerate()
            .map(|(index, (hash, _))| (hash.as_str(), (index + 1) as i32)).collect();

        let mut current_sorted: Vec<_> = current.torrents.iter().collect();
        current_sorted.sort_by(|a, b| {
            let net_a = a.uploaded_bytes as i128 - a.downloaded_bytes as i128;
            let net_b = b.uploaded_bytes as i128 - b.downloaded_bytes as i128;
            net_b.cmp(&net_a).then_with(|| a.hash.cmp(&b.hash))
        });
        let mut period_changes = HashMap::new();
        for (index, torrent) in current_sorted.iter().enumerate() {
            if let Some(&old_rank) = old_ranks.get(torrent.hash.as_str()) {
                period_changes.insert(torrent.hash.clone(), old_rank - (index + 1) as i32);
            }
        }
        changes.insert(period.clone(), period_changes);
    }
    Ok(changes)
}

pub(crate) fn run_management_cycle(
    cfg: &Config,
    database: &Database,
    password: Option<&str>,
    force_dry: bool,
) -> Result<DashboardData> {
    let conn = ConnectionInfo::new(
        &cfg.url,
        cfg.fixture_path.as_deref(),
        Path::new(&cfg.disk_path),
    )?;
    // A fixture is always read-only, even when a caller forgets to request --dry.
    let is_dry = cfg.dry_run || force_dry || conn.is_fixture();
    if !conn.is_fixture() {
        let password = password.context("qBittorrent password is required outside fixture mode")?;
        login(&conn, password)?;
    }

    let managed: Vec<_> = get_torrents(&conn)?
        .into_iter()
        .filter(|torrent| is_managed_category(cfg, &torrent.category))
        .collect();
    let managed_count = managed.len();
    let analytics_state = database.load_analytics()?;
    let (analytics, current_activity) = track_analytics(&managed, analytics_state);
    // Match the old JSONL persistence point: analytics and transfer deltas are committed before
    // filesystem accounting or deletion can fail. Dry runs remain entirely non-persistent.
    database.persist_analytics(&analytics, &current_activity, is_dry)?;
    let stored_activity = database.load_activity()?;

    // Only completed torrents are ranked. Incomplete ones remain under Cleanuparr's control.
    let complete: Vec<_> = managed
        .iter()
        .filter(|torrent| torrent.progress >= 1.0)
        .cloned()
        .collect();
    let complete_count = complete.len();
    let mut entries = Vec::new();
    for torrent in complete {
        if let Some(files) = torrent_files(&conn, &torrent)? {
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
        .map(|group| build_cluster(cfg, group))
        .collect();
    clusters.sort_by(compare_clusters);
    for (index, cluster) in clusters.iter_mut().enumerate() {
        cluster.rank = index + 1;
    }

    // The global disk watermark is the only storage limit. Grace-protected clusters and
    // clusters that cannot reclaim space are skipped; eligible clusters are evicted worst first.
    let (used_before, total) = disk_usage(&cfg.disk_path)?;
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
        .map(|(tag, cluster)| cluster_dashboard_row(cfg, tag, cluster, is_dry))
        .chain(
            dropped
                .iter()
                .map(|cluster| cluster_dashboard_row(cfg, "(DEL-DISK)", cluster, is_dry)),
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

    tracing::info!(
        managed_torrents = managed_count,
        complete_torrents = complete_count,
        clusters = ranking.len(),
        disk_used_percent = used_before as f64 / total as f64 * 100.0,
        projected_used_percent = projected as f64 / total as f64 * 100.0,
        "management cycle calculated"
    );
    if over_limit {
        tracing::warn!("filesystem remains above the high watermark; no eligible clusters remain");
    }

    let mut actual_reclaimed = None;
    let mut disk_after = (used_before, total);
    let mut deletions: Vec<DeletionBatch> = database.load_deletions()?;
    let mut new_deletion_batch = None;
    if !dropped.is_empty() && !is_dry {
        let to_delete: Vec<_> = dropped
            .iter()
            .flat_map(|cluster| cluster.torrents.iter().cloned())
            .collect();
        delete_torrents(&conn, &to_delete)?;
        disk_after = disk_usage(&cfg.disk_path)?;
        actual_reclaimed = Some(used_before.saturating_sub(disk_after.0));
        let batch = deletion_batch(cfg, &dropped, timestamp, actual_reclaimed);
        deletions.push(batch.clone());
        new_deletion_batch = Some(batch);
        tracing::info!(
            deleted_torrents = to_delete.len(),
            estimated_reclaimed_gib = estimated_reclaimed as f64 / GIB as f64,
            observed_reclaimed_gib = actual_reclaimed.unwrap_or(0) as f64 / GIB as f64,
            "torrent eviction completed"
        );
    } else if !dropped.is_empty() {
        tracing::info!(
            would_delete_torrents = dropped.iter().map(|cluster| cluster.torrents.len()).sum::<usize>(),
            "dry run: torrents would be deleted"
        );
    } else {
        tracing::info!("no torrents need eviction");
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

    let mut history: Vec<HistorySnapshot> = database.load_history()?;
    // Both live runs and dry previews include the current snapshot in the response. The DB
    // layer decides whether it is persisted; dry previews only replace latest_dashboard.
    history.push(snapshot.clone());
    // Keep the complete history in SQLite; limit only the dashboard response size.
    if history.len() > 1500 {
        let excess = history.len() - 1500;
        history.drain(0..excess);
    }
    if deletions.len() > 250 {
        let excess = deletions.len() - 250;
        deletions.drain(0..excess);
    }

    let mut periods = build_periods(
        &analytics,
        &stored_activity,
        &current_activity,
        timestamp,
        cfg,
    );
    for key in ["all", "year", "month", "week"] {
        periods.entry(key.to_string()).or_insert(AnalyticsPeriod {
            torrents: Vec::new(),
            tracker_history: Vec::new(),
        });
    }

    let run = RunSummary {
        timestamp,
        mode: if conn.is_fixture() { "fixture" } else if is_dry { "dry" } else { "live" }.to_string(),
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

    let mut dashboard = DashboardData {
        generated_at: timestamp,
        run,
        history,
        reclaimable_bytes,
        protected_bytes,
        ranking,
        periods,
        rank_changes: HashMap::new(),
        deletions,
    };
    dashboard.rank_changes = rank_changes_from_previous(database, &dashboard.periods)?;

    database
        .save_run(
            &snapshot,
            new_deletion_batch.as_ref(),
            &dashboard,
            is_dry,
        )
        .context("cannot save management results to SQLite")?;

    Ok(dashboard)
}


/// Create a read-only dashboard from imported JSON/JSONL data before any management scan.
/// Legacy exports contain analytics and history, but not the latest ranking, so ranking remains empty.
pub(crate) fn seed_dashboard_from_legacy(cfg: &Config, database: &Database) -> Result<()> {
    if database.dashboard_json()?.is_some() {
        return Ok(());
    }

    let analytics = database.load_analytics()?;
    let activity = database.load_activity()?;
    let mut history = database.load_history()?;
    let mut deletions = database.load_deletions()?;
    if analytics.tracking_started_at == 0
        && analytics.trackers.is_empty()
        && analytics.torrents.is_empty()
        && activity.is_empty()
        && history.is_empty()
        && deletions.is_empty()
    {
        return Ok(());
    }

    let timestamp = history
        .last()
        .map(|snapshot| snapshot.timestamp)
        .or_else(|| activity.iter().map(|event| event.timestamp).max())
        .or_else(|| deletions.iter().map(|batch| batch.timestamp).max())
        .unwrap_or_else(now_unix);

    if history.len() > 1500 {
        history.drain(0..history.len() - 1500);
    }
    if deletions.len() > 250 {
        deletions.drain(0..deletions.len() - 250);
    }

    let mut periods = build_periods(&analytics, &activity, &[], timestamp, cfg);
    for key in ["all", "year", "month", "week"] {
        periods.entry(key.to_string()).or_insert(AnalyticsPeriod {
            torrents: Vec::new(),
            tracker_history: Vec::new(),
        });
    }

    let latest = history.last();
    let run = RunSummary {
        timestamp,
        mode: "historical".to_string(),
        result: "imported".to_string(),
        disk_used_bytes: latest.map_or(0, |snapshot| snapshot.disk_used_bytes),
        disk_total_bytes: latest.map_or(0, |snapshot| snapshot.disk_total_bytes),
        disk_used_percent: latest.map_or(0.0, |snapshot| snapshot.disk_used_percent),
        projected_used_bytes: latest.map_or(0, |snapshot| snapshot.projected_used_bytes),
        projected_used_percent: latest.map_or(0.0, |snapshot| snapshot.projected_used_percent),
        high_watermark_percent: latest.map_or(cfg.high_watermark * 100.0, |snapshot| snapshot.high_watermark_percent),
        low_watermark_percent: latest.map_or(cfg.low_watermark * 100.0, |snapshot| snapshot.low_watermark_percent),
        estimated_reclaimed_bytes: latest.map_or(0, |snapshot| snapshot.estimated_reclaimed_bytes),
        actual_reclaimed_bytes: latest.and_then(|snapshot| snapshot.actual_reclaimed_bytes),
        // The old history format does not record live torrent counts or current ranking.
        managed_torrents: 0,
        complete_torrents: 0,
        clusters: 0,
    };
    let dashboard = DashboardData {
        generated_at: timestamp,
        run,
        reclaimable_bytes: latest.map_or(0, |snapshot| snapshot.reclaimable_bytes),
        protected_bytes: latest.map_or(0, |snapshot| snapshot.protected_bytes),
        history,
        ranking: Vec::new(),
        periods,
        rank_changes: HashMap::new(),
        deletions,
    };
    database.save_dashboard(&dashboard).context("cannot create dashboard from imported legacy history")?;
    tracing::info!(timestamp, "seeded dashboard from imported legacy state; current ranking awaits a management run");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn legacy_history_seeds_dashboard_without_inventing_a_ranking() {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("graine-legacy-dashboard-test-{nonce}"));
        let legacy = root.join("legacy");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("history.jsonl"),
            serde_json::json!({
                "timestamp": 1794677280,
                "disk_used_bytes": 500,
                "disk_total_bytes": 1000,
                "disk_used_percent": 50.0,
                "projected_used_bytes": 450,
                "projected_used_percent": 45.0,
                "high_watermark_percent": 85.0,
                "low_watermark_percent": 82.0,
                "reclaimable_bytes": 300,
                "protected_bytes": 100,
                "estimated_reclaimed_bytes": 50,
                "actual_reclaimed_bytes": 50
            })
            .to_string()
                + "\n",
        )
        .unwrap();

        let config: Config = toml::from_str(
            r#"
            url = "http://127.0.0.1:8080"
            disk_path = "/data/shared"
            managed_categories = ["radarr"]
            "#,
        )
        .unwrap();
        let database = Database::new(root.join("graine.sqlite3"), legacy);
        database.initialize().unwrap();
        seed_dashboard_from_legacy(&config, &database).unwrap();

        let payload: serde_json::Value = serde_json::from_str(
            &database.dashboard_json().unwrap().unwrap(),
        )
        .unwrap();
        assert_eq!(payload["run"]["mode"], "historical");
        assert_eq!(payload["run"]["disk_used_bytes"], 500);
        assert_eq!(payload["reclaimable_bytes"], 300);
        assert_eq!(payload["ranking"].as_array().unwrap().len(), 0);

        std::fs::remove_dir_all(root).unwrap();
    }
}
