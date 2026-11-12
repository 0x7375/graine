use crate::model::{ApiFile, Torrent};
use std::process::exit;
use ureq::Agent;

pub(crate) struct ConnectionInfo {
    pub(crate) agent: Agent,
    pub(crate) url: String,
}

pub(crate) fn login(conn: &ConnectionInfo, password: &str) {
    match conn
        .agent
        .post(format!("{}/api/v2/auth/login", conn.url))
        .header("Referer", &conn.url)
        .send_form([("username", "admin"), ("password", password)])
    {
        Ok(response) => {
            if !response.headers().contains_key("set-cookie") {
                eprintln!("Login failed, did not get an auth cookie");
                exit(1);
            }
        }
        Err(error) => {
            eprintln!("Login failed: {error}");
            exit(1);
        }
    }
}

pub(crate) fn get_torrents(conn: &ConnectionInfo) -> Vec<Torrent> {
    conn.agent
        .get(format!("{}/api/v2/torrents/info", conn.url))
        .call()
        .unwrap_or_else(|error| {
            eprintln!("Cannot retrieve torrents from qBittorrent: {error}");
            exit(1);
        })
        .body_mut()
        .read_json::<Vec<Torrent>>()
        .unwrap_or_else(|error| {
            eprintln!("Cannot parse qBittorrent torrent list: {error}");
            exit(1);
        })
}

pub(crate) fn get_torrent_files(conn: &ConnectionInfo, hash: &str) -> Vec<ApiFile> {
    conn.agent
        .get(format!("{}/api/v2/torrents/files?hash={hash}", conn.url))
        .call()
        .and_then(|mut response| response.body_mut().read_json::<Vec<ApiFile>>())
        .unwrap_or_else(|error| {
            eprintln!("Failed to list files for torrent {hash}: {error}, aborting");
            exit(1);
        })
}

pub(crate) fn delete_torrents(conn: &ConnectionInfo, to_delete: &[Torrent]) {
    let hashes: Vec<String> = to_delete
        .iter()
        .map(|torrent| torrent.hash.clone())
        .collect();
    let response = conn
        .agent
        .post(format!("{}/api/v2/torrents/delete", conn.url))
        .header("Referer", &conn.url)
        .send_form([
            ("hashes", hashes.join("|")),
            ("deleteFiles", "true".to_string()),
        ]);

    if let Err(error) = response {
        eprintln!("Failed to delete torrents: {error}");
        exit(1);
    }
}
