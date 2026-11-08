use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    env,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, exit},
    time::{SystemTime, UNIX_EPOCH},
};
use ureq::Agent;

const DAY_IN_SEC: u64 = 60 * 60 * 24;
const GIB: u64 = 1024 * 1024 * 1024;
const CROSS_SUFFIX: &str = ".cross";
const ANALYTICS_VERSION: u32 = 2;
const ANALYTICS_FILE: &str = "analytics.json";
const ACTIVITY_FILE: &str = "activity.jsonl";
const HISTORY_FILE: &str = "history.jsonl";
const DELETIONS_FILE: &str = "deletions.jsonl";
const DASHBOARD_FILE: &str = "/var/www/graine/index.html";
const USAGE: &str = "Usage: QBIT_PW=<password> graine <config.toml> [--dry]";

fn default_high() -> f64 {
    0.85
}
fn default_low() -> f64 {
    0.80
}
fn default_grace() -> u64 {
    7
}

/// A group of categories that share rules. `<category>.cross` is included automatically.
#[derive(Deserialize)]
struct Pool {
    name: String,
    categories: Vec<String>,
    /// Optional cap on the disk this pool may occupy (reclaimable bytes only).
    capacity_gib: Option<u64>,
    /// Never evict a cluster whose original is younger than this (hit and run protection).
    #[serde(default = "default_grace")]
    grace_days: u64,
}

/// Trackers you care about. Unlisted trackers have priority 0. When graine has to delete,
/// lower priority goes first no matter the score; equal priority is ranked by score.
#[derive(Deserialize)]
struct TrackerPriority {
    /// Matched as a substring of the torrent's tracker URL, e.g. "tracker.example.org".
    host: String,
    priority: i32,
}

#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    trackers: Vec<TrackerPriority>,
    url: String,
    /// Any path on the filesystem that holds downloads and library.
    disk_path: String,
    /// Start evicting above this fraction of disk used...
    #[serde(default = "default_high")]
    high_watermark: f64,
    /// ...and stop once back under this one.
    #[serde(default = "default_low")]
    low_watermark: f64,
    pools: Vec<Pool>,
}

#[derive(Clone, Deserialize)]
struct Torrent {
    hash: String,
    name: String,
    category: String,
    save_path: String,
    progress: f64,
    seeding_time: u64,
    added_on: u64,
    last_activity: u64,
    uploaded: u64,
    downloaded: u64,
    tracker: String,
}

#[derive(Deserialize)]
struct ApiFile {
    name: String,
}

/// Uploaded and downloaded bytes are stored separately. Net upload is derived by the UI.
#[derive(Serialize, Deserialize, Default, Clone)]
struct TransferTotals {
    uploaded_bytes: u64,
    downloaded_bytes: u64,
}

impl TransferTotals {
    fn add(&mut self, uploaded: u64, downloaded: u64) {
        self.uploaded_bytes = self.uploaded_bytes.saturating_add(uploaded);
        self.downloaded_bytes = self.downloaded_bytes.saturating_add(downloaded);
    }
}

/// The tracker total is deliberately split by category as well as kept as a total.
#[derive(Serialize, Deserialize, Default, Clone)]
struct TrackerAnalytics {
    total: TransferTotals,
    regular: TransferTotals,
    cross_seed: TransferTotals,
}

/// Records remain after qBittorrent removes a torrent, preserving all-time leaderboards.
#[derive(Serialize, Deserialize, Clone)]
struct TorrentAnalytics {
    name: String,
    tracker: String,
    category: String,
    added_on: u64,
    first_seen_at: u64,
    last_seen_at: u64,
    all_time_uploaded_bytes: u64,
    all_time_downloaded_bytes: u64,
    last_uploaded_counter: u64,
    last_downloaded_counter: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct AnalyticsState {
    version: u32,
    /// The first live run of this version. Older torrents are baselined, not assigned to a period.
    tracking_started_at: u64,
    trackers: HashMap<String, TrackerAnalytics>,
    torrents: HashMap<String, TorrentAnalytics>,
}

/// A counter delta observed during one graine run. `event_id` makes a retry after a crash
/// between appending this file and saving analytics.json safe to aggregate only once.
#[derive(Serialize, Deserialize, Clone)]
struct ActivityEvent {
    #[serde(default)]
    event_id: String,
    timestamp: u64,
    hash: String,
    name: String,
    tracker: String,
    category: String,
    is_cross_seed: bool,
    uploaded_bytes: u64,
    downloaded_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone)]
struct PoolSnapshot {
    name: String,
    reclaimable_bytes: u64,
    protected_bytes: u64,
    capacity_bytes: Option<u64>,
}

/// A point in time after the run. All pool figures describe clusters retained after its decisions.
#[derive(Serialize, Deserialize, Clone)]
struct HistorySnapshot {
    timestamp: u64,
    disk_used_bytes: u64,
    disk_total_bytes: u64,
    disk_used_percent: f64,
    projected_used_bytes: u64,
    projected_used_percent: f64,
    high_watermark_percent: f64,
    low_watermark_percent: f64,
    reclaimable_bytes: u64,
    protected_bytes: u64,
    estimated_reclaimed_bytes: u64,
    actual_reclaimed_bytes: Option<u64>,
    pools: Vec<PoolSnapshot>,
}

#[derive(Serialize, Deserialize, Clone)]
struct DeletionEvent {
    name: String,
    torrent_names: Vec<String>,
    tracker_hosts: Vec<String>,
    pool: String,
    reason: String,
    priority: i32,
    score: Option<f64>,
    estimated_reclaimed_bytes: u64,
    uploaded_bytes: u64,
    seeding_days: f64,
}

