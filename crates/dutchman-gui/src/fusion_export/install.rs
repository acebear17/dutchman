//! Installing the Fusion exporter add-in: the pinned release is downloaded, its SHA-256 checked
//! before anything is written, and the add-in put into Fusion's AddIns folder the way the
//! exporter's own `scripts/install.py` does (unzip to a temporary folder beside the target,
//! replace the old `FusionBulkExport`, rename the new one into place).

use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The exporter release Dutchman installs: the one it was tested with.
#[derive(Clone, Copy, Debug)]
pub struct Pin {
    pub version: &'static str,
    pub zip_url: &'static str,
    /// Lowercase hex SHA-256 of the zip.
    pub sha256: &'static str,
}

/// The pinned exporter release: version, zip and hash change together, here only.
///
/// This is the protocol release's release candidate (rc.2). Its URL answers only once 0.4.0 is
/// released; the released zip is rebuilt, so replace `sha256` with the published
/// `FusionBulkExport-0.4.0.zip.sha256` then.
pub const PIN: Pin = Pin {
    version: "0.4.0",
    zip_url: "https://github.com/cad-lifeboat/fusion-bulk-export/releases/download/v0.4.0/FusionBulkExport-0.4.0.zip",
    sha256: "2dfc7e45d962fd971200a39e2eb1ab9ddf24f4804457d8af8a2dc2bb64b62a27",
};

/// Where the exporter announces its newest release; only used to say a newer one exists.
pub const LATEST_RELEASE_JSON: &str = "https://github.com/cad-lifeboat/fusion-bulk-export/releases/latest/download/FusionBulkExport-release.json";

/// The add-in's folder name.
pub const ADDIN: &str = "FusionBulkExport";
/// How long to wait to connect to the download host.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a download may stall before it is given up.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ZIP_BYTES: u64 = 64 << 20;
const MAX_UNPACKED_BYTES: u64 = 256 << 20;

/// Whether this computer can run Fusion, so the install is offered: Windows and macOS.
pub fn supported() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

/// Fusion's AddIns folder for this person.
pub fn addins_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
    };
    if cfg!(target_os = "macos") {
        var("HOME").map(|home| {
            home.join("Library/Application Support/Autodesk/Autodesk Fusion 360/API/AddIns")
        })
    } else if cfg!(windows) {
        var("APPDATA").map(|dir| dir.join(r"Autodesk\Autodesk Fusion 360\API\AddIns"))
    } else {
        None
    }
}

/// What to install and where.
#[derive(Clone, Debug)]
pub struct Options {
    pub version: String,
    pub zip_url: String,
    pub sha256: String,
    pub addins: PathBuf,
    pub connect_timeout: Duration,
    pub stall_timeout: Duration,
}

impl Options {
    /// The pinned release into this person's AddIns folder.
    pub fn pinned() -> Result<Self, Failure> {
        Ok(Self {
            version: PIN.version.into(),
            zip_url: PIN.zip_url.into(),
            sha256: PIN.sha256.into(),
            addins: addins_dir().ok_or(Failure::Unsupported)?,
            connect_timeout: CONNECT_TIMEOUT,
            stall_timeout: STALL_TIMEOUT,
        })
    }
}

/// Where an install is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Downloading,
    Checking,
    Installing,
}

/// Why nothing was installed (or what was left alone).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Unsupported,
    /// The exporter is exporting; replacing it now would break that.
    Busy,
    /// A newer exporter is already installed.
    Newer(String),
    Download(String),
    /// The download's hash is not the pinned one; nothing was written.
    Checksum,
    Write(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("Fusion does not run on this computer."),
            Self::Busy => f.write_str(
                "The Fusion exporter is exporting right now. Install it when the export is done.",
            ),
            Self::Newer(v) => write!(
                f,
                "A newer Fusion exporter ({v}) is already installed, so it was left alone."
            ),
            Self::Download(why) => write!(f, "Could not download the Fusion exporter: {why}"),
            Self::Checksum => f.write_str(
                "The download did not match its checksum, so nothing was installed. Try again.",
            ),
            Self::Write(why) => write!(f, "Could not install the Fusion exporter: {why}"),
        }
    }
}

/// A finished install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub version: String,
    pub folder: PathBuf,
}

