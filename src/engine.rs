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

/// Stable ranking for a single period, with a deterministic tie-breaker.
fn period_ranks(period: &AnalyticsPeriod) -> HashMap<String, i32> {
    let mut sorted: Vec<_> = period.torrents.iter().collect();
    sorted.sort_by(|a, b| {
        let net_a = a.uploaded_bytes as i128 - a.downloaded_bytes as i128;
        let net_b = b.uploaded_bytes as i128 - b.downloaded_bytes as i128;
        net_b.cmp(&net_a).then_with(|| a.hash.cmp(&b.hash))
    });
    sorted.iter().enumerate()
        .map(|(index, torrent)| (torrent.hash.clone(), (index + 1) as i32))
        .collect()
}

fn rank_difference(initial: &HashMap<String, i32>, current: &HashMap<String, i32>) -> HashMap<String, i32> {
    current.iter().filter_map(|(hash, current_rank)| {
        initial.get(hash).map(|old_rank| (hash.clone(), old_rank - current_rank))
    }).collect()
}

type Ranks = HashMap<String, i32>;
type NewBaseline = (String, u64, Ranks);

struct RankChangeResult {
    changes: HashMap<String, Ranks>,
    baseline_dates: HashMap<String, u64>,
    new_baselines: Vec<NewBaseline>,
}

/// All-time compares with the previous run. Calendar periods compare with the
/// first successful, non-dry run observed in that period. Never invent a ranking
/// at midnight if Graine was not running then.
fn rank_changes_from_baselines(
    database: &Database,
    periods: &HashMap<String, AnalyticsPeriod>,
    timestamp: u64,
    is_dry: bool,
) -> Result<RankChangeResult> {
    let (week, month, year) = crate::util::period_starts(timestamp);
    let starts = [("week", week), ("month", month), ("year", year)];
    let mut changes = HashMap::new();
    let mut baseline_dates = HashMap::new();
    let mut new_baselines = Vec::new();

    for (name, start) in starts {
        let Some(current) = periods.get(name) else { continue };
        let current_ranks = period_ranks(current);
        if let Some((captured_at, initial_ranks)) = database.leaderboard_baseline(name, start)? {
            baseline_dates.insert(name.to_string(), captured_at);
            changes.insert(name.to_string(), rank_difference(&initial_ranks, &current_ranks));
        } else if !is_dry {
            // Store the first real observation as this calendar period's reference.
            new_baselines.push((name.to_string(), start, current_ranks));
        }
    }

    if let (Some(current), Some(previous_json)) = (periods.get("all"), database.dashboard_json()?) {
        let previous: serde_json::Value = serde_json::from_str(&previous_json)?;
        if let Some(old) = previous.pointer("/periods/all/torrents").and_then(|v| v.as_array()) {
            let mut previous_ranks: Vec<(String, i128)> = old.iter().filter_map(|item| {
                Some((
                    item.get("hash")?.as_str()?.to_owned(),
                    item.get("uploaded_bytes")?.as_u64()? as i128
                        - item.get("downloaded_bytes")?.as_u64()? as i128,
                ))
            }).collect();
            previous_ranks.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let old_ranks: HashMap<String, i32> = previous_ranks.into_iter().enumerate()
                .map(|(index, (hash, _))| (hash, (index + 1) as i32)).collect();
            let all_changes = rank_difference(&old_ranks, &period_ranks(current));
            changes.insert("all".to_string(), all_changes);
            if let Some(last_time) = previous.get("generated_at").and_then(|v| v.as_u64()) {
                baseline_dates.insert("all".to_string(), last_time);
            }
        }
    }
    Ok(RankChangeResult { changes, baseline_dates, new_baselines })
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
        login(&conn, &cfg.username, password)?;
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

    // Logical bytes of unique, fully unlinkable files, deduplicated by inode.
    // `df` still measures global filesystem occupancy for the watermarks, but
    // its before/after delta must not be attributed to individual deletions.
    let actual_reclaimed = None;
    let mut disk_after = (used_before, total);
    let mut deletions: Vec<DeletionBatch> = database.load_deletions()?;
    let mut new_deletion_batch = None;
    if !dropped.is_empty() && !is_dry {
        let to_delete: Vec<_> = dropped
            .iter()
            .flat_map(|cluster| cluster.torrents.iter().cloned())
            .collect();
        delete_torrents(&conn, &to_delete)?;
        // Keep the disk-history snapshot accurate, but never use the global
        // filesystem delta to report data removed by these torrents.
        disk_after = disk_usage(&cfg.disk_path)?;
        let batch = deletion_batch(cfg, &dropped, timestamp, None);
        deletions.push(batch.clone());
        new_deletion_batch = Some(batch);
        tracing::info!(
            deleted_torrents = to_delete.len(),
            logical_removed_gib = estimated_reclaimed as f64 / GIB as f64,
            "torrent deletion request completed"
        );
    } else if !dropped.is_empty() {
        tracing::info!(
            would_delete_torrents = dropped.iter().map(|cluster| cluster.torrents.len()).sum::<usize>(),
            "dry run: torrents would be deleted"
        );
    } else {
        tracing::info!("no torrents need eviction");
    }

    let (used_after, total_after) = disk_after;
    let snapshot = history_snapshot(crate::dashboard::SnapshotInput {
        timestamp,
        disk_used: used_after,
        disk_total: total_after,
        projected_used: projected,
        estimated_reclaimed,
        actual_reclaimed,
        reclaimable_bytes,
        protected_bytes,
        category_usage,
        high_watermark: cfg.high_watermark,
        low_watermark: cfg.low_watermark,
    });

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

    let planned_torrents: usize = dropped.iter().map(|cluster| cluster.torrents.len()).sum();
    let planned_clusters = dropped.len();
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
        planned_torrents,
        planned_clusters,
        removed_torrents: if is_dry { 0 } else { planned_torrents },
        removed_clusters: if is_dry { 0 } else { planned_clusters },
        deletion_batch_id: new_deletion_batch.as_ref().map(|batch| batch.batch_id.clone()),
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
        rank_baselines_at: HashMap::new(),
        deletions,
    };
    let rank_changes = rank_changes_from_baselines(database, &dashboard.periods, timestamp, is_dry)?;
    dashboard.rank_changes = rank_changes.changes;
    dashboard.rank_baselines_at = rank_changes.baseline_dates;

    database
        .save_run(
            &snapshot,
            new_deletion_batch.as_ref(),
            &dashboard,
            is_dry,
            &rank_changes.new_baselines,
        )
        .context("cannot save management results to SQLite")?;

    Ok(dashboard)
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn rank_difference_uses_reference_ranks_and_ignores_new_torrents() {
        let previous = HashMap::from([
            ("a".into(), 3),
            ("b".into(), 1),
            ("c".into(), 2),
        ]);
        let current = HashMap::from([
            ("a".into(), 1),
            ("b".into(), 3),
            ("c".into(), 2),
            ("new".into(), 4),
        ]);
        let changes = rank_difference(&previous, &current);
        assert_eq!(changes.get("a"), Some(&2));
        assert_eq!(changes.get("b"), Some(&-2));
        assert_eq!(changes.get("c"), Some(&0));
        assert_eq!(changes.get("new"), None);
    }


}