/// One API deletion call may remove several clusters. The measured df delta belongs to the batch,
/// not to any individual torrent, so it is stored once here rather than misleadingly apportioned.
#[derive(Serialize, Deserialize, Clone)]
struct DeletionBatch {
    batch_id: String,
    timestamp: u64,
    estimated_reclaimed_bytes: u64,
    observed_reclaimed_bytes: Option<u64>,
    clusters: Vec<DeletionEvent>,
}

#[derive(Serialize, Clone)]
struct LeaderboardEntry {
    hash: String,
    name: String,
    tracker: String,
    category: String,
    is_cross_seed: bool,
    uploaded_bytes: u64,
    downloaded_bytes: u64,
    timestamp: u64,
}

#[derive(Serialize, Clone)]
struct TrackerRow {
    tracker: String,
    total: TransferTotals,
    regular: TransferTotals,
    cross_seed: TransferTotals,
}

#[derive(Serialize, Clone)]
struct AnalyticsPeriod {
    trackers: Vec<TrackerRow>,
    torrents: Vec<LeaderboardEntry>,
}

#[derive(Serialize)]
struct DashboardCluster {
    rank: usize,
    name: String,
    torrent_names: Vec<String>,
    tracker_hosts: Vec<String>,
    pool: String,
    status: String,
    reason: Option<String>,
    priority: i32,
    score: Option<f64>,
    cost_bytes: u64,
    data_bytes: u64,
    uploaded_bytes: u64,
    seeding_days: f64,
    torrent_count: usize,
    cross_seed_count: usize,
}

#[derive(Serialize)]
struct RunSummary {
    timestamp: u64,
    mode: String,
    result: String,
    disk_used_bytes: u64,
    disk_total_bytes: u64,
    disk_used_percent: f64,
    projected_used_bytes: u64,
    projected_used_percent: f64,
    high_watermark_percent: f64,
    low_watermark_percent: f64,
    estimated_reclaimed_bytes: u64,
    actual_reclaimed_bytes: Option<u64>,
    managed_torrents: usize,
    complete_torrents: usize,
    clusters: usize,
}

#[derive(Serialize)]
struct DashboardData {
    generated_at: u64,
    run: RunSummary,
    history: Vec<HistorySnapshot>,
    pools: Vec<PoolSnapshot>,
    reclaimable_bytes: u64,
    protected_bytes: u64,
    ranking: Vec<DashboardCluster>,
    periods: HashMap<String, AnalyticsPeriod>,
    deletions: Vec<DeletionBatch>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Convert days since 1970-01-01 to a Gregorian UTC date.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year, month as u32, day as u32)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = month as i64 + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn period_starts(now: u64) -> (u64, u64, u64) {
    let days = (now / DAY_IN_SEC) as i64;
    let (year, month, _) = civil_from_days(days);
    let year_start = days_from_civil(year, 1, 1) as u64 * DAY_IN_SEC;
    let month_start = days_from_civil(year, month, 1) as u64 * DAY_IN_SEC;
    let week_start = now - (((days as u64 + 4) % 7) * DAY_IN_SEC) - (now % DAY_IN_SEC);
    (week_start, month_start, year_start)
}

impl Torrent {
    fn is_cross(&self) -> bool {
        self.category.ends_with(CROSS_SUFFIX)
    }
}

fn pool_of(cfg: &Config, category: &str) -> Option<usize> {
    let base = category.strip_suffix(CROSS_SUFFIX).unwrap_or(category);
    cfg.pools
        .iter()
        .position(|p| p.categories.iter().any(|c| c == base))
}

fn tracker_priority(cfg: &Config, tracker: &str) -> i32 {
    cfg.trackers
        .iter()
        .find(|t| tracker.contains(&t.host))
        .map_or(0, |t| t.priority)
}

/// Use the tracker hostname as the stable aggregation key, not the announce URL.
fn tracker_host(tracker: &str) -> String {
    let without_scheme = tracker.split_once("://").map_or(tracker, |(_, rest)| rest);
    let authority = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(without_scheme)
        .rsplit('@')
        .next()
        .unwrap_or("");
    let host = if authority.starts_with('[') {
        authority
            .find(']')
            .map_or(authority, |end| &authority[..=end])
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    if host.is_empty() {
        "unknown".to_string()
    } else {
        host.to_ascii_lowercase()
    }
}

fn add_tracker_totals(
    trackers: &mut HashMap<String, TrackerAnalytics>,
    tracker: &str,
    is_cross_seed: bool,
    uploaded: u64,
    downloaded: u64,
) {
    let totals = trackers.entry(tracker.to_string()).or_default();
    totals.total.add(uploaded, downloaded);
    if is_cross_seed {
        totals.cross_seed.add(uploaded, downloaded);
    } else {
        totals.regular.add(uploaded, downloaded);
    }
}

fn counter_delta(current: u64, previous: u64) -> u64 {
    // qBittorrent can reset a counter when a torrent is rechecked/re-added. Count the new
    // counter value as fresh activity instead of allowing subtraction to underflow.
    current.checked_sub(previous).unwrap_or(current)
}

fn atomic_write(path: &str, contents: &[u8]) {
    let temporary = format!("{path}.tmp");
    fs::write(&temporary, contents).unwrap_or_else(|e| {
        eprintln!("Cannot write {temporary}: {e}");
        exit(1);
    });
    fs::rename(&temporary, path).unwrap_or_else(|e| {
        eprintln!("Cannot replace {path}: {e}");
        exit(1);
    });
}

fn load_analytics() -> AnalyticsState {
    let content = match fs::read_to_string(ANALYTICS_FILE) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return AnalyticsState::default();
        }
        Err(e) => {
            eprintln!("Cannot read {ANALYTICS_FILE}: {e}");
            exit(1);
        }
    };
    let state: AnalyticsState = serde_json::from_str(&content).unwrap_or_else(|e| {
        eprintln!("Cannot parse {ANALYTICS_FILE}: {e}; refusing to reset analytics history");
        exit(1)
    });
    if state.version != 0 && state.version != 1 && state.version != ANALYTICS_VERSION {
        eprintln!(
            "Unsupported analytics version {} (expected {}); refusing to overwrite it",
            state.version, ANALYTICS_VERSION
        );
        exit(1);
    }
    state
}

