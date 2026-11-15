use crate::model::{ApiFile, Torrent};
use anyhow::{Context, Result, ensure, bail};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
};
use ureq::Agent;

pub(crate) enum ConnectionInfo {
    Api { agent: Agent, url: String },
    Fixture { torrents: Vec<Torrent>, files: HashMap<String, Vec<ApiFile>> },
}

#[derive(Deserialize)]
struct FixtureSnapshot {
    torrents: Vec<Torrent>,
    files: HashMap<String, Vec<ApiFile>>,
}

impl ConnectionInfo {
    pub(crate) fn new(
        url: &str,
        fixture_path: Option<&Path>,
        disk_path: &Path,
    ) -> Result<Self> {
        let Some(fixture_path) = fixture_path else {
            return Ok(Self::Api {
                agent: ureq::agent(),
                url: url.to_string(),
            });
        };

        let root = disk_path
            .canonicalize()
            .with_context(|| format!("cannot resolve fixture disk directory {}", disk_path.display()))?;
        let content = fs::read_to_string(fixture_path)
            .with_context(|| format!("cannot read qBittorrent fixture {}", fixture_path.display()))?;
        let mut fixture: FixtureSnapshot = serde_json::from_str(&content)
            .with_context(|| format!("cannot parse qBittorrent fixture {}", fixture_path.display()))?;

        for torrent in &mut fixture.torrents {
            let declared_path = PathBuf::from(&torrent.save_path);
            let candidate = if declared_path.is_absolute() {
                declared_path
            } else {
                root.join(declared_path)
            };
            let save_path = candidate.canonicalize().with_context(|| {
                format!("cannot resolve fixture save path {} for torrent {}", candidate.display(), torrent.name)
            })?;
            ensure!(
                save_path.starts_with(&root),
                "fixture save path for torrent {} escapes disk_path: {}",
                torrent.name,
                save_path.display()
            );
            ensure!(save_path.is_dir(), "fixture save path is not a directory: {}", save_path.display());
            torrent.save_path = save_path.to_string_lossy().into_owned();

            let files = fixture.files.get(&torrent.hash).with_context(|| {
                format!("fixture has no files entry for torrent {} ({})", torrent.name, torrent.hash)
            })?;
            for file in files {
                let relative = Path::new(&file.name);
                ensure!(
                    !relative.is_absolute()
                        && !relative.components().any(|component| component == Component::ParentDir),
                    "fixture file path must be relative and cannot contain '..': {}",
                    file.name
                );
                let full_path = save_path.join(relative);
                let canonical_file = full_path.canonicalize().with_context(|| {
                    format!("fixture file does not exist: {}", full_path.display())
                })?;
                ensure!(
                    canonical_file.starts_with(&root),
                    "fixture file escapes disk_path: {}",
                    canonical_file.display()
                );
                ensure!(canonical_file.is_file(), "fixture path is not a file: {}", canonical_file.display());
            }
        }

        Ok(Self::Fixture {
            torrents: fixture.torrents,
            files: fixture.files,
        })
    }

    pub(crate) fn is_fixture(&self) -> bool {
        matches!(self, Self::Fixture { .. })
    }
}

pub(crate) fn login(conn: &ConnectionInfo, password: &str) -> Result<()> {
    match conn {
        ConnectionInfo::Fixture { .. } => Ok(()),
        ConnectionInfo::Api { agent, url } => {
            let response = agent
                .post(format!("{url}/api/v2/auth/login"))
                .header("Referer", url)
                .send_form([("username", "admin"), ("password", password)])
                .context("qBittorrent login request failed")?;

            if !response.headers().contains_key("set-cookie") {
                bail!("qBittorrent login failed: response did not include an authentication cookie");
            }
            Ok(())
        }
    }
}

pub(crate) fn get_torrents(conn: &ConnectionInfo) -> Result<Vec<Torrent>> {
    match conn {
        ConnectionInfo::Fixture { torrents, .. } => Ok(torrents.clone()),
        ConnectionInfo::Api { agent, url } => {
            let mut response = agent
                .get(format!("{url}/api/v2/torrents/info"))
                .call()
                .context("cannot retrieve torrents from qBittorrent")?;
            response
                .body_mut()
                .read_json::<Vec<Torrent>>()
                .context("cannot parse qBittorrent torrent list")
        }
    }
}

pub(crate) fn get_torrent_files(conn: &ConnectionInfo, hash: &str) -> Result<Vec<ApiFile>> {
    match conn {
        ConnectionInfo::Fixture { files, .. } => files
            .get(hash)
            .cloned()
            .with_context(|| format!("fixture has no file list for torrent {hash}")),
        ConnectionInfo::Api { agent, url } => {
            let mut response = agent
                .get(format!("{url}/api/v2/torrents/files?hash={hash}"))
                .call()
                .with_context(|| format!("failed to list files for torrent {hash}"))?;
            response
                .body_mut()
                .read_json::<Vec<ApiFile>>()
                .with_context(|| format!("cannot parse file list for torrent {hash}"))
        }
    }
}

pub(crate) fn delete_torrents(conn: &ConnectionInfo, to_delete: &[Torrent]) -> Result<()> {
    let ConnectionInfo::Api { agent, url } = conn else {
        bail!("refusing to delete torrents while using a local fixture");
    };
    let hashes: Vec<String> = to_delete
        .iter()
        .map(|torrent| torrent.hash.clone())
        .collect();
    agent
        .post(format!("{url}/api/v2/torrents/delete"))
        .header("Referer", url)
        .send_form([
            ("hashes", hashes.join("|")),
            ("deleteFiles", "true".to_string()),
        ])
        .context("failed to delete torrents through qBittorrent")?;
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn fixture_reads_local_files_and_refuses_deletion() {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("graine-fixture-test-{nonce}"));
        let disk = root.join("disk");
        let save = disk.join("radarr/movie");
        fs::create_dir_all(&save).unwrap();
        fs::write(save.join("film.mkv"), b"fixture").unwrap();
        let hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut files = serde_json::Map::new();
        files.insert(hash.to_string(), json!([{ "name": "film.mkv" }]));
        let snapshot = json!({
            "torrents": [{
                "hash": hash, "name": "film", "category": "radarr",
                "save_path": "radarr/movie", "progress": 1.0,
                "seeding_time": 1000000, "added_on": 1, "last_activity": 1,
                "uploaded": 10, "downloaded": 2, "tracker": "https://tracker.example/announce"
            }],
            "files": files
        });
        let fixture_path = root.join("qbittorrent.json");
        fs::write(&fixture_path, snapshot.to_string()).unwrap();

        let connection = ConnectionInfo::new("http://unused.invalid", Some(&fixture_path), &disk).unwrap();
        assert!(connection.is_fixture());
        let torrents = get_torrents(&connection).unwrap();
        assert_eq!(torrents.len(), 1);
        assert_eq!(get_torrent_files(&connection, hash).unwrap().len(), 1);
        assert!(delete_torrents(&connection, &torrents).is_err());

        fs::remove_dir_all(root).unwrap();
    }
}
