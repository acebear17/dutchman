//! The link record written beside a converted design: which source file, and which of its bytes,
//! the output was converted from, and how.
//!
//! `<output stem>.link.json` lets a later run tell that the source changed (`dutchman sync`,
//! the desktop app's "changed since last convert" list) and lets SolidCraft find the source
//! again. It is written by the same code that writes the design, whichever way the conversion
//! was started.

use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde_json::{json, Value};

use crate::{watch, ConvertOptions, OutputFormat};

/// The record's schema.
pub const SCHEMA: &str = "dutchman.link/1";

/// Where the record of the design `output` is written: its file name with `.link.json` in place
/// of its suffix.
pub fn path_for(output: &Path) -> PathBuf {
    output.with_extension("link.json")
}

/// What a source file was when its conversion began. Taken before converting, so a file saved
/// while the conversion runs is seen as changed afterwards and not recorded as converted.
#[derive(Clone, Debug)]
pub struct Source {
    sha256: Option<String>,
    size: Option<u64>,
    modified: Option<SystemTime>,
}

impl Source {
    /// Reads `path`. What cannot be read (or is not one file) is left out of the record.
    pub fn read(path: &Path) -> Self {
        // A file over the input limit is refused by the conversion; hashing it first would only
        // make a refusal wait on reading gigabytes.
        let hashable = std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.len() <= dutchman_core::MAX_INPUT_BYTES);
        Self::with_digest(path, hashable.then(|| watch::hash(path).ok()).flatten())
    }

    /// `path` with a digest already taken of its bytes.
    pub fn with_digest(path: &Path, sha256: impl Into<Option<String>>) -> Self {
        let meta = std::fs::metadata(path).ok().filter(|meta| meta.is_file());
        Self {
            sha256: sha256.into().filter(|_| meta.is_some()),
            size: meta.as_ref().map(std::fs::Metadata::len),
            modified: meta.and_then(|meta| meta.modified().ok()),
        }
    }
}

/// The source as it is before converting, unless no record will be written.
pub(crate) fn snapshot(input: &Path, options: &ConvertOptions) -> Option<Source> {
    (!options.no_link_record).then(|| Source::read(input))
}

/// Writes the record of a successful conversion beside its output and names it in `report`
/// (`link_record`, the file name); a record that cannot be written is reported as
/// `link_record_error`. A failed conversion, or one that wrote no file, leaves any existing
/// record as it was.
pub(crate) fn write_for_report(
    input: &Path,
    output: &Path,
    options: &ConvertOptions,
    source: Option<&Source>,
    report: &mut Value,
) {
    if options.no_link_record
        || report.get("ok").and_then(Value::as_bool) != Some(true)
        || !output.is_file()
    {
        return;
    }
    let owned;
    let source = match source {
        Some(source) => source,
        None => {
            owned = Source::read(input);
            &owned
        }
    };
    let path = path_for(output);
    let result = record(input, output, options, report, source)
        .and_then(|record| write_file(&path, &record).map(|()| path));
    if let Some(fields) = report.as_object_mut() {
        match result {
            Ok(path) => {
                let name = path.file_name().map(|name| name.to_string_lossy());
                fields.insert("link_record".to_owned(), json!(name));
            }
            Err(why) => {
                fields.insert("link_record_error".to_owned(), json!(why));
            }
        }
    }
}

/// Writes `record` to `path` atomically.
pub fn write_file(path: &Path, record: &Value) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(record).map_err(|err| err.to_string())?;
    text.push('\n');
    watch::atomic_write(path, text.as_bytes())
        .map_err(|err| format!("cannot write {}: {err}", path.display()))
}

/// The record of `output`, converted from `input` (as it was, `source`) with `options`.
pub fn record(
    input: &Path,
    output: &Path,
    options: &ConvertOptions,
    report: &Value,
    source: &Source,
) -> Result<Value, String> {
    let output_digest = watch::hash(output)
        .map_err(|err| format!("cannot read {} to record it: {err}", output.display()))?;
    let folder = std::path::absolute(output)
        .map_err(|err| err.to_string())?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let file_name = output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or("the output has no file name")?;
    let mut dutchman = json!({"version": env!("CARGO_PKG_VERSION")});
    if let (Some(commit), Some(fields)) = (option_env!("DUTCHMAN_COMMIT"), dutchman.as_object_mut())
    {
        fields.insert("commit".to_owned(), json!(commit));
    }
    Ok(json!({
        "schema": SCHEMA,
        "source": {
            "path": source_path(&folder, input)?,
            "sha256": source.sha256,
            "size": source.size,
            "modified": source.modified.map(rfc3339),
        },
        "output": {"file": file_name, "sha256": output_digest},
        "dutchman": dutchman,
        "cadmpeg": {"version": crate::anon::CADMPEG_VERSION},
        "options": options_json(options, report),
        "converted_at": rfc3339(SystemTime::now()),
    }))
}

