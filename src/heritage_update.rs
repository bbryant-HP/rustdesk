//! Heritage Help Desk: updates from the Heritage server, and the Heritage version number.
//!
//! An installed End user or Full client asks the Heritage server about once an hour (and at once
//! when an admin clicks Update in the console) whether a newer Heritage build is out. The server
//! answers with a notice signed by the key this install was built with (the same one that signs
//! its custom.txt and admins' commands), naming this device, the version, and the download's
//! SHA-256 and size. The client downloads that exact file from the server, checks it, waits until
//! nobody is connected, and installs it with RustDesk's own updater, which keeps the settings.
//! The console may run on plain HTTP, so nothing is trusted because of the connection: a forged
//! notice fails the signature, and a swapped download fails the hash.
//!
//! Technician (outgoing-only) installs have no service to do this, so they are updated by
//! reinstalling from the console.
//!
//! Kept in its own file so merging upstream RustDesk releases only touches a few lines.

/// Heritage's own version, year.month.build (the GitHub Actions run number), set by the build.
/// Empty in local builds, which then never update themselves.
pub const VERSION: &str = match option_env!("HERITAGE_VERSION") {
    Some(v) => v,
    None => "",
};

/// What the About page shows, e.g. "2026.11.12 (RustDesk 1.5.3)".
pub fn display_version() -> String {
    if VERSION.is_empty() {
        crate::VERSION.to_owned()
    } else {
        format!("{VERSION} (RustDesk {})", crate::VERSION)
    }
}

