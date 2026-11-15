use crate::{
    config::{Config, tracker_host, tracker_label},
    model::{Cluster, DashboardCluster, DeletionBatch, DeletionEvent, HistorySnapshot},
};
use std::collections::HashSet;

pub(crate) fn cluster_dashboard_row(
    cfg: &Config,
    tag: &str,
    cluster: &Cluster,
    is_dry: bool,
) -> DashboardCluster {
    let mut torrent_names: Vec<String> = cluster
        .torrents
        .iter()
        .map(|torrent| torrent.name.clone())
        .collect();
    torrent_names.sort();

    let name = cluster
        .torrents
        .iter()
        .min_by_key(|torrent| torrent.added_on)
        .map(|torrent| torrent.name.clone())
        .unwrap_or_else(|| "Unknown cluster".to_string());

    let mut tracker_hosts: Vec<String> = cluster
        .torrents
        .iter()
        .map(|torrent| tracker_label(cfg, &tracker_host(&torrent.tracker)))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    tracker_hosts.sort();

    let (status, reason) = match tag {
        "(DEL-DISK)" => (
            (if is_dry { "WOULD DELETE" } else { "DELETED" }).to_string(),
            Some("Disk watermark".to_string()),
        ),
        "(FREE)" => ("FREE".to_string(), None),
        "(GRACE)" => ("GRACE".to_string(), None),
        _ => ("GOOD".to_string(), None),
    };

    DashboardCluster {
        rank: cluster.rank,
        name,
        torrent_names,
        tracker_hosts,
        category: cluster.category.clone(),
        status,
        reason,
        priority: cluster.priority,
        score: cluster.score.is_finite().then_some(cluster.score),
        cost_bytes: cluster.cost,
        data_bytes: cluster.data_bytes,
        uploaded_bytes: cluster.uploaded,
        seeding_days: cluster.days as f64,
        torrent_count: cluster.torrents.len(),
        cross_seed_count: cluster
            .torrents
            .iter()
            .filter(|torrent| torrent.is_cross())
            .count(),
    }
}

pub(crate) fn deletion_batch(
    cfg: &Config,
    dropped: &[Cluster],
    timestamp: u64,
    observed_reclaimed_bytes: Option<u64>,
) -> DeletionBatch {
    let clusters = dropped
        .iter()
        .map(|cluster| {
            let mut torrent_names: Vec<String> = cluster
                .torrents
                .iter()
                .map(|torrent| torrent.name.clone())
                .collect();
            torrent_names.sort();

            let mut tracker_hosts: Vec<String> = cluster
                .torrents
                .iter()
                .map(|torrent| tracker_label(cfg, &tracker_host(&torrent.tracker)))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            tracker_hosts.sort();

            DeletionEvent {
                name: cluster
                    .torrents
                    .iter()
                    .min_by_key(|torrent| torrent.added_on)
                    .map(|torrent| torrent.name.clone())
                    .unwrap_or_else(|| "Unknown cluster".to_string()),
                torrent_names,
                tracker_hosts,
                category: cluster.category.clone(),
                reason: "Disk watermark".to_string(),
                priority: cluster.priority,
                score: cluster.score.is_finite().then_some(cluster.score),
                estimated_reclaimed_bytes: cluster.cost,
                uploaded_bytes: cluster.uploaded,
                seeding_days: cluster.days as f64,
            }
        })
        .collect();

    let estimated_reclaimed_bytes = dropped
        .iter()
        .map(|cluster| cluster.cost)
        .fold(0_u64, u64::saturating_add);

    DeletionBatch {
        batch_id: timestamp.to_string(),
        timestamp,
        estimated_reclaimed_bytes,
        observed_reclaimed_bytes,
        clusters,
    }
}

pub(crate) fn history_snapshot(
    timestamp: u64,
    disk_used: u64,
    disk_total: u64,
    projected_used: u64,
    estimated_reclaimed: u64,
    actual_reclaimed: Option<u64>,
    reclaimable_bytes: u64,
    protected_bytes: u64,
    category_usage: std::collections::BTreeMap<String, u64>,
    high_watermark: f64,
    low_watermark: f64,
) -> HistorySnapshot {
    HistorySnapshot {
        timestamp,
        disk_used_bytes: disk_used,
        disk_total_bytes: disk_total,
        disk_used_percent: if disk_total == 0 {
            0.0
        } else {
            disk_used as f64 / disk_total as f64 * 100.0
        },
        projected_used_bytes: projected_used,
        projected_used_percent: if disk_total == 0 {
            0.0
        } else {
            projected_used as f64 / disk_total as f64 * 100.0
        },
        high_watermark_percent: high_watermark * 100.0,
        low_watermark_percent: low_watermark * 100.0,
        reclaimable_bytes,
        protected_bytes,
        estimated_reclaimed_bytes: estimated_reclaimed,
        actual_reclaimed_bytes: actual_reclaimed,
        category_usage,
    }
}