fn append_jsonl<T: Serialize>(path: &str, records: &[T]) {
    if records.is_empty() {
        return;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|e| {
            eprintln!("Cannot open {path}: {e}");
            exit(1);
        });
    for record in records {
        serde_json::to_writer(&mut file, record).unwrap_or_else(|e| {
            eprintln!("Cannot serialize a record for {path}: {e}");
            exit(1);
        });
        file.write_all(b"\n").unwrap_or_else(|e| {
            eprintln!("Cannot append to {path}: {e}");
            exit(1);
        });
    }
    file.sync_data().unwrap_or_else(|e| {
        eprintln!("Cannot sync {path}: {e}");
        exit(1);
    });
}

fn read_jsonl<T: DeserializeOwned>(path: &str) -> Vec<T> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!("Cannot read {path}: {e}");
            exit(1);
        }
    };
    content
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            if line.trim().is_empty() {
                return None;
            }
            match serde_json::from_str(line) {
                Ok(record) => Some(record),
                Err(e) => {
                    // A process killed midway through the final append may leave one partial line.
                    eprintln!(
                        "WARN: ignoring invalid JSON in {path}, line {}: {e}",
                        index + 1
                    );
                    None
                }
            }
        })
        .collect()
}

/// Update all-time counters and append observed deltas. All-time counters are seeded from
/// qBittorrent's lifetime counters on first observation. Period rankings begin at installation;
/// old lifetime traffic is not falsely attributed to the current week/month/year.
fn track_analytics(torrents: &[Torrent], is_dry: bool) -> (AnalyticsState, Vec<ActivityEvent>) {
    let now = now_unix();
    let mut state = load_analytics();
    if state.tracking_started_at == 0 {
        state.tracking_started_at = now;
    }
    state.version = ANALYTICS_VERSION;
    let mut events = Vec::new();

    for torrent in torrents {
        let tracker = tracker_host(&torrent.tracker);
        let is_cross_seed = torrent.is_cross();

        if let Some(previous) = state.torrents.get_mut(&torrent.hash) {
            let previous_uploaded = previous.last_uploaded_counter;
            let previous_downloaded = previous.last_downloaded_counter;
            let uploaded_delta = counter_delta(torrent.uploaded, previous_uploaded);
            let downloaded_delta = counter_delta(torrent.downloaded, previous_downloaded);

            previous.name = torrent.name.clone();
            previous.tracker = tracker.clone();
            previous.category = torrent.category.clone();
            previous.added_on = torrent.added_on;
            previous.last_seen_at = now;
            previous.last_uploaded_counter = torrent.uploaded;
            previous.last_downloaded_counter = torrent.downloaded;
            previous.all_time_uploaded_bytes = previous
                .all_time_uploaded_bytes
                .saturating_add(uploaded_delta);
            previous.all_time_downloaded_bytes = previous
                .all_time_downloaded_bytes
                .saturating_add(downloaded_delta);

            add_tracker_totals(
                &mut state.trackers,
                &tracker,
                is_cross_seed,
                uploaded_delta,
                downloaded_delta,
            );

            if uploaded_delta > 0 || downloaded_delta > 0 {
                events.push(ActivityEvent {
                    event_id: format!(
                        "{}:{}:{}:{}:{}",
                        torrent.hash,
                        previous_uploaded,
                        torrent.uploaded,
                        previous_downloaded,
                        torrent.downloaded
                    ),
                    timestamp: now,
                    hash: torrent.hash.clone(),
                    name: torrent.name.clone(),
                    tracker,
                    category: torrent.category.clone(),
                    is_cross_seed,
                    uploaded_bytes: uploaded_delta,
                    downloaded_bytes: downloaded_delta,
                });
            }
        } else {
            let added_during_tracking =
                torrent.added_on >= state.tracking_started_at && torrent.added_on <= now;
            let period_uploaded = if added_during_tracking {
                torrent.uploaded
            } else {
                0
            };
            let period_downloaded = if added_during_tracking {
                torrent.downloaded
            } else {
                0
            };

            state.torrents.insert(
                torrent.hash.clone(),
                TorrentAnalytics {
                    name: torrent.name.clone(),
                    tracker: tracker.clone(),
                    category: torrent.category.clone(),
                    added_on: torrent.added_on,
                    first_seen_at: now,
                    last_seen_at: now,
                    all_time_uploaded_bytes: torrent.uploaded,
                    all_time_downloaded_bytes: torrent.downloaded,
                    last_uploaded_counter: torrent.uploaded,
                    last_downloaded_counter: torrent.downloaded,
                },
            );
            add_tracker_totals(
                &mut state.trackers,
                &tracker,
                is_cross_seed,
                torrent.uploaded,
                torrent.downloaded,
            );

            if period_uploaded > 0 || period_downloaded > 0 {
                events.push(ActivityEvent {
                    event_id: format!(
                        "{}:0:{}:0:{}",
                        torrent.hash, torrent.uploaded, torrent.downloaded
                    ),
                    timestamp: now,
                    hash: torrent.hash.clone(),
                    name: torrent.name.clone(),
                    tracker,
                    category: torrent.category.clone(),
                    is_cross_seed,
                    uploaded_bytes: period_uploaded,
                    downloaded_bytes: period_downloaded,
                });
            }
        }
    }

    if !is_dry {
        // Write the log first. If saving state fails, a retry may append the same event again;
        // event_id deduplication in the dashboard aggregation prevents double counting.
        append_jsonl(ACTIVITY_FILE, &events);
        let json = serde_json::to_vec_pretty(&state).unwrap_or_else(|e| {
            eprintln!("Cannot serialize {ANALYTICS_FILE}: {e}");
            exit(1);
        });
        atomic_write(ANALYTICS_FILE, &json);
    }
    (state, events)
}