/// Version of the add-in installed in `addins` (its `install-stamp.json`, else its manifest).
pub fn installed_version(addins: &Path) -> Option<String> {
    let folder = addins.join(ADDIN);
    for file in ["install-stamp.json", "FusionBulkExport.manifest"] {
        let version = std::fs::read_to_string(folder.join(file))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.get("version")?.as_str().map(str::to_owned));
        if version.is_some() {
            return version;
        }
    }
    None
}

/// Numeric parts of a dotted version such as `0.4.0`.
pub fn version_parts(version: &str) -> Option<Vec<u64>> {
    let core = version.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let parts: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

/// Whether version `a` is newer than `b`; false when either is unreadable.
pub fn is_newer(a: &str, b: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (version_parts(a), version_parts(b)) else {
        return false;
    };
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

/// Installs `options`. `busy` tells whether the exporter is exporting (its agent file's `busy`);
/// it is asked before anything is replaced, and again just before the swap. `progress` is told
/// each step. Nothing is written until the download matches its SHA-256.
pub fn install(
    options: &Options,
    busy: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Step),
) -> Result<Installed, Failure> {
    if busy() {
        return Err(Failure::Busy);
    }
    if let Some(have) = installed_version(&options.addins) {
        if is_newer(&have, &options.version) {
            return Err(Failure::Newer(have));
        }
    }
    progress(Step::Downloading);
    let zip = download(options)?;
    progress(Step::Checking);
    let got = hex(&Sha256::digest(&zip));
    if !got.eq_ignore_ascii_case(options.sha256.trim()) {
        return Err(Failure::Checksum);
    }
    progress(Step::Installing);
    let write = |e: &dyn fmt::Display| Failure::Write(e.to_string());
    std::fs::create_dir_all(&options.addins).map_err(|e| write(&e))?;
    let staging = tempfile::Builder::new()
        .prefix(".FusionBulkExport-install-")
        .tempdir_in(&options.addins)
        .map_err(|e| write(&e))?;
    let staged = staging.path().join(ADDIN);
    unpack(&zip, &staged).map_err(|e| write(&e))?;
    if !staged.join("FusionBulkExport.manifest").is_file() {
        return Err(Failure::Write("the download holds no add-in".into()));
    }
    if busy() {
        return Err(Failure::Busy);
    }
    let target = options.addins.join(ADDIN);
    let old = staging.path().join("old");
    let had_old = target.exists();
    if had_old {
        std::fs::rename(&target, &old).map_err(|e| write(&e))?;
    }
    if let Err(e) = std::fs::rename(&staged, &target) {
        if had_old {
            let _ = std::fs::rename(&old, &target);
        }
        return Err(write(&e));
    }
    // The old folder and the staging folder go when `staging` drops.
    Ok(Installed {
        version: installed_version(&options.addins).unwrap_or_else(|| options.version.clone()),
        folder: target,
    })
}

fn download(options: &Options) -> Result<Vec<u8>, Failure> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(options.connect_timeout)
        .timeout_read(options.stall_timeout)
        .build();
    let response = agent
        .get(&options.zip_url)
        .set(
            "User-Agent",
            concat!(
                "Dutchman-fusion-exporter-install/",
                env!("CARGO_PKG_VERSION")
            ),
        )
        .call()
        .map_err(|e| Failure::Download(short(&e.to_string())))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_ZIP_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Failure::Download(short(&e.to_string())))?;
    if bytes.len() as u64 > MAX_ZIP_BYTES {
        return Err(Failure::Download("the file is too large".into()));
    }
    Ok(bytes)
}