/// The options that change what is written. Where part files are searched for is left out: it
/// names folders, and only finds the same files.
fn options_json(options: &ConvertOptions, report: &Value) -> Value {
    let choices = options.choices();
    let target = options.solidcraft_version.map_or_else(
        || {
            report
                .pointer("/solidcraft/target")
                .and_then(Value::as_str)
                .map(str::to_owned)
        },
        |version| Some(version.to_string()),
    );
    json!({
        "to": match options.to {
            OutputFormat::Solidcraft => "solidcraft",
            OutputFormat::Freecad => "freecad",
            OutputFormat::Fcstd => "fcstd",
            OutputFormat::Dxf => "dxf",
        },
        "preset": options.preset.name(),
        "history": choices.history,
        "sketches": choices.sketches,
        "joints": choices.joints,
        "part_positions": options.effective_part_positions().name(),
        "stand_up": options.stand_up_choice(),
        "solidcraft_version": target,
        "part": options.part.map(|part| json!({
            "number": part.number,
            "children": part.children,
            "stop_at_feature": part.stop_at_feature,
        })),
    })
}

/// `source` as the record names it: relative to `folder` (the output's folder) with `/`
/// separators when the two share more than the root, otherwise absolute.
fn source_path(folder: &Path, source: &Path) -> Result<String, String> {
    let source = std::path::absolute(source).map_err(|err| err.to_string())?;
    Ok(relative(folder, &source).unwrap_or_else(|| source.to_string_lossy().into_owned()))
}

fn relative(folder: &Path, target: &Path) -> Option<String> {
    let from: Vec<Component<'_>> = normal(folder);
    let to: Vec<Component<'_>> = normal(target);
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    // Only a root (or a drive) in common: a relative path would name the whole machine.
    let root = from
        .iter()
        .take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
        .count();
    if shared <= root {
        return None;
    }
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len().saturating_sub(shared)];
    parts.extend(
        to.iter()
            .skip(shared)
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    Some(parts.join("/"))
}

/// The components of an absolute path, `.` and `..` already applied.
fn normal(path: &Path) -> Vec<Component<'_>> {
    let mut parts: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(parts.last(), Some(Component::Normal(_))) {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    parts
}

/// `time` as an RFC 3339 UTC timestamp, to the second.
fn rfc3339(time: SystemTime) -> String {
    let seconds = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    };
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_file_over_the_input_limit_is_not_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.step");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(dutchman_core::MAX_INPUT_BYTES + 1)
            .unwrap();
        let source = Source::read(&path);
        assert_eq!(source.sha256, None);
        assert_eq!(source.size, Some(dutchman_core::MAX_INPUT_BYTES + 1));
    }

    #[test]
    fn timestamps_are_rfc_3339_utc() {
        let at = |seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        assert_eq!(rfc3339(at(0)), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(at(951_782_400)), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(at(1_700_000_000)), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn paths_are_relative_when_they_share_more_than_the_root() {
        let rel = |a: &str, b: &str| relative(Path::new(a), Path::new(b));
        assert_eq!(rel("/work/out", "/work/in/a.f3d").unwrap(), "../in/a.f3d");
        assert_eq!(rel("/work/out", "/work/out/a.f3d").unwrap(), "a.f3d");
        assert_eq!(rel("/work/out", "/other/a.f3d"), None);
        assert_eq!(rel("/work/out", "/work/out/x/../a.f3d").unwrap(), "a.f3d");
    }

    #[test]
    fn the_record_sits_beside_the_output() {
        assert_eq!(
            path_for(Path::new("/o/part.sc")),
            Path::new("/o/part.link.json")
        );
    }
}
