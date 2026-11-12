use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[derive(Clone, Deserialize)]
pub(crate) struct Torrent {
    pub(crate) hash: String,
    pub(crate) name: String,
    pub(crate) category: String,
    pub(crate) save_path: String,
    pub(crate) progress: f64,
    pub(crate) seeding_time: u64,
    pub(crate) added_on: u64,
    pub(crate) last_activity: u64,
    pub(crate) uploaded: u64,
    pub(crate) downloaded: u64,
    pub(crate) tracker: String,
}

impl Torrent {
    pub(crate) fn is_cross(&self) -> bool {
        self.category.ends_with(".cross")
    }
}

#[derive(Deserialize)]
pub(crate) struct ApiFile {
    pub(crate) name: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub(crate) struct TransferTotals {
    pub(crate) uploaded_bytes: u64,
    pub(crate) downloaded_bytes: u64,
}

impl TransferTotals {
    pub(crate) fn add(&mut self, uploaded: u64, downloaded: u64) {
        self.uploaded_bytes = self.uploaded_bytes.saturating_add(uploaded);
        self.downloaded_bytes = self.downloaded_bytes.saturating_add(downloaded);
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub(crate) struct TrackerAnalytics {
    pub(crate) total: TransferTotals,
    pub(crate) regular: TransferTotals,
    pub(crate) cross_seed: TransferTotals,
}

/// Records remain after qBittorrent removes a torrent, preserving all-time leaderboards.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct TorrentAnalytics {
    pub(crate) name: String,
    pub(crate) tracker: String,
    pub(crate) category: String,
    pub(crate) added_on: u64,
    pub(crate) first_seen_at: u64,
    pub(crate) last_seen_at: u64,
    pub(crate) all_time_uploaded_bytes: u64,
    pub(crate) all_time_downloaded_bytes: u64,
    pub(crate) last_uploaded_counter: u64,
    pub(crate) last_downloaded_counter: u64,
}

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct AnalyticsState {
    pub(crate) version: u32,
    /// The first live run of this version. Older torrents are baselined, not assigned to a period.
    pub(crate) tracking_started_at: u64,
    pub(crate) trackers: HashMap<String, TrackerAnalytics>,
    pub(crate) torrents: HashMap<String, TorrentAnalytics>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct ActivityEvent {
    #[serde(default)]
    pub(crate) event_id: String,
    pub(crate) timestamp: u64,
    pub(crate) hash: String,
    pub(crate) name: String,
    pub(crate) tracker: String,
    pub(crate) category: String,
    pub(crate) is_cross_seed: bool,
    pub(crate) uploaded_bytes: u64,
    pub(crate) downloaded_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct HistorySnapshot {
    pub(crate) timestamp: u64,
    pub(crate) disk_used_bytes: u64,
    pub(crate) disk_total_bytes: u64,
    pub(crate) disk_used_percent: f64,
    pub(crate) projected_used_bytes: u64,
    pub(crate) projected_used_percent: f64,
    pub(crate) high_watermark_percent: f64,
    pub(crate) low_watermark_percent: f64,
    pub(crate) reclaimable_bytes: u64,
    pub(crate) protected_bytes: u64,
    pub(crate) estimated_reclaimed_bytes: u64,
    pub(crate) actual_reclaimed_bytes: Option<u64>,
    /// Completed, retained torrent data attributed once per inode to its original category.
    /// Older snapshots do not contain this field and are rendered as unclassified/Other.
    #[serde(default)]
    pub(crate) category_usage: BTreeMap<String, u64>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct DeletionEvent {
    pub(crate) name: String,
    pub(crate) torrent_names: Vec<String>,
    pub(crate) tracker_hosts: Vec<String>,
    /// `pool` is accepted for compatibility with older deletion history records.
    #[serde(alias = "pool")]
    pub(crate) category: String,
    pub(crate) reason: String,
    pub(crate) priority: i32,
    pub(crate) score: Option<f64>,
    pub(crate) estimated_reclaimed_bytes: u64,
    pub(crate) uploaded_bytes: u64,
    pub(crate) seeding_days: f64,
}

/// One API deletion call may remove several clusters. The measured df delta belongs to the batch.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct DeletionBatch {
    pub(crate) batch_id: String,
    pub(crate) timestamp: u64,
    pub(crate) estimated_reclaimed_bytes: u64,
    pub(crate) observed_reclaimed_bytes: Option<u64>,
    pub(crate) clusters: Vec<DeletionEvent>,
}

#[derive(Serialize, Clone)]
pub(crate) struct LeaderboardEntry {
    pub(crate) hash: String,
    pub(crate) name: String,
    pub(crate) tracker: String,
    pub(crate) category: String,
    pub(crate) is_cross_seed: bool,
    pub(crate) uploaded_bytes: u64,
    pub(crate) downloaded_bytes: u64,
    pub(crate) timestamp: u64,
}

/// Cumulative net upload values used by the tracker's time-series chart.
#[derive(Serialize, Clone, Default)]
pub(crate) struct TrackerHistoryTotals {
    pub(crate) total_net_bytes: i128,
    pub(crate) regular_net_bytes: i128,
    pub(crate) cross_seed_net_bytes: i128,
}

impl TrackerHistoryTotals {
    pub(crate) fn add(&mut self, uploaded: u64, downloaded: u64, is_cross_seed: bool) {
        let net = uploaded as i128 - downloaded as i128;
        self.total_net_bytes += net;
        if is_cross_seed {
            self.cross_seed_net_bytes += net;
        } else {
            self.regular_net_bytes += net;
        }
    }
}

#[derive(Serialize, Clone)]
pub(crate) struct TrackerHistoryPoint {
    pub(crate) timestamp: u64,
    pub(crate) trackers: BTreeMap<String, TrackerHistoryTotals>,
}

#[derive(Serialize, Clone)]
pub(crate) struct AnalyticsPeriod {
    pub(crate) torrents: Vec<LeaderboardEntry>,
    pub(crate) tracker_history: Vec<TrackerHistoryPoint>,
}

#[derive(Serialize)]
pub(crate) struct DashboardCluster {
    pub(crate) rank: usize,
    pub(crate) name: String,
    pub(crate) torrent_names: Vec<String>,
    pub(crate) tracker_hosts: Vec<String>,
    pub(crate) category: String,
    pub(crate) status: String,
    pub(crate) reason: Option<String>,
    pub(crate) priority: i32,
    pub(crate) score: Option<f64>,
    pub(crate) cost_bytes: u64,
    pub(crate) data_bytes: u64,
    pub(crate) uploaded_bytes: u64,
    pub(crate) seeding_days: f64,
    pub(crate) torrent_count: usize,
    pub(crate) cross_seed_count: usize,
}

#[derive(Serialize)]
pub(crate) struct RunSummary {
    pub(crate) timestamp: u64,
    pub(crate) mode: String,
    pub(crate) result: String,
    pub(crate) disk_used_bytes: u64,
    pub(crate) disk_total_bytes: u64,
    pub(crate) disk_used_percent: f64,
    pub(crate) projected_used_bytes: u64,
    pub(crate) projected_used_percent: f64,
    pub(crate) high_watermark_percent: f64,
    pub(crate) low_watermark_percent: f64,
    pub(crate) estimated_reclaimed_bytes: u64,
    pub(crate) actual_reclaimed_bytes: Option<u64>,
    pub(crate) managed_torrents: usize,
    pub(crate) complete_torrents: usize,
    pub(crate) clusters: usize,
}

#[derive(Serialize)]
pub(crate) struct DashboardData {
    pub(crate) generated_at: u64,
    pub(crate) run: RunSummary,
    pub(crate) history: Vec<HistorySnapshot>,
    pub(crate) reclaimable_bytes: u64,
    pub(crate) protected_bytes: u64,
    pub(crate) ranking: Vec<DashboardCluster>,
    pub(crate) periods: HashMap<String, AnalyticsPeriod>,
    pub(crate) deletions: Vec<DeletionBatch>,
}

pub(crate) struct FileRef {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) nlink: u64,
    pub(crate) size: u64,
    pub(crate) allocated_bytes: u64,
    pub(crate) path: PathBuf,
}

pub(crate) struct Entry {
    pub(crate) torrent: Torrent,
    pub(crate) files: Vec<FileRef>,
}

pub(crate) struct Cluster {
    pub(crate) torrents: Vec<Torrent>,
    pub(crate) category: String,
    pub(crate) priority: i32,
    pub(crate) cost: u64,
    pub(crate) data_bytes: u64,
    /// Allocated filesystem blocks, deduplicated by inode, for the disk-usage chart.
    pub(crate) disk_bytes: u64,
    /// Allocated blocks whose hardlinks outside this cluster keep them on disk after eviction.
    pub(crate) protected_disk_bytes: u64,
    pub(crate) uploaded: u64,
    pub(crate) days: f32,
    pub(crate) is_grace: bool,
    pub(crate) score: f64,
    pub(crate) rank: usize,
}