struct ConnectionInfo {
    agent: Agent,
    url: String,
}

fn login(conn: &ConnectionInfo, pw: &str) {
    match conn
        .agent
        .post(format!("{}/api/v2/auth/login", conn.url))
        .header("Referer", &conn.url)
        .send_form([("username", "admin"), ("password", pw)])
    {
        Ok(r) => {
            if !r.headers().contains_key("set-cookie") {
                eprintln!("Login failed, did not get an auth cookie");
                exit(1);
            }
        }
        Err(e) => {
            eprintln!("Login failed: {e}");
            exit(1);
        }
    }
}

fn get_torrents(conn: &ConnectionInfo) -> Vec<Torrent> {
    conn.agent
        .get(format!("{}/api/v2/torrents/info", conn.url))
        .call()
        .unwrap_or_else(|e| {
            eprintln!("Cannot retrieve torrents from qBittorrent: {e}");
            exit(1);
        })
        .body_mut()
        .read_json::<Vec<Torrent>>()
        .unwrap_or_else(|e| {
            eprintln!("Cannot parse qBittorrent torrent list: {e}");
            exit(1);
        })
}

fn delete_torrents(conn: &ConnectionInfo, to_delete: &[Torrent]) {
    let hashes: Vec<String> = to_delete.iter().map(|t| t.hash.clone()).collect();
    let response = conn
        .agent
        .post(format!("{}/api/v2/torrents/delete", conn.url))
        .header("Referer", &conn.url)
        .send_form([
            ("hashes", hashes.join("|")),
            ("deleteFiles", "true".to_string()),
        ]);
    if let Err(e) = response {
        eprintln!("Failed to delete torrents: {e}");
        exit(1)
    }
}

