use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env, fmt, fs,
    process::exit,
    time::{SystemTime, UNIX_EPOCH},
};
use ureq::Agent;

const DAY_IN_SEC: u64 = 60 * 60 * 24;
const GIB: u64 = 1024 * 1024 * 1024;

const GRACE_PERIOD_DAYS: u64 = 7;
const GRACE_PERIOD_SECONDS: u64 = GRACE_PERIOD_DAYS * DAY_IN_SEC;

const TARGET_CATEGORY: &str = "autobrr";
const CROSS_CATEGORY: &str = "autobrr.cross";

const TOTAL_CAPACITY_BYTES: u64 = 200 * GIB;

// true: a cross-seed whose original (same name + size) is gone is deleted right away.
// false: it is only evicted under capacity pressure (safer if qui renames torrents).
const DELETE_ORPHANS: bool = false;

#[derive(Deserialize, Debug)]
struct Torrent {
    hash: String,
    name: String,
    category: String,
    seeding_time: u64,
    added_on: u64,
    last_activity: u64,
    ratio: f32,
    size: u64,
    uploaded: u64,
    downloaded: u64,
    tracker: String,
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
        let score = if self.is_original() {
            format!("{:.2}", self.score())
        } else {
            "-".to_string()
        };

        write!(
            f,
            "[{name}...]\tSize: {size:.2} GB\t| Ratio: {ratio:.2}\t| Days: {days:.1}\t| Score: {score}",
            name = self.name.chars().take(40).collect::<String>(),
            size = self.size as f64 / GIB as f64,
            ratio = self.ratio,
            days = self.seeding_time as f32 / DAY_IN_SEC as f32,
        )
    }
}

impl Torrent {
    fn is_original(&self) -> bool {
        self.category == TARGET_CATEGORY
    }

    fn score(&self) -> f32 {
        let days_seeded = (self.seeding_time as f32 / DAY_IN_SEC as f32).max(1.0);
        let days_inactive = elapsed(self.last_activity) / DAY_IN_SEC;
        let decay = 0.5_f32.powf(days_inactive as f32 / 3.0);

        (self.ratio / days_seeded) * decay
    }
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

fn fetch_category(conn: &ConnectionInfo, category: &str) -> Vec<Torrent> {
    conn.agent
        .get(format!(
            "{}/api/v2/torrents/info?category={category}",
            conn.url
        ))
        .call()
        .unwrap()
        .body_mut()
        .read_json::<Vec<Torrent>>()
        .unwrap()
}

fn get_torrents(conn: &ConnectionInfo) -> Vec<Torrent> {
    let mut all = fetch_category(conn, TARGET_CATEGORY);
    all.extend(fetch_category(conn, CROSS_CATEGORY));
    all
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

/// An original plus the cross-seeds that share its data (same name and size).
/// Kept or deleted as one unit.
struct Group {
    torrents: Vec<Torrent>,
    originals: u64,
    is_grace: bool,
    score: f32,
    size: u64,
}

fn main() {
    let mut args = env::args();
    args.next();

    let (Some(url), dry, None) = (args.next(), args.next(), args.next()) else {
        eprintln!("Usage: QBIT_PW=<password> graine <url> [--dry]");
        exit(1);
    };

    let Ok(pw) = env::var("QBIT_PW") else {
        eprintln!("Error: QBIT_PW environment variable is missing.");
        exit(1);
    };

    let agent = ureq::agent();
    let conn = ConnectionInfo { agent, url };

    login(&conn, &pw);

    let torrents = get_torrents(&conn);

    println!("--- WEEKLY SUMMARY ---");
    let is_dry = dry == Some("--dry".to_string());
    track_weekly_stats(&torrents, is_dry);

    let mut by_content: HashMap<(String, u64), Vec<Torrent>> = HashMap::new();
    for t in torrents {
        by_content
            .entry((t.name.clone(), t.size))
            .or_default()
            .push(t);
    }

    let mut groups: Vec<Group> = by_content
        .into_values()
        .map(|torrents| {
            let originals: Vec<&Torrent> = torrents.iter().filter(|t| t.is_original()).collect();
            let n = originals.len() as u64;
            let is_grace = originals
                .iter()
                .any(|t| t.seeding_time < GRACE_PERIOD_SECONDS);
            let score = originals.iter().map(|t| t.score()).fold(0.0_f32, f32::max);
            let size = torrents[0].size * n.max(1);

            Group {
                originals: n,
                is_grace,
                score,
                size,
                torrents,
            }
        })
        .collect();

    groups.sort_by(|a, b| {
        let key = |g: &Group| if g.is_grace { f32::MAX } else { g.score };
        key(a).total_cmp(&key(b)).reverse()
    });

    let mut capacity: u64 = 0;
    let mut to_delete: Vec<Torrent> = vec![];

    println!("--- KEEPING ---");
    for group in groups {
        if group.is_grace {
            capacity += group.size;
            for t in &group.torrents {
                let tag = if t.is_original() {
                    "(GRACE)"
                } else {
                    "(CROSS)"
                };
                println!("{tag}\t{t}");
            }
            continue;
        }

        let orphan = group.originals == 0;

        if (orphan && DELETE_ORPHANS) || capacity + group.size > TOTAL_CAPACITY_BYTES {
            to_delete.extend(group.torrents);
        } else {
            capacity += group.size;
            for t in &group.torrents {
                let tag = if t.is_original() { "(GOOD)" } else { "(CROSS)" };
                println!("{tag}\t{t}");
            }
        }
    }

    println!(
        "Capacity used: {:.2} / {:.2} GB",
        capacity as f64 / GIB as f64,
        TOTAL_CAPACITY_BYTES as f64 / GIB as f64
    );

    if to_delete.is_empty() {
        println!("Nothing to delete");
        exit(0)
    }

    println!("--- DELETING ---");
    for torrent in &to_delete {
        println!("{}", torrent);
    }

    if !is_dry {
        delete_torrents(&conn, &to_delete);
    }

    println!("Deleted {} torrents.", to_delete.len());
}
