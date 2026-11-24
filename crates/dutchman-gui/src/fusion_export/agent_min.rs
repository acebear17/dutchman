//! The least this feature needs from the Fusion exporter's agent file: where its jobs go,
//! what it supports and which version it is. Full discovery (heartbeat, process checks) is
//! a separate piece that replaces `read_minimal`.
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The exporter an export job is handed to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub jobs_dir: PathBuf,
    pub features: Vec<String>,
    pub version: String,
    /// Fusion's process id, for the heartbeat rule.
    pub pid: Option<u32>,
    /// The heartbeat, seconds since 1970.
    pub updated: Option<i64>,
}

/// What reading the agent file found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// No file, or another exporter, or one that does not accept protocol 1.
    Absent,
    /// A file that cannot be trusted; the text says why in plain words.
    Invalid(String),
    Ready(Agent),
}

/// Whether the exporter is there, by the protocol's rule: a heartbeat under 60 s old is
/// running; an older one is checked against the process, never taken as gone by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Liveness {
    Running,
    /// The heartbeat is old but Fusion's process lives: Fusion is busy, keep waiting.
    Busy,
    Gone,
}

pub const HEARTBEAT_SECS: i64 = 60;

/// Seconds since 1970.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Whether a process with this id is running (asks the system's own tool).
pub fn pid_alive_system(pid: u32) -> bool {
    use std::process::{Command, Stdio};
    if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .stderr(Stdio::null())
            .output()
            .is_ok_and(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .any(|word| word == pid.to_string())
            })
    } else {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

/// The agent's liveness at `now`; `pid_alive` is asked only when the heartbeat is old.
pub fn liveness(agent: &Agent, now: i64, pid_alive: fn(u32) -> bool) -> Liveness {
    match agent.updated {
        Some(updated) if now - updated < HEARTBEAT_SECS => Liveness::Running,
        _ => match agent.pid {
            Some(pid) if pid_alive(pid) => Liveness::Busy,
            _ => Liveness::Gone,
        },
    }
}

/// `2026-11-23T09:30:11.004000+00:00` as seconds since 1970.
pub fn parse_iso(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once('T')?;
    let mut parts = date.split('-');
    let (y, m, d): (i64, i64, i64) = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    let (time, offset) = match rest.find(['+', '-', 'Z']) {
        Some(at) => (rest.get(..at)?, rest.get(at..)?),
        None => (rest, "Z"),
    };
    let mut clock = time.split(':');
    let hours: i64 = clock.next()?.parse().ok()?;
    let minutes: i64 = clock.next()?.parse().ok()?;
    let seconds: f64 = clock.next().unwrap_or("0").parse().ok()?;
    let shift = if offset == "Z" {
        0
    } else {
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let body = offset.get(1..)?;
        let (oh, om) = body.split_once(':').unwrap_or((body, "0"));
        sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
    };
    // Days from civil (proleptic Gregorian).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hours * 3600 + minutes * 60 + seconds as i64 - shift)
}

/// Biggest agent or status file read.
pub const SMALL_FILE: u64 = 1024 * 1024;

/// Reads a regular file of at most `cap` bytes. `Ok(None)` when it does not exist; an error
/// for a link, a folder, or a file over the cap, without reading it.
pub fn read_small(path: &Path, cap: u64) -> Result<Option<String>, String> {
    use std::io::Read;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(why) => return Err(format!("cannot read a file of the Fusion exporter: {why}")),
    };
    if !meta.is_file() {
        return Err("a file of the Fusion exporter is a link or not a plain file".into());
    }
    if meta.len() > cap {
        return Err("a file of the Fusion exporter is too large".into());
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(cap).read_to_string(&mut text))
        .map_err(|why| format!("cannot read a file of the Fusion exporter: {why}"))?;
    Ok(Some(text))
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || (cfg!(windows)
            && a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase())
}

/// Reads the agent file (`<cad dir>/agents/fusion-bulk-export.json`). Its `jobs_dir` must be
/// `<cad dir>/jobs`: any other folder is refused, so a file there cannot send jobs, or
/// deletes, elsewhere.
pub fn read(path: &Path) -> Found {
    let text = match read_small(path, SMALL_FILE) {
        Ok(Some(text)) => text,
        Ok(None) => return Found::Absent,
        Err(why) => {
            return Found::Invalid(format!(
                "The Fusion exporter's agent file cannot be used: {why}."
            ))
        }
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Found::Absent;
    };
    if value.get("exporter").and_then(Value::as_str) != Some("fusion-bulk-export") {
        return Found::Absent;
    }
    let accepts_one = value
        .get("protocols")
        .and_then(Value::as_array)
        .map(|list| list.iter().any(|p| p.as_i64() == Some(1)))
        .unwrap_or_else(|| value.get("protocol").and_then(Value::as_i64) == Some(1));
    if !accepts_one {
        return Found::Absent;
    }
    let Some(jobs_dir) = value
        .get("jobs_dir")
        .and_then(Value::as_str)
        .map(PathBuf::from)
    else {
        return Found::Absent;
    };
    let expected = path
        .parent()
        .and_then(Path::parent)
        .map(|cad| cad.join("jobs"));
    if !expected.is_some_and(|expected| same_path(&jobs_dir, &expected)) {
        return Found::Invalid(
            "The Fusion exporter's agent file names a jobs folder outside your .cad-export folder, so Dutchman will not use it.".into(),
        );
    }
    let features = value
        .get("features")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|f| f.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Found::Ready(Agent {
        jobs_dir,
        features,
        version: value
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        pid: value
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok()),
        updated: value
            .get("updated")
            .and_then(Value::as_str)
            .and_then(parse_iso),
    })
}

/// The agent, when the file is there and can be used.
pub fn read_minimal(path: &Path) -> Option<Agent> {
    match read(path) {
        Found::Ready(agent) => Some(agent),
        _ => None,
    }
}

/// The exporter's agent file: `~/.cad-export/agents/fusion-bulk-export.json`.
pub fn agent_file(cad_export: &Path) -> PathBuf {
    cad_export.join("agents").join("fusion-bulk-export.json")
}

/// `~/.cad-export` of the person running this program (`%USERPROFILE%` on Windows).
pub fn cad_export_dir() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    }?;
    Some(PathBuf::from(home).join(".cad-export"))
}