/// (used, total) bytes of the filesystem holding `path`, like `df` reports them.
fn disk_usage(path: &str) -> (u64, u64) {
    let out = Command::new("df")
        .args(["-B1", "--output=used,avail", path])
        .output()
        .unwrap_or_else(|e| {
            eprintln!("Cannot run df: {e}");
            exit(1)
        });
    if !out.status.success() {
        eprintln!(
            "df failed for {path}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        exit(1);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .nth(1)
        .and_then(|line| {
            let mut it = line.split_whitespace().map(|v| v.parse::<u64>());
            match (it.next(), it.next()) {
                (Some(Ok(used)), Some(Ok(avail))) => Some((used, used + avail)),
                _ => None,
            }
        })
        .unwrap_or_else(|| {
            eprintln!("Cannot parse df output for {path}");
            exit(1)
        })
}

struct FileRef {
    dev: u64,
    ino: u64,
    nlink: u64,
    size: u64,
    path: PathBuf,
}

struct Entry {
    torrent: Torrent,
    files: Vec<FileRef>,
}

/// Files of a torrent with their on-disk identity. An API failure aborts the run
/// (nothing gets deleted); a torrent whose files cannot be read is skipped.
fn torrent_files(conn: &ConnectionInfo, t: &Torrent) -> Option<Vec<FileRef>> {
    let files = conn
        .agent
        .get(format!(
            "{}/api/v2/torrents/files?hash={}",
            conn.url, t.hash
        ))
        .call()
        .and_then(|mut r| r.body_mut().read_json::<Vec<ApiFile>>());
    let files = match files {
        Ok(f) if !f.is_empty() => f,
        Ok(_) => {
            eprintln!("WARN: no file list for {}, skipping", t.name);
            return None;
        }
        Err(e) => {
            eprintln!("Failed to list files for {}: {e}, aborting", t.name);
            exit(1)
        }
    };

    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let path = Path::new(&t.save_path).join(&f.name);
        match fs::metadata(&path) {
            Ok(m) => out.push(FileRef {
                dev: m.dev(),
                ino: m.ino(),
                nlink: m.nlink(),
                size: m.len(),
                path,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                eprintln!(
                    "WARN: cannot stat {}: {e}, skipping {}",
                    path.display(),
                    t.name
                );
                return None;
            }
        }
    }
    if out.is_empty() {
        eprintln!("WARN: no file of {} exists on disk, skipping", t.name);
        return None;
    }
    Some(out)
}

/// Torrents sharing any inode form one cluster. `cost` is bytes actually freed if the entire
/// cluster is removed; `data_bytes` counts each distinct inode once, including library-backed data.
struct Cluster {
    torrents: Vec<Torrent>,
    pool: usize,
    priority: i32,
    cost: u64,
    data_bytes: u64,
    uploaded: u64,
    days: f32,
    is_grace: bool,
    score: f64,
    rank: usize,
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn build_cluster(cfg: &Config, entries: Vec<Entry>) -> Cluster {
    // inode -> (hardlink count, size, distinct paths referenced by this cluster)
    let mut inodes: HashMap<(u64, u64), (u64, u64, HashSet<PathBuf>)> = HashMap::new();
    for e in &entries {
        for f in &e.files {
            inodes
                .entry((f.dev, f.ino))
                .or_insert((f.nlink, f.size, HashSet::new()))
                .2
                .insert(f.path.clone());
        }
    }
    let data_bytes = inodes
        .values()
        .map(|(_, size, _)| *size)
        .fold(0_u64, u64::saturating_add);
    let cost = inodes
        .values()
        .filter(|(nlink, _, paths)| paths.len() as u64 >= *nlink)
        .map(|(_, size, _)| *size)
        .fold(0_u64, u64::saturating_add);

    let uploaded = entries
        .iter()
        .map(|e| e.torrent.uploaded)
        .fold(0_u64, u64::saturating_add);
    let seeding_time = entries
        .iter()
        .map(|e| e.torrent.seeding_time)
        .max()
        .unwrap_or(0);
    let days = seeding_time as f32 / DAY_IN_SEC as f32;

    let oldest = entries.iter().min_by_key(|e| e.torrent.added_on).unwrap();
    let pool = pool_of(cfg, &oldest.torrent.category).unwrap_or(0);
    let grace_secs = cfg.pools[pool].grace_days.saturating_mul(DAY_IN_SEC);
    let anchors: Vec<&Entry> = entries.iter().filter(|e| !e.torrent.is_cross()).collect();
    let is_grace = if anchors.is_empty() {
        entries.iter().any(|e| e.torrent.seeding_time < grace_secs)
    } else {
        anchors.iter().any(|e| e.torrent.seeding_time < grace_secs)
    };

    let now = now_unix();

    let upload_rate: f64 = entries
        .iter()
        .map(|e| {
            let seeding_days = (e.torrent.seeding_time as f64 / DAY_IN_SEC as f64).max(1.0);

            let inactive_days =
                now.saturating_sub(e.torrent.last_activity) as f64 / DAY_IN_SEC as f64;

            let activity_factor = 1.0 / (1.0 + inactive_days / 7.0);

            (e.torrent.uploaded as f64 / seeding_days) * activity_factor
        })
        .sum();

    let score = if cost == 0 {
        f64::INFINITY
    } else {
        upload_rate / cost as f64
    };
    let priority = entries
        .iter()
        .map(|e| tracker_priority(cfg, &e.torrent.tracker))
        .max()
        .unwrap_or(0);

    Cluster {
        torrents: entries.into_iter().map(|e| e.torrent).collect(),
        pool,
        priority,
        cost,
        data_bytes,
        uploaded,
        days,
        is_grace,
        score,
        rank: 0,
    }
}

/// Best first: grace, then higher tracker priority, then higher score.
fn compare_clusters(a: &Cluster, b: &Cluster) -> Ordering {
    let (a_score, b_score) = if a.is_grace {
        (0.0, 0.0)
    } else {
        (a.score, b.score)
    };
    b.is_grace
        .cmp(&a.is_grace)
        .then(b.priority.cmp(&a.priority))
        .then(b_score.total_cmp(&a_score))
}

fn cluster_dashboard_row(cfg: &Config, tag: &str, c: &Cluster, is_dry: bool) -> DashboardCluster {
    let mut torrent_names: Vec<String> = c.torrents.iter().map(|t| t.name.clone()).collect();
    torrent_names.sort();
    let name = c
        .torrents
        .iter()
        .min_by_key(|t| t.added_on)
        .map(|t| t.name.clone())
        .unwrap_or_else(|| "Unknown cluster".to_string());
    let mut tracker_hosts: Vec<String> = c
        .torrents
        .iter()
        .map(|t| tracker_host(&t.tracker))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    tracker_hosts.sort();
    let (status, reason) = match tag {
        "(DEL-CAP)" => (
            (if is_dry { "WOULD DELETE" } else { "DELETED" }).to_string(),
            Some("Pool capacity".to_string()),
        ),
        "(DEL-DISK)" => (
            (if is_dry { "WOULD DELETE" } else { "DELETED" }).to_string(),
            Some("Disk watermark".to_string()),
        ),
        "(FREE)" => ("FREE".to_string(), None),
        "(GRACE)" => ("GRACE".to_string(), None),
        _ => ("GOOD".to_string(), None),
    };
    DashboardCluster {
        rank: c.rank,
        name,
        torrent_names,
        tracker_hosts,
        pool: cfg.pools[c.pool].name.clone(),
        status,
        reason,
        priority: c.priority,
        score: c.score.is_finite().then_some(c.score),
        cost_bytes: c.cost,
        data_bytes: c.data_bytes,
        uploaded_bytes: c.uploaded,
        seeding_days: c.days as f64,
        torrent_count: c.torrents.len(),
        cross_seed_count: c.torrents.iter().filter(|t| t.is_cross()).count(),
    }
}

fn deletion_batch(
    cfg: &Config,
    dropped: &[(&str, Cluster)],
    timestamp: u64,
    observed_reclaimed_bytes: Option<u64>,
) -> DeletionBatch {
    let clusters = dropped
        .iter()
        .map(|(tag, c)| {
            let mut torrent_names: Vec<String> =
                c.torrents.iter().map(|t| t.name.clone()).collect();
            torrent_names.sort();
            let mut tracker_hosts: Vec<String> = c
                .torrents
                .iter()
                .map(|t| tracker_host(&t.tracker))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            tracker_hosts.sort();
            DeletionEvent {
                name: c
                    .torrents
                    .iter()
                    .min_by_key(|t| t.added_on)
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| "Unknown cluster".to_string()),
                torrent_names,
                tracker_hosts,
                pool: cfg.pools[c.pool].name.clone(),
                reason: if *tag == "(DEL-CAP)" {
                    "Pool capacity".to_string()
                } else {
                    "Disk watermark".to_string()
                },
                priority: c.priority,
                score: c.score.is_finite().then_some(c.score),
                estimated_reclaimed_bytes: c.cost,
                uploaded_bytes: c.uploaded,
                seeding_days: c.days as f64,
            }
        })
        .collect();
    let estimated_reclaimed_bytes = dropped
        .iter()
        .map(|(_, c)| c.cost)
        .fold(0_u64, u64::saturating_add);
    DeletionBatch {
        batch_id: format!("{timestamp}"),
        timestamp,
        estimated_reclaimed_bytes,
        observed_reclaimed_bytes,
        clusters,
    }
}

fn tracker_rows(trackers: HashMap<String, TrackerAnalytics>) -> Vec<TrackerRow> {
    let mut rows: Vec<TrackerRow> = trackers
        .into_iter()
        .map(|(tracker, values)| TrackerRow {
            tracker,
            total: values.total,
            regular: values.regular,
            cross_seed: values.cross_seed,
        })
        .collect();
    rows.sort_by(|a, b| {
        b.total
            .uploaded_bytes
            .cmp(&a.total.uploaded_bytes)
            .then(a.tracker.cmp(&b.tracker))
    });
    rows
}

fn all_time_period(state: &AnalyticsState) -> AnalyticsPeriod {
    let torrents = state
        .torrents
        .iter()
        .map(|(hash, t)| LeaderboardEntry {
            hash: hash.clone(),
            name: t.name.clone(),
            tracker: t.tracker.clone(),
            category: t.category.clone(),
            is_cross_seed: t.category.ends_with(CROSS_SUFFIX),
            uploaded_bytes: t.all_time_uploaded_bytes,
            downloaded_bytes: t.all_time_downloaded_bytes,
            timestamp: t.last_seen_at,
        })
        .collect();
    AnalyticsPeriod {
        trackers: tracker_rows(state.trackers.clone()),
        torrents,
    }
}

fn activity_period(events: &[ActivityEvent], start: u64, now: u64) -> AnalyticsPeriod {
    let mut trackers: HashMap<String, TrackerAnalytics> = HashMap::new();
    let mut torrents: HashMap<String, LeaderboardEntry> = HashMap::new();
    let mut seen = HashSet::new();
    for event in events {
        let identity = if event.event_id.is_empty() {
            format!(
                "legacy:{}:{}:{}:{}",
                event.timestamp, event.hash, event.uploaded_bytes, event.downloaded_bytes
            )
        } else {
            event.event_id.clone()
        };
        if event.timestamp < start || event.timestamp > now || !seen.insert(identity) {
            continue;
        }
        add_tracker_totals(
            &mut trackers,
            &event.tracker,
            event.is_cross_seed,
            event.uploaded_bytes,
            event.downloaded_bytes,
        );
        let torrent = torrents
            .entry(event.hash.clone())
            .or_insert_with(|| LeaderboardEntry {
                hash: event.hash.clone(),
                name: event.name.clone(),
                tracker: event.tracker.clone(),
                category: event.category.clone(),
                is_cross_seed: event.is_cross_seed,
                uploaded_bytes: 0,
                downloaded_bytes: 0,
                timestamp: event.timestamp,
            });
        torrent.name = event.name.clone();
        torrent.tracker = event.tracker.clone();
        torrent.category = event.category.clone();
        torrent.is_cross_seed = event.is_cross_seed;
        torrent.uploaded_bytes = torrent.uploaded_bytes.saturating_add(event.uploaded_bytes);
        torrent.downloaded_bytes = torrent
            .downloaded_bytes
            .saturating_add(event.downloaded_bytes);
        torrent.timestamp = torrent.timestamp.max(event.timestamp);
    }
    AnalyticsPeriod {
        trackers: tracker_rows(trackers),
        torrents: torrents.into_values().collect(),
    }
}

fn build_periods(
    state: &AnalyticsState,
    stored_events: &[ActivityEvent],
    current_events: &[ActivityEvent],
    now: u64,
) -> HashMap<String, AnalyticsPeriod> {
    let mut events = stored_events.to_vec();
    // In --dry mode current deltas are not persisted, but showing them in the generated page
    // makes the preview accurately reflect what this run observed.
    events.extend_from_slice(current_events);
    let (week, month, year) = period_starts(now);
    HashMap::from([
        ("all".to_string(), all_time_period(state)),
        ("year".to_string(), activity_period(&events, year, now)),
        ("month".to_string(), activity_period(&events, month, now)),
        ("week".to_string(), activity_period(&events, week, now)),
    ])
}

fn history_snapshot(
    cfg: &Config,
    timestamp: u64,
    disk_used: u64,
    disk_total: u64,
    projected_used: u64,
    estimated_reclaimed: u64,
    actual_reclaimed: Option<u64>,
    pools: Vec<PoolSnapshot>,
    reclaimable_bytes: u64,
    protected_bytes: u64,
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
        high_watermark_percent: cfg.high_watermark * 100.0,
        low_watermark_percent: cfg.low_watermark * 100.0,
        reclaimable_bytes,
        protected_bytes,
        estimated_reclaimed_bytes: estimated_reclaimed,
        actual_reclaimed_bytes: actual_reclaimed,
        pools,
    }
}

fn html_safe_json<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_string(value).unwrap_or_else(|e| {
        eprintln!("Cannot serialize dashboard data: {e}");
        exit(1);
    });
    json.replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn generate_dashboard(data: &DashboardData) {
    let json = html_safe_json(data);
    let html = DASHBOARD_HTML.replace("__GRAINE_DATA__", &json);
    atomic_write(DASHBOARD_FILE, html.as_bytes());
    atomic_write("/var/www/graine/style.css", DASHBOARD_CSS.as_bytes());
    atomic_write("/var/www/graine/app.js", DASHBOARD_JS.as_bytes());
}

const DASHBOARD_HTML: &str = include_str!("dashboard/index.html");
const DASHBOARD_CSS: &str = include_str!("dashboard/style.css");
const DASHBOARD_JS: &str = include_str!("dashboard/app.js");

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

    let cfg_text = fs::read_to_string(&config_path).unwrap_or_else(|e| {
        eprintln!("Cannot read {config_path}: {e}");
        exit(1)
    });
    let cfg: Config = toml::from_str(&cfg_text).unwrap_or_else(|e| {
        eprintln!("Bad config: {e}");
        exit(1)
    });
    if cfg.pools.is_empty()
        || cfg.low_watermark >= cfg.high_watermark
        || !(0.0..=1.0).contains(&cfg.low_watermark)
        || !(0.0..=1.0).contains(&cfg.high_watermark)
    {
        eprintln!("Bad config: need pools and 0 <= low_watermark < high_watermark <= 1");
        exit(1);
    }

    let Ok(pw) = env::var("QBIT_PW") else {
        eprintln!("Error: QBIT_PW environment variable is missing.");
        exit(1);
    };
    let conn = ConnectionInfo {
        agent: ureq::agent(),
        url: cfg.url.clone(),
    };
    login(&conn, &pw);

    let managed: Vec<Torrent> = get_torrents(&conn)
        .into_iter()
        .filter(|t| pool_of(&cfg, &t.category).is_some())
        .collect();
    let managed_count = managed.len();
    let (analytics, current_activity) = track_analytics(&managed, is_dry);
    let stored_activity: Vec<ActivityEvent> = read_jsonl(ACTIVITY_FILE);

    // Only completed torrents are ranked. Incomplete ones remain under Cleanuparr's control.
    let complete: Vec<Torrent> = managed
        .iter()
        .filter(|t| t.progress >= 1.0)
        .cloned()
        .collect();
    let complete_count = complete.len();
    let mut entries: Vec<Entry> = Vec::new();
    for torrent in complete {
        if let Some(files) = torrent_files(&conn, &torrent) {
            entries.push(Entry { torrent, files });
        }
    }

    // Union-find merges chains such as original -> cross-seed A -> cross-seed B.
    let mut parent: Vec<usize> = (0..entries.len()).collect();
    let mut owner: HashMap<(u64, u64), usize> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        for file in &entry.files {
            let key = (file.dev, file.ino);
            if let Some(j) = owner.get(&key).copied() {
                let a = find(&mut parent, i);
                let b = find(&mut parent, j);
                parent[a] = b;
            } else {
                owner.insert(key, i);
            }
        }
    }
    let mut by_root: HashMap<usize, Vec<Entry>> = HashMap::new();
    for (i, entry) in entries.into_iter().enumerate() {
        let root = find(&mut parent, i);
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

    // Step 1: enforce per-pool capacity (same decision logic as before).
    let mut pool_used_for_cap: Vec<u64> = vec![0; cfg.pools.len()];
    let mut kept: Vec<(&str, Cluster)> = Vec::new();
    let mut dropped: Vec<(&str, Cluster)> = Vec::new();
    for cluster in clusters {
        if cluster.cost == 0 {
            kept.push(("(FREE)", cluster));
            continue;
        }
        if cluster.is_grace {
            pool_used_for_cap[cluster.pool] =
                pool_used_for_cap[cluster.pool].saturating_add(cluster.cost);
            kept.push(("(GRACE)", cluster));
            continue;
        }
        let over = cfg.pools[cluster.pool].capacity_gib.is_some_and(|g| {
            pool_used_for_cap[cluster.pool].saturating_add(cluster.cost) > g.saturating_mul(GIB)
        });
        if over {
            dropped.push(("(DEL-CAP)", cluster));
        } else {
            pool_used_for_cap[cluster.pool] =
                pool_used_for_cap[cluster.pool].saturating_add(cluster.cost);
            kept.push(("(GOOD)", cluster));
        }
    }

    // Step 2: apply the global watermark, counting only bytes a deletion can really free.
    let (used_before, total) = disk_usage(&cfg.disk_path);
    let capacity_reclaimed: u64 = dropped
        .iter()
        .map(|(_, c)| c.cost)
        .fold(0_u64, u64::saturating_add);
    let mut projected = used_before.saturating_sub(capacity_reclaimed);
    let high = (cfg.high_watermark * total as f64) as u64;
    let low = (cfg.low_watermark * total as f64) as u64;
    let mut evict: HashSet<usize> = HashSet::new();
    if projected > high {
        let mut idx = kept.len();
        while idx > 0 && projected > low {
            idx -= 1;
            let cluster = &kept[idx].1;
            if cluster.is_grace || cluster.cost == 0 {
                continue;
            }
            projected = projected.saturating_sub(cluster.cost);
            evict.insert(idx);
        }
    }
    let estimated_reclaimed = used_before.saturating_sub(projected);
    let over_limit = projected > high;
    let mut final_kept: Vec<(&str, Cluster)> = Vec::new();
    for (index, (tag, cluster)) in kept.into_iter().enumerate() {
        if evict.contains(&index) {
            dropped.push(("(DEL-DISK)", cluster));
        } else {
            final_kept.push((tag, cluster));
        }
    }

    let timestamp = now_unix();
    let mut ranking: Vec<DashboardCluster> = final_kept
        .iter()
        .map(|(tag, c)| cluster_dashboard_row(&cfg, tag, c, is_dry))
        .chain(
            dropped
                .iter()
                .map(|(tag, c)| cluster_dashboard_row(&cfg, tag, c, is_dry)),
        )
        .collect();
    ranking.sort_by_key(|c| c.rank);

    // Pool and free/protected totals describe the clusters that remain after this run.
    let mut pool_reclaimable = vec![0_u64; cfg.pools.len()];
    let mut pool_protected = vec![0_u64; cfg.pools.len()];
    for (_, cluster) in &final_kept {
        pool_reclaimable[cluster.pool] =
            pool_reclaimable[cluster.pool].saturating_add(cluster.cost);
        pool_protected[cluster.pool] = pool_protected[cluster.pool]
            .saturating_add(cluster.data_bytes.saturating_sub(cluster.cost));
    }
    let reclaimable_bytes = pool_reclaimable
        .iter()
        .copied()
        .fold(0_u64, u64::saturating_add);
    let protected_bytes = pool_protected
        .iter()
        .copied()
        .fold(0_u64, u64::saturating_add);
    let pools: Vec<PoolSnapshot> = cfg
        .pools
        .iter()
        .enumerate()
        .map(|(i, pool)| PoolSnapshot {
            name: pool.name.clone(),
            reclaimable_bytes: pool_reclaimable[i],
            protected_bytes: pool_protected[i],
            capacity_bytes: pool.capacity_gib.map(|g| g.saturating_mul(GIB)),
        })
        .collect();

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
    for (i, pool) in cfg.pools.iter().enumerate() {
        let cap = pool
            .capacity_gib
            .map_or("unlimited".to_string(), |g| format!("{g} GiB cap"));
        println!(
            "pool {}: {:.2} GiB reclaimable ({cap})",
            pool.name,
            pool_reclaimable[i] as f64 / GIB as f64
        );
    }
    if over_limit {
        eprintln!("WARNING: still above the high watermark; no eligible clusters remain");
    }

    let mut actual_reclaimed = None;
    let mut disk_after = (used_before, total);
    let mut deletions: Vec<DeletionBatch> = read_jsonl(DELETIONS_FILE);
    if !dropped.is_empty() && !is_dry {
        let to_delete: Vec<Torrent> = dropped
            .iter()
            .flat_map(|(_, cluster)| cluster.torrents.iter().cloned())
            .collect();
        delete_torrents(&conn, &to_delete);
        disk_after = disk_usage(&cfg.disk_path);
        actual_reclaimed = Some(used_before.saturating_sub(disk_after.0));
        let batch = deletion_batch(&cfg, &dropped, timestamp, actual_reclaimed);
        append_jsonl(DELETIONS_FILE, std::slice::from_ref(&batch));
        deletions.push(batch);
        println!(
            "Deleted {} torrents; estimated {:.2} GB, observed {:.2} GB reclaimed.",
            to_delete.len(),
            estimated_reclaimed as f64 / GIB as f64,
            actual_reclaimed.unwrap_or(0) as f64 / GIB as f64
        );
    } else if !dropped.is_empty() {
        println!(
            "Dry run: {} torrents would be deleted.",
            dropped.iter().map(|(_, c)| c.torrents.len()).sum::<usize>()
        );
    } else {
        println!("Nothing to delete");
        if !is_dry {
            actual_reclaimed = Some(0);
        }
    }

    let (used_after, total_after) = disk_after;
    let snapshot = history_snapshot(
        &cfg,
        timestamp,
        used_after,
        total_after,
        projected,
        estimated_reclaimed,
        actual_reclaimed,
        pools.clone(),
        reclaimable_bytes,
        protected_bytes,
    );
    let mut history: Vec<HistorySnapshot> = read_jsonl(HISTORY_FILE);
    if !is_dry {
        append_jsonl(HISTORY_FILE, std::slice::from_ref(&snapshot));
    }
    history.push(snapshot.clone());
    // Retain the complete history on disk, but keep the generated HTML a reasonable size.
    if history.len() > 1500 {
        let excess = history.len() - 1500;
        history.drain(0..excess);
    }
    if deletions.len() > 250 {
        let excess = deletions.len() - 250;
        deletions.drain(0..excess);
    }

    let mut periods = build_periods(&analytics, &stored_activity, &current_activity, timestamp);
    // Ensure all four period keys are present, even on a completely new installation.
    for key in ["all", "year", "month", "week"] {
        periods.entry(key.to_string()).or_insert(AnalyticsPeriod {
            trackers: Vec::new(),
            torrents: Vec::new(),
        });
    }
    let run = RunSummary {
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
        pools,
        reclaimable_bytes,
        protected_bytes,
        ranking,
        periods,
        deletions,
    };
    generate_dashboard(&dashboard);
    println!("Dashboard updated: {DASHBOARD_FILE}");

    if over_limit {
        exit(2);
    }
}