fn short(text: &str) -> String {
    text.chars().take(200).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpacks `<top>/FusionBulkExport/...` from the zip into `dest`.
fn unpack(zip: &[u8], dest: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        let parts: Vec<_> = path.components().collect();
        // Only the add-in folder inside the release folder.
        let (Some(first), true) = (parts.get(1), parts.len() >= 2) else {
            continue;
        };
        if first.as_os_str() != ADDIN {
            continue;
        }
        let relative: PathBuf = parts.iter().skip(2).collect();
        let out = dest.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        total += entry.size();
        if total > MAX_UNPACKED_BYTES {
            return Err("the add-in is larger than expected".into());
        }
        let mut file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut (&mut entry).take(MAX_UNPACKED_BYTES), &mut file)
            .map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The newest release the exporter's project announces: `(version, zip_url)`.
pub fn latest_release() -> Result<(String, String), String> {
    latest_release_at(LATEST_RELEASE_JSON)
}

fn latest_release_at(url: &str) -> Result<(String, String), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(STALL_TIMEOUT)
        .build();
    let response = agent.get(url).call().map_err(|e| short(&e.to_string()))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(1 << 16)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    match (text("version"), text("zip_url")) {
        (Some(v), Some(u)) if version_parts(&v).is_some() => Ok((v, u)),
        _ => Err("unreadable release information".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    /// A zip laid out like a release: `FusionBulkExport-<v>/FusionBulkExport/...`.
    fn release_zip(version: &str) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default();
        let top = format!("FusionBulkExport-{version}");
        let files = [
            ("README.txt".to_string(), "read me".to_string()),
            (
                "FusionBulkExport/FusionBulkExport.manifest".into(),
                format!("{{\"version\": \"{version}\"}}"),
            ),
            (
                "FusionBulkExport/install-stamp.json".into(),
                format!("{{\"version\": \"{version}\", \"sha256\": \"00\"}}"),
            ),
            ("FusionBulkExport/lib/a.py".into(), "print(1)".into()),
        ];
        for (name, body) in files {
            zip.start_file(format!("{top}/{name}"), opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    /// Serves `body` for any request on a loopback port until the test ends.
    fn serve(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = Arc::new(body);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let body = body.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    while reader.read_line(&mut line).is_ok() && line.trim() != "" {
                        line.clear();
                    }
                    let mut stream = stream;
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(&body);
                });
            }
        });
        format!("http://127.0.0.1:{port}/FusionBulkExport.zip")
    }

    fn options(addins: &Path, url: String, zip: &[u8], version: &str) -> Options {
        Options {
            version: version.into(),
            zip_url: url,
            sha256: sha(zip),
            addins: addins.to_path_buf(),
            connect_timeout: Duration::from_secs(2),
            stall_timeout: Duration::from_secs(2),
        }
    }

    fn run(options: &Options, busy: bool) -> Result<Installed, Failure> {
        install(options, &|| busy, &mut |_| {})
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn plant(addins: &Path, version: &str) {
        let folder = addins.join(ADDIN);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("install-stamp.json"),
            format!("{{\"version\": \"{version}\"}}"),
        )
        .unwrap();
        std::fs::write(folder.join("old-file.py"), "old").unwrap();
    }

    #[test]
    fn a_good_install_lands_and_replaces_the_old_folder() {
        let zip = release_zip("0.4.0");
        let url = serve(zip.clone());
        let addins = tempfile::tempdir().unwrap();
        plant(addins.path(), "0.3.5");
        let mut steps = Vec::new();
        let done = install(
            &options(addins.path(), url, &zip, "0.4.0"),
            &|| false,
            &mut |s| steps.push(s),
        )
        .unwrap();
        assert_eq!(done.version, "0.4.0");
        assert_eq!(steps, [Step::Downloading, Step::Checking, Step::Installing]);
        let folder = addins.path().join(ADDIN);
        assert!(folder.join("lib/a.py").is_file());
        assert!(!folder.join("old-file.py").exists());
        assert_eq!(installed_version(addins.path()).as_deref(), Some("0.4.0"));
        assert_eq!(entries(addins.path()), [ADDIN], "no staging left behind");
    }

    #[test]
    fn a_checksum_mismatch_writes_nothing() {
        let zip = release_zip("0.4.0");
        let url = serve(zip.clone());
        let addins = tempfile::tempdir().unwrap();
        let mut o = options(addins.path(), url, &zip, "0.4.0");
        o.sha256 = sha(b"something else");
        assert_eq!(run(&o, false), Err(Failure::Checksum));
        assert!(entries(addins.path()).is_empty());
        // An existing install is left as it was.
        plant(addins.path(), "0.3.5");
        assert_eq!(run(&o, false), Err(Failure::Checksum));
        assert!(addins.path().join(ADDIN).join("old-file.py").is_file());
    }

    #[test]
    fn an_older_version_never_replaces_a_newer_one() {
        let zip = release_zip("0.4.0");
        let url = serve(zip.clone());
        let addins = tempfile::tempdir().unwrap();
        plant(addins.path(), "0.5.0");
        let result = run(&options(addins.path(), url, &zip, "0.4.0"), false);
        assert_eq!(result, Err(Failure::Newer("0.5.0".into())));
        assert!(addins.path().join(ADDIN).join("old-file.py").is_file());
    }

    #[test]
    fn a_busy_exporter_is_never_replaced() {
        let zip = release_zip("0.4.0");
        let url = serve(zip.clone());
        let addins = tempfile::tempdir().unwrap();
        plant(addins.path(), "0.3.5");
        let result = run(&options(addins.path(), url, &zip, "0.4.0"), true);
        assert_eq!(result, Err(Failure::Busy));
        assert!(addins.path().join(ADDIN).join("old-file.py").is_file());
        // It turning busy during the download stops the swap too.
        let calls = std::cell::Cell::new(0);
        let url = serve(zip.clone());
        let result = install(
            &options(addins.path(), url, &zip, "0.4.0"),
            &|| {
                calls.set(calls.get() + 1);
                calls.get() > 1
            },
            &mut |_| {},
        );
        assert_eq!(result, Err(Failure::Busy));
        assert!(addins.path().join(ADDIN).join("old-file.py").is_file());
        assert_eq!(entries(addins.path()), [ADDIN]);
    }

    #[test]
    fn a_download_that_stalls_gives_up_and_writes_nothing() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nab");
                std::thread::sleep(Duration::from_secs(10));
            }
        });
        let addins = tempfile::tempdir().unwrap();
        let mut o = options(
            addins.path(),
            format!("http://127.0.0.1:{port}/x.zip"),
            b"",
            "0.4.0",
        );
        o.stall_timeout = Duration::from_millis(300);
        let started = std::time::Instant::now();
        assert!(matches!(run(&o, false), Err(Failure::Download(_))));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(entries(addins.path()).is_empty());
    }

    #[test]
    fn a_refused_connection_is_a_download_failure() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addins = tempfile::tempdir().unwrap();
        let o = options(
            addins.path(),
            format!("http://127.0.0.1:{port}/x.zip"),
            b"",
            "0.4.0",
        );
        assert!(matches!(run(&o, false), Err(Failure::Download(_))));
    }

    #[test]
    fn the_pin_keeps_its_parts_together() {
        assert!(PIN
            .zip_url
            .contains(&format!("v{0}/FusionBulkExport-{0}.zip", PIN.version)));
        assert_eq!(PIN.sha256.len(), 64);
        assert!(version_parts(PIN.version).is_some());
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.4.0", "0.4"));
        assert!(!is_newer("0.3.5", "0.4.0"));
        assert!(!is_newer("junk", "0.4.0"));
    }

    #[test]
    fn the_newer_release_is_read_from_its_json() {
        let json =
            br#"{"version":"0.4.1","zip_url":"https://example/z.zip","sha256":"x","protocol":1}"#;
        let url = serve(json.to_vec());
        assert_eq!(
            latest_release_at(&url).unwrap(),
            ("0.4.1".into(), "https://example/z.zip".into())
        );
        assert!(latest_release_at(&serve(b"{}".to_vec())).is_err());
    }

    /// The real release candidate's three files (a folder named by `DUTCHMAN_FX_RC_DIR`);
    /// skipped when it is not set. The hash comes from the release's own
    /// `.sha256` file, never from the pin, which changes when the final zip is rebuilt.
    #[test]
    fn the_real_release_candidate_installs() {
        let Some(dir) = std::env::var_os("DUTCHMAN_FX_RC_DIR").map(PathBuf::from) else {
            eprintln!("DUTCHMAN_FX_RC_DIR not set; skipped");
            return;
        };
        let zip = std::fs::read(dir.join("FusionBulkExport-0.4.0.zip")).unwrap();
        let sum = std::fs::read_to_string(dir.join("FusionBulkExport-0.4.0.zip.sha256")).unwrap();
        let url = serve(zip);
        let addins = tempfile::tempdir().unwrap();
        plant(addins.path(), "0.3.5");
        let o = Options {
            version: "0.4.0".into(),
            zip_url: url,
            sha256: sum.split_whitespace().next().unwrap().into(),
            addins: addins.path().to_path_buf(),
            connect_timeout: Duration::from_secs(5),
            stall_timeout: Duration::from_secs(5),
        };
        let done = run(&o, false).unwrap();
        assert_eq!(done.version, "0.4.0");
        let folder = addins.path().join(ADDIN);
        for file in ["install-stamp.json", "FusionBulkExport.manifest"] {
            let v: Value =
                serde_json::from_str(&std::fs::read_to_string(folder.join(file)).unwrap()).unwrap();
            assert_eq!(v["version"], "0.4.0", "{file}");
        }
        assert!(!folder.join("old-file.py").exists());
    }
}