/// Year.month.build as numbers, so 2026.12.3 sorts after 2026.11.40.
fn parse_version(v: &str) -> Option<Vec<u64>> {
    let parts = v
        .split('.')
        .map(|p| p.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    (parts.len() == 3).then_some(parts)
}

fn is_newer(offered: &str, current: &str) -> bool {
    match (parse_version(offered), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Asks for a check now (the console's Update button), if this install checks at all.
pub fn check_now() {
    #[cfg(windows)]
    if let Some(tx) = imp::WAKE.lock().unwrap().as_ref() {
        tx.send(()).ok();
    }
}

#[cfg(windows)]
pub use imp::start;

#[cfg(windows)]
mod imp {
    use super::{is_newer, VERSION};
    use hbb_common::{
        bail,
        config::{self, Config, LocalConfig},
        log, ResultType,
    };
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::{
        io::{Read, Write},
        path::{Path, PathBuf},
        sync::{mpsc, Mutex},
        time::{Duration, Instant},
    };

    const NOTICE_PREFIX: &[u8] = b"heritage-update:";
    const FIRST_CHECK: Duration = Duration::from_secs(90);
    const CHECK_EVERY: Duration = Duration::from_secs(3600);
    const RETRY_BUSY: Duration = Duration::from_secs(600); // someone was connected
    const MIN_GAP: Duration = Duration::from_secs(120);
    const MAX_SIZE: u64 = 300 * 1024 * 1024;
    const MAX_ATTEMPTS: u32 = 3;
    // "<sha256>:<attempts>": a build that installs but somehow doesn't take is not retried forever.
    const ATTEMPTS_KEY: &str = "heritage-update-attempts";

    lazy_static::lazy_static! {
        pub(super) static ref WAKE: Mutex<Option<mpsc::Sender<()>>> = Mutex::new(None);
    }

    enum Next {
        Later,
        Soon,
    }

    /// Starts the hourly check. Called once by the installed service's server process.
    pub fn start() {
        let mut wake = WAKE.lock().unwrap();
        if wake.is_some() || VERSION.is_empty() || config::is_outgoing_only() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        *wake = Some(tx);
        std::thread::spawn(move || {
            std::thread::sleep(FIRST_CHECK);
            let mut last: Option<Instant> = None;
            loop {
                if let Some(t) = last {
                    if t.elapsed() < MIN_GAP {
                        std::thread::sleep(MIN_GAP - t.elapsed());
                    }
                }
                while rx.try_recv().is_ok() {} // several Update clicks are one check
                last = Some(Instant::now());
                let wait = match check_and_update() {
                    Ok(Next::Later) => CHECK_EVERY,
                    Ok(Next::Soon) => RETRY_BUSY,
                    Err(e) => {
                        log::warn!("Heritage update: {e}");
                        CHECK_EVERY
                    }
                };
                if let Err(mpsc::RecvTimeoutError::Disconnected) = rx.recv_timeout(wait) {
                    break;
                }
            }
        });
    }

    fn conn_type() -> &'static str {
        if config::is_incoming_only() {
            "incoming"
        } else if config::is_outgoing_only() {
            "outgoing"
        } else {
            "both"
        }
    }

    fn api_server() -> String {
        let url = crate::common::get_api_server(
            Config::get_option("api-server"),
            Config::get_option("custom-rendezvous-server"),
        );
        if url.is_empty() || crate::is_public(&url) {
            return "".to_owned();
        }
        url
    }

    fn check_and_update() -> ResultType<Next> {
        let api = api_server();
        if api.is_empty() {
            return Ok(Next::Later);
        }
        let id = Config::get_id();
        let mut body = json!({
            "id": id,
            "uuid": crate::encode64(hbb_common::get_uuid()),
            "type": conn_type(),
            "version": VERSION,
        });
        let client = crate::hbbs_http::create_http_client_with_url(&api);
        let rsp: Value = client
            .post(format!("{api}/api/heritage/update/check"))
            .json(&body)
            .timeout(Duration::from_secs(30))
            .send()?
            .error_for_status()?
            .json()?;
        let Some(signed) = rsp["update"].as_str() else {
            return Ok(Next::Later);
        };
        let notice = verify_notice(signed, &id)?;
        let version = notice["version"].as_str().unwrap_or_default().to_owned();
        let sha256 = notice["sha256"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let size = notice["size"].as_u64().unwrap_or(0);
        if !is_newer(&version, VERSION) {
            return Ok(Next::Later);
        }
        if sha256.len() != 64
            || !sha256.chars().all(|c| c.is_ascii_hexdigit())
            || size == 0
            || size > MAX_SIZE
        {
            bail!("the update notice for {version} is malformed");
        }
        let tried = attempts(&sha256);
        if tried >= MAX_ATTEMPTS {
            bail!("{version} was installed {tried} times and still isn't running; not trying again");
        }
        if !crate::updater::has_no_active_conns() {
            log::info!("Heritage update: {version} is ready; waiting until nobody is connected");
            return Ok(Next::Soon);
        }
        let file = std::env::temp_dir().join(format!("heritage-update-{}.exe", &sha256[..16]));
        if file_sha256(&file).as_deref() != Some(sha256.as_str()) {
            body["sha256"] = json!(sha256);
            download(&client, &format!("{api}/api/heritage/update/download"), &body, &file, &sha256, size)?;
        }
        // Checked again: a download can take minutes.
        if !crate::updater::has_no_active_conns() {
            return Ok(Next::Soon);
        }
        LocalConfig::set_option(ATTEMPTS_KEY.to_owned(), format!("{sha256}:{}", tried + 1));
        log::info!("Heritage update: installing {version} over {VERSION}");
        // Copies the file somewhere only admins can write, checks the hash again, and runs it
        // with --update, which keeps this install's ID, settings and custom.txt.
        crate::updater::update_new_version(false, &version, &file, &sha256);
        Ok(Next::Later)
    }

    fn attempts(sha256: &str) -> u32 {
        let saved = LocalConfig::get_option(ATTEMPTS_KEY);
        match saved.split_once(':') {
            Some((s, n)) if s == sha256 => n.parse().unwrap_or(0),
            _ => 0,
        }
    }

    fn verify_notice(signed: &str, id: &str) -> ResultType<Value> {
        use hbb_common::sodiumoxide::crypto::sign;
        let key = hbb_common::config::HARD_SETTINGS
            .read()
            .unwrap()
            .get("heritage-action-key")
            .cloned()
            .unwrap_or_default();
        if key.is_empty() {
            bail!("this install has no server key; reinstall it from the console");
        }
        let Some(pk) = hbb_common::base64::decode(&key)
            .ok()
            .and_then(|k| sign::PublicKey::from_slice(&k))
        else {
            bail!("the server key is unreadable");
        };
        let Ok(data) = hbb_common::base64::decode(signed) else {
            bail!("the update notice isn't base64");
        };
        let Ok(msg) = sign::verify(&data, &pk) else {
            bail!("refused an update notice with a bad signature");
        };
        let Some(body) = msg.as_slice().strip_prefix(NOTICE_PREFIX) else {
            bail!("refused a signed message that isn't an update notice");
        };
        let notice: Value = serde_json::from_slice(body)?;
        if notice["device"].as_str() != Some(id) {
            bail!("refused an update notice meant for another device");
        }
        if notice["type"].as_str() != Some(conn_type()) {
            bail!("refused an update notice for another client type");
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let exp = notice["exp"].as_i64().unwrap_or(0);
        if exp < now || exp > now + 7200 {
            bail!("refused an expired update notice");
        }
        Ok(notice)
    }

    fn file_sha256(path: &Path) -> Option<String> {
        let mut f = std::fs::File::open(path).ok()?;
        crate::update_hash::sha256_reader_hex(&mut f).ok()
    }

    fn download(
        client: &reqwest::blocking::Client,
        url: &str,
        body: &Value,
        file: &PathBuf,
        sha256: &str,
        size: u64,
    ) -> ResultType<()> {
        let mut rsp = client
            .post(url)
            .json(body)
            .timeout(Duration::from_secs(900))
            .send()?
            .error_for_status()?;
        let part = file.with_extension("part");
        let result = (|| -> ResultType<()> {
            let mut out = std::fs::File::create(&part)?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; 64 * 1024];
            let mut total = 0u64;
            loop {
                let n = rsp.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                if total > size {
                    bail!("the download is bigger than the notice said");
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n])?;
            }
            out.flush()?;
            if total != size {
                bail!("the download is {total} bytes, expected {size}");
            }
            if format!("{:x}", hasher.finalize()) != sha256 {
                bail!("the download doesn't match the notice's SHA-256");
            }
            Ok(())
        })();
        if result.is_err() {
            std::fs::remove_file(&part).ok();
            return result;
        }
        std::fs::remove_file(file).ok();
        std::fs::rename(&part, file)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("2026.12.3", "2026.11.40"));
        assert!(is_newer("2026.11.12", "2026.11.11"));
        assert!(!is_newer("2026.11.11", "2026.11.11"));
        assert!(!is_newer("2026.11.10", "2026.11.11"));
        assert!(!is_newer("2026.11", "2026.10.1"));
        assert!(!is_newer("2026.11.x", "2026.10.1"));
        assert!(!is_newer("2026.11.12", ""));
    }
}
