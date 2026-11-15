use crate::{
    model::{
        ActivityEvent, AnalyticsState, DashboardData, DeletionBatch, HistorySnapshot,
    },
    util::ANALYTICS_VERSION,
};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const LEGACY_IMPORT_KEY: &str = "legacy_jsonl_import_v1";
const ANALYTICS_KEY: &str = "analytics_state";

#[derive(Clone)]
pub(crate) struct Database {
    path: PathBuf,
    legacy_data_dir: PathBuf,
}

impl Database {
    pub(crate) fn new(path: PathBuf, legacy_data_dir: PathBuf) -> Self {
        Self {
            path,
            legacy_data_dir,
        }
    }

    pub(crate) fn initialize(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create database directory {}", parent.display()))?;
        }

        let mut connection = self.connect()?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS app_state (
                key TEXT PRIMARY KEY,
                json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS activity_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS activity_events_timestamp
                ON activity_events(timestamp);
            CREATE TABLE IF NOT EXISTS history_snapshots (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS history_snapshots_timestamp
                ON history_snapshots(timestamp);
            CREATE TABLE IF NOT EXISTS deletion_batches (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                batch_id TEXT NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS deletion_batches_timestamp
                ON deletion_batches(timestamp);
            CREATE TABLE IF NOT EXISTS latest_dashboard (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                generated_at INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            ",
        )?;

        self.import_legacy(&mut connection)?;
        Ok(())
    }

    fn connect(&self) -> Result<Connection> {
        let connection = Connection::open(&self.path)
            .with_context(|| format!("cannot open SQLite database {}", self.path.display()))?;
        connection.busy_timeout(Duration::from_secs(10))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(connection)
    }

    fn import_legacy(&self, connection: &mut Connection) -> Result<()> {
        let already_imported: Option<String> = connection
            .query_row(
                "SELECT value FROM metadata WHERE key = ?1",
                [LEGACY_IMPORT_KEY],
                |row| row.get(0),
            )
            .optional()?;
        if already_imported.is_some() {
            return Ok(());
        }

        // Parse every source file before opening the transaction. If any file is malformed,
        // startup fails and no import marker or partial data is written. The source files remain
        // untouched, so the operator can repair the offending record and retry safely.
        let analytics_path = self.legacy_data_dir.join("analytics.json");
        let activity_path = self.legacy_data_dir.join("activity.jsonl");
        let history_path = self.legacy_data_dir.join("history.jsonl");
        let deletions_path = self.legacy_data_dir.join("deletions.jsonl");
        // Do not mark an empty directory as imported. This lets developers start the app,
        // copy the server's history files afterwards, and restart without resetting the DB.
        if ![&analytics_path, &activity_path, &history_path, &deletions_path]
            .iter()
            .any(|path| path.exists())
        {
            tracing::debug!(legacy_dir = %self.legacy_data_dir.display(), "no legacy state files found; import deferred");
            return Ok(());
        }

        let analytics = if analytics_path.exists() {
            let text = fs::read_to_string(&analytics_path)
                .with_context(|| format!("cannot read {}", analytics_path.display()))?;
            let state: AnalyticsState = serde_json::from_str(&text)
                .with_context(|| format!("cannot parse {}", analytics_path.display()))?;
            if state.version != 0 && state.version != 1 && state.version != ANALYTICS_VERSION {
                bail!(
                    "unsupported analytics version {} in {}; refusing to import",
                    state.version,
                    analytics_path.display()
                );
            }
            Some(state)
        } else {
            None
        };

        let mut activity = read_legacy_jsonl::<ActivityEvent>(&activity_path)?;
        for event in &mut activity {
            if event.event_id.is_empty() {
                event.event_id = format!(
                    "legacy:{}:{}:{}:{}",
                    event.timestamp, event.hash, event.uploaded_bytes, event.downloaded_bytes
                );
            }
        }
        let history = read_legacy_jsonl::<HistorySnapshot>(&history_path)?;
        let deletions = read_legacy_jsonl::<DeletionBatch>(&deletions_path)?;

        let transaction = connection.transaction()?;
        if let Some(state) = analytics {
            transaction.execute(
                "INSERT OR IGNORE INTO app_state(key, json) VALUES (?1, ?2)",
                params![ANALYTICS_KEY, serde_json::to_string(&state)?],
            )?;
        }

        for event in &activity {
            transaction.execute(
                "INSERT OR IGNORE INTO activity_events(event_id, timestamp, payload) VALUES (?1, ?2, ?3)",
                params![
                    event.event_id,
                    event.timestamp,
                    serde_json::to_string(event)?
                ],
            )?;
        }

        for snapshot in &history {
            transaction.execute(
                "INSERT INTO history_snapshots(timestamp, payload) VALUES (?1, ?2)",
                params![snapshot.timestamp, serde_json::to_string(snapshot)?],
            )?;
        }

        for batch in &deletions {
            transaction.execute(
                "INSERT OR IGNORE INTO deletion_batches(batch_id, timestamp, payload) VALUES (?1, ?2, ?3)",
                params![batch.batch_id, batch.timestamp, serde_json::to_string(batch)?],
            )?;
        }

        transaction.execute(
            "INSERT INTO metadata(key, value) VALUES (?1, ?2)",
            params![LEGACY_IMPORT_KEY, "complete"],
        )?;
        transaction.commit()?;

        tracing::info!(
            activity_events = activity.len(),
            history_snapshots = history.len(),
            deletion_batches = deletions.len(),
            legacy_dir = %self.legacy_data_dir.display(),
            "legacy JSON/JSONL data imported into SQLite"
        );
        Ok(())
    }

    pub(crate) fn load_analytics(&self) -> Result<AnalyticsState> {
        let connection = self.connect()?;
        let json: Option<String> = connection
            .query_row(
                "SELECT json FROM app_state WHERE key = ?1",
                [ANALYTICS_KEY],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|value| serde_json::from_str(&value).context("cannot decode analytics state"))
            .transpose()
            .map(|value| value.unwrap_or_default())
    }

    pub(crate) fn load_activity(&self) -> Result<Vec<ActivityEvent>> {
        self.load_records(
            "SELECT payload FROM activity_events ORDER BY seq",
            "activity event",
        )
    }

    pub(crate) fn load_history(&self) -> Result<Vec<HistorySnapshot>> {
        self.load_records(
            "SELECT payload FROM history_snapshots ORDER BY seq",
            "history snapshot",
        )
    }

    pub(crate) fn load_deletions(&self) -> Result<Vec<DeletionBatch>> {
        self.load_records(
            "SELECT payload FROM deletion_batches ORDER BY seq",
            "deletion batch",
        )
    }

    fn load_records<T: DeserializeOwned>(&self, query: &str, label: &str) -> Result<Vec<T>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(query)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut records = Vec::new();
        for row in rows {
            let json = row?;
            records.push(
                serde_json::from_str(&json)
                    .with_context(|| format!("cannot decode stored {label}"))?,
            );
        }
        Ok(records)
    }

    pub(crate) fn persist_analytics(
        &self,
        analytics: &AnalyticsState,
        activity: &[ActivityEvent],
        is_dry: bool,
    ) -> Result<()> {
        if is_dry {
            return Ok(());
        }

        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO app_state(key, json) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET json = excluded.json",
            params![ANALYTICS_KEY, serde_json::to_string(analytics)?],
        )?;

        for event in activity {
            let event_id = if event.event_id.is_empty() {
                format!(
                    "legacy:{}:{}:{}:{}",
                    event.timestamp, event.hash, event.uploaded_bytes, event.downloaded_bytes
                )
            } else {
                event.event_id.clone()
            };
            transaction.execute(
                "INSERT OR IGNORE INTO activity_events(event_id, timestamp, payload) VALUES (?1, ?2, ?3)",
                params![event_id, event.timestamp, serde_json::to_string(event)?],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn save_run(
        &self,
        snapshot: &HistorySnapshot,
        deletion_batch: Option<&DeletionBatch>,
        dashboard: &DashboardData,
        is_dry: bool,
    ) -> Result<()> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;

        if !is_dry {
            transaction.execute(
                "INSERT INTO history_snapshots(timestamp, payload) VALUES (?1, ?2)",
                params![snapshot.timestamp, serde_json::to_string(snapshot)?],
            )?;

            if let Some(batch) = deletion_batch {
                transaction.execute(
                    "INSERT OR IGNORE INTO deletion_batches(batch_id, timestamp, payload) VALUES (?1, ?2, ?3)",
                    params![batch.batch_id, batch.timestamp, serde_json::to_string(batch)?],
                )?;
            }
        }

        transaction.execute(
            "INSERT INTO latest_dashboard(id, generated_at, payload) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET generated_at = excluded.generated_at, payload = excluded.payload",
            params![dashboard.generated_at, serde_json::to_string(dashboard)?],
        )?;
        transaction.commit()?;
        Ok(())
    }


    pub(crate) fn save_dashboard(&self, dashboard: &DashboardData) -> Result<()> {
        let connection = self.connect()?;
        connection.execute(
            "INSERT INTO latest_dashboard(id, generated_at, payload) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET generated_at = excluded.generated_at, payload = excluded.payload",
            params![dashboard.generated_at, serde_json::to_string(dashboard)?],
        )?;
        Ok(())
    }

    pub(crate) fn dashboard_json(&self) -> Result<Option<String>> {
        let connection = self.connect()?;
        Ok(connection
            .query_row(
                "SELECT payload FROM latest_dashboard WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

}

fn read_legacy_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
    };

    let mut records = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        records.push(serde_json::from_str(line).with_context(|| {
            format!(
                "invalid JSON in {} at line {}; migration stopped without changing source files",
                path.display(),
                index + 1
            )
        })?);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn test_directory() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("graine-db-test-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).expect("test directory should be created");
        path
    }

    #[test]
    fn imports_legacy_files_once_and_keeps_old_record_compatibility() {
        let root = test_directory();
        let legacy = root.join("legacy");
        fs::create_dir_all(&legacy).unwrap();

        fs::write(
            legacy.join("analytics.json"),
            json!({
                "version": ANALYTICS_VERSION,
                "tracking_started_at": 10,
                "trackers": {},
                "torrents": {}
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            legacy.join("activity.jsonl"),
            format!(
                "{}\n",
                json!({
                    "timestamp": 11,
                    "hash": "abc",
                    "name": "torrent",
                    "tracker": "tracker.example",
                    "category": "radarr",
                    "is_cross_seed": false,
                    "uploaded_bytes": 100,
                    "downloaded_bytes": 10
                })
            ),
        )
        .unwrap();
        fs::write(
            legacy.join("history.jsonl"),
            format!(
                "{}\n",
                json!({
                    "timestamp": 12,
                    "disk_used_bytes": 500,
                    "disk_total_bytes": 1_000,
                    "disk_used_percent": 50.0,
                    "projected_used_bytes": 500,
                    "projected_used_percent": 50.0,
                    "high_watermark_percent": 85.0,
                    "low_watermark_percent": 82.0,
                    "reclaimable_bytes": 100,
                    "protected_bytes": 0,
                    "estimated_reclaimed_bytes": 0,
                    "actual_reclaimed_bytes": 0
                })
            ),
        )
        .unwrap();
        fs::write(
            legacy.join("deletions.jsonl"),
            format!(
                "{}\n",
                json!({
                    "batch_id": "13",
                    "timestamp": 13,
                    "estimated_reclaimed_bytes": 100,
                    "observed_reclaimed_bytes": 90,
                    "clusters": [{
                        "name": "torrent",
                        "torrent_names": ["torrent"],
                        "tracker_hosts": ["tracker.example"],
                        "pool": "radarr",
                        "reason": "Disk watermark",
                        "priority": 1,
                        "score": 2.0,
                        "estimated_reclaimed_bytes": 100,
                        "uploaded_bytes": 200,
                        "seeding_days": 10.0
                    }]
                })
            ),
        )
        .unwrap();

        let database = Database::new(root.join("graine.sqlite3"), legacy);
        database.initialize().unwrap();
        database.initialize().unwrap();

        assert_eq!(database.load_analytics().unwrap().tracking_started_at, 10);
        let activity = database.load_activity().unwrap();
        assert_eq!(activity.len(), 1);
        assert!(activity[0].event_id.starts_with("legacy:"));
        assert_eq!(database.load_history().unwrap().len(), 1);
        let deletions = database.load_deletions().unwrap();
        assert_eq!(deletions.len(), 1);
        assert_eq!(deletions[0].clusters[0].category, "radarr");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_legacy_directory_does_not_block_later_import() {
        let root = test_directory();
        let legacy = root.join("legacy");
        fs::create_dir_all(&legacy).unwrap();
        let database = Database::new(root.join("graine.sqlite3"), legacy.clone());

        // First startup happens before the user has copied the server's files.
        database.initialize().unwrap();
        fs::write(
            legacy.join("activity.jsonl"),
            format!("{}\n", json!({
                "timestamp": 42,
                "hash": "late-copy",
                "name": "copied later",
                "tracker": "tracker.example",
                "category": "radarr",
                "is_cross_seed": false,
                "uploaded_bytes": 5,
                "downloaded_bytes": 1
            })),
        ).unwrap();

        database.initialize().unwrap();
        let events = database.load_activity().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].hash, "late-copy");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_legacy_jsonl_does_not_mark_import_complete_or_change_source() {
        let root = test_directory();
        let legacy = root.join("legacy");
        fs::create_dir_all(&legacy).unwrap();
        let activity_path = legacy.join("activity.jsonl");
        let original = "{malformed json}\n";
        fs::write(&activity_path, original).unwrap();

        let database = Database::new(root.join("graine.sqlite3"), legacy.clone());
        assert!(database.initialize().is_err());
        assert_eq!(fs::read_to_string(&activity_path).unwrap(), original);

        fs::remove_file(&activity_path).unwrap();
        database.initialize().unwrap();
        assert!(database.load_activity().unwrap().is_empty());

        fs::remove_dir_all(root).unwrap();
    }
}
