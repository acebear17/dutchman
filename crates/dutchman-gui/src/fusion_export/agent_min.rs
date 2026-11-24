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

/// Reads the agent file; `None` when it is missing, unreadable, another exporter, or does
/// not accept job protocol 1.
pub fn read_minimal(path: &Path) -> Option<Agent> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    if value.get("exporter")?.as_str()? != "fusion-bulk-export" {
        return None;
    }
    let accepts_one = value
        .get("protocols")
        .and_then(Value::as_array)
        .map(|list| list.iter().any(|p| p.as_i64() == Some(1)))
        .unwrap_or_else(|| value.get("protocol").and_then(Value::as_i64) == Some(1));
    if !accepts_one {
        return None;
    }
    let jobs_dir = PathBuf::from(value.get("jobs_dir")?.as_str()?);
    let features = value
        .get("features")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|f| f.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Some(Agent {
        jobs_dir,
        features,
        version,
    })
}
