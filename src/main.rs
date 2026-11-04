use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    env, fmt, fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, exit},
    time::{SystemTime, UNIX_EPOCH},
};
use ureq::Agent;

const DAY_IN_SEC: u64 = 60 * 60 * 24;
const GIB: u64 = 1024 * 1024 * 1024;
const CROSS_SUFFIX: &str = ".cross";
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

#[derive(Deserialize)]
struct Torrent {
    hash: String,
    name: String,
    category: String,
    save_path: String,
    progress: f64,
    seeding_time: u64,
    added_on: u64,
    last_activity: u64,
    size: u64,
    uploaded: u64,
    downloaded: u64,
    tracker: String,
}

#[derive(Deserialize)]
struct ApiFile {
    name: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Stats {
    week: u64,
    trackers: HashMap<String, i64>,
    torrents: HashMap<String, i64>,
}

fn elapsed(time: u64) -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .saturating_sub(time)
}

fn get_sunday_midnight() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let days = now / DAY_IN_SEC;
    now - (((days + 4) % 7) * DAY_IN_SEC) - (now % DAY_IN_SEC)
}

impl fmt::Display for Torrent {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "[{name}...]\t{cat}\t| Size: {size:.2} GB\t| Up: {up:.2} GB\t| Days: {days:.1}",
            name = self.name.chars().take(40).collect::<String>(),
            cat = self.category,
            size = self.size as f64 / GIB as f64,
            up = self.uploaded as f64 / GIB as f64,
            days = self.seeding_time as f32 / DAY_IN_SEC as f32,
        )
    }
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
        .unwrap()
        .body_mut()
        .read_json::<Vec<Torrent>>()
        .unwrap()
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

    let text = String::from_utf8_lossy(&out.stdout);
    let parsed = text.lines().nth(1).and_then(|line| {
        let mut it = line.split_whitespace().map(|v| v.parse::<u64>());
        match (it.next(), it.next()) {
            (Some(Ok(used)), Some(Ok(avail))) => Some((used, used + avail)),
            _ => None,
        }
    });

    parsed.unwrap_or_else(|| {
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
            // Files that were never downloaded (unwanted files set to "do not download",
            // junk removed by Cleanuparr...) take no space, so they cost nothing.
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

fn track_weekly_stats(torrents: &[Torrent], is_dry: bool) {
    let current_sunday = get_sunday_midnight();

    let mut stats: Stats = fs::read_to_string("stats.json")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    if stats.week != current_sunday {
        stats.week = current_sunday;
        stats.trackers.clear();
    }

    let mut current_torrents = HashMap::new();

    for t in torrents {
        let net = t.uploaded as i64 - t.downloaded as i64;
        let diff = match stats.torrents.get(&t.hash) {
            Some(&prev) => net - prev,
            None if t.added_on >= current_sunday => net,
            None => 0,
        };

        *stats.trackers.entry(t.tracker.clone()).or_default() += diff;
        current_torrents.insert(t.hash.clone(), net);
    }

    stats.torrents = current_torrents;

    for (tracker, net) in &stats.trackers {
        let tracker = tracker.split("/").nth(2).unwrap_or(tracker);

        println!("{tracker}:\t{:+.2} GB", *net as f64 / 1_073_741_824.0);
    }

    if let Ok(json) = serde_json::to_string(&stats)
        && !is_dry
    {
        let _ = fs::write("stats.json", json);
    }
}

/// Torrents that share files on disk (same inode). Space only comes back when every
/// member goes, so the cluster is what gets ranked, kept or deleted.
struct Cluster {
    torrents: Vec<Torrent>,
    pool: usize,
    /// Highest tracker priority among the members: keeping the cluster serves that tracker.
    priority: i32,
    /// Bytes a delete would actually free: files whose every hardlink belongs to the cluster.
    /// 0 means something outside (typically the library) keeps the data alive.
    cost: u64,
    uploaded: u64,
    days: f32,
    is_grace: bool,
    /// Upload per freed byte per day, decayed by inactivity. Infinite when cost is 0.
    score: f32,
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn build_cluster(cfg: &Config, entries: Vec<Entry>) -> Cluster {
    // inode -> (hardlink count, size, distinct paths referenced by the cluster)
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
    let cost: u64 = inodes
        .values()
        .filter(|(nlink, _, paths)| paths.len() as u64 >= *nlink)
        .map(|(_, size, _)| *size)
        .sum();

    let uploaded: u64 = entries.iter().map(|e| e.torrent.uploaded).sum();

    let seeding_time = entries
        .iter()
        .map(|e| e.torrent.seeding_time)
        .max()
        .unwrap_or(0);
    let days = seeding_time as f32 / DAY_IN_SEC as f32;

    let last_activity = entries
        .iter()
        .map(|e| e.torrent.last_activity)
        .max()
        .unwrap_or(0);
    let days_inactive = elapsed(last_activity) / DAY_IN_SEC;
    let decay = 0.5_f32.powf(days_inactive as f32 / 3.0);

    // The oldest member decides which pool the cluster belongs to.
    let oldest = entries.iter().min_by_key(|e| e.torrent.added_on).unwrap();
    let pool = pool_of(cfg, &oldest.torrent.category).unwrap_or(0);
    let grace_secs = cfg.pools[pool].grace_days * DAY_IN_SEC;

    // Grace follows the non-cross members: a cross-seed added late must not extend the
    // protection. With no original left, the cross-seeds act as the original.
    let anchors: Vec<&Entry> = entries.iter().filter(|e| !e.torrent.is_cross()).collect();
    let is_grace = if anchors.is_empty() {
        entries.iter().any(|e| e.torrent.seeding_time < grace_secs)
    } else {
        anchors.iter().any(|e| e.torrent.seeding_time < grace_secs)
    };

    let score = if cost == 0 {
        f32::INFINITY
    } else {
        (uploaded as f32 / cost as f32) / days.max(1.0) * decay
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
        uploaded,
        days,
        is_grace,
        score,
    }
}

fn print_cluster(cfg: &Config, tag: &str, c: &Cluster) {
    let score = if c.score.is_finite() {
        format!("{:.2}", c.score)
    } else {
        "-".to_string()
    };
    println!(
        "{tag}\t{} | Prio: {} | Score: {score} | Cost: {:.2} GB | Up: {:.2} GB | Days: {:.1}",
        cfg.pools[c.pool].name,
        c.priority,
        c.cost as f64 / GIB as f64,
        c.uploaded as f64 / GIB as f64,
        c.days
    );
    for t in &c.torrents {
        println!("\t  {t}");
    }
}

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
    if cfg.pools.is_empty() || cfg.low_watermark >= cfg.high_watermark {
        eprintln!("Bad config: need at least one pool and low_watermark < high_watermark");
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

    println!("--- WEEKLY SUMMARY ---");
    track_weekly_stats(&managed, is_dry);

    // Only completed torrents are ranked. Incomplete ones (stuck, stalled, failed
    // import) are left alone for Cleanuparr.
    let complete: Vec<Torrent> = managed.into_iter().filter(|t| t.progress >= 1.0).collect();

    let mut entries: Vec<Entry> = vec![];
    for t in complete {
        if let Some(files) = torrent_files(&conn, &t) {
            entries.push(Entry { torrent: t, files });
        }
    }

    // Link torrents that share an inode (union-find), so chains like
    // original -> cross-seed A -> cross-seed B land in one cluster.
    let mut parent: Vec<usize> = (0..entries.len()).collect();
    let mut owner: HashMap<(u64, u64), usize> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        for f in &e.files {
            let key = (f.dev, f.ino);
            let existing = owner.get(&key).copied();
            match existing {
                Some(j) => {
                    let a = find(&mut parent, i);
                    let b = find(&mut parent, j);
                    parent[a] = b;
                }
                None => {
                    owner.insert(key, i);
                }
            }
        }
    }

    let mut by_root: HashMap<usize, Vec<Entry>> = HashMap::new();
    for (i, e) in entries.into_iter().enumerate() {
        let root = find(&mut parent, i);
        by_root.entry(root).or_default().push(e);
    }

    let mut clusters: Vec<Cluster> = by_root
        .into_values()
        .map(|es| build_cluster(&cfg, es))
        .collect();

    // Best first: grace, then higher tracker priority, then higher score. Everything is
    // deleted from the back of this order, so low priority goes before any score matters.
    // Cost-0 clusters have an infinite score and are skipped below.
    clusters.sort_by(|a, b| {
        let key = |c: &Cluster| {
            (
                c.is_grace,
                c.priority,
                if c.is_grace { 0.0 } else { c.score },
            )
        };
        let (ag, ap, asc) = key(a);
        let (bg, bp, bsc) = key(b);
        bg.cmp(&ag).then(bp.cmp(&ap)).then(bsc.total_cmp(&asc))
    });

    // Step 1: per-pool capacity.
    let mut pool_used: Vec<u64> = vec![0; cfg.pools.len()];
    let mut kept: Vec<(&str, Cluster)> = vec![];
    let mut dropped: Vec<(&str, Cluster)> = vec![];

    for c in clusters {
        if c.cost == 0 {
            kept.push(("(FREE)", c));
            continue;
        }
        if c.is_grace {
            pool_used[c.pool] += c.cost;
            kept.push(("(GRACE)", c));
            continue;
        }
        let over = cfg.pools[c.pool]
            .capacity_gib
            .is_some_and(|g| pool_used[c.pool] + c.cost > g * GIB);
        if over {
            dropped.push(("(DEL-CAP)", c));
        } else {
            pool_used[c.pool] += c.cost;
            kept.push(("(GOOD)", c));
        }
    }

    // Step 2: global watermark. Evict the lowest score across all pools, counting only
    // the bytes each delete actually frees, until back under the low watermark.
    let (used, total) = disk_usage(&cfg.disk_path);
    let freed: u64 = dropped.iter().map(|(_, c)| c.cost).sum();
    let mut projected = used.saturating_sub(freed);
    let high = (cfg.high_watermark * total as f64) as u64;
    let low = (cfg.low_watermark * total as f64) as u64;

    let mut evict: HashSet<usize> = HashSet::new();
    if projected > high {
        let mut idx = kept.len();
        while idx > 0 && projected > low {
            idx -= 1;
            let c = &kept[idx].1;
            if c.is_grace || c.cost == 0 {
                continue;
            }
            projected = projected.saturating_sub(c.cost);
            evict.insert(idx);
        }
    }
    let over_limit = projected > high;

    let mut final_kept: Vec<(&str, Cluster)> = vec![];
    for (i, (tag, c)) in kept.into_iter().enumerate() {
        if evict.contains(&i) {
            dropped.push(("(DEL-DISK)", c));
        } else {
            final_kept.push((tag, c));
        }
    }

    println!("--- KEEPING ---");
    for (tag, c) in &final_kept {
        print_cluster(&cfg, tag, c);
    }

    println!("--- POOLS ---");
    for (i, p) in cfg.pools.iter().enumerate() {
        let cap = p
            .capacity_gib
            .map_or("no cap".to_string(), |g| format!("{g} GiB cap"));
        println!(
            "{}: {:.2} GB reclaimable ({cap})",
            p.name,
            pool_used[i] as f64 / GIB as f64
        );
    }
    println!(
        "Disk: {:.1}% used now, {:.1}% after deletions (evict above {:.0}%, down to {:.0}%)",
        used as f64 / total as f64 * 100.0,
        projected as f64 / total as f64 * 100.0,
        cfg.high_watermark * 100.0,
        cfg.low_watermark * 100.0
    );
    if over_limit {
        eprintln!(
            "WARNING: still above the high watermark with nothing left to evict \
             (everything else is in grace or backed by files outside graine's pools)"
        );
    }

    if dropped.is_empty() {
        println!("Nothing to delete");
        exit(if over_limit { 2 } else { 0 })
    }

    println!("--- DELETING ---");
    for (tag, c) in &dropped {
        print_cluster(&cfg, tag, c);
    }

    let to_delete: Vec<Torrent> = dropped.into_iter().flat_map(|(_, c)| c.torrents).collect();

    if !is_dry {
        delete_torrents(&conn, &to_delete);
    }

    println!("Deleted {} torrents.", to_delete.len());
    if over_limit {
        exit(2)
    }
}
