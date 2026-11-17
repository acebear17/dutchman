//! The same CADIR and SolidCraft writer pipeline as the CLI, with host-owned file access.
#[cfg(target_arch = "wasm32")]
mod adapter;
// The guest's evaluator over the host's `evaluate` import, and its tests.
#[cfg(any(test, target_arch = "wasm32"))]
mod evaluator;
mod report;
mod stand_up;

use dutchman_core::{Error, Format, Report, Writer};
use dutchman_solidcraft::{SolidCraftEvaluator, SolidCraftVersion, SolidCraftWriter, Verification};
use serde::Deserialize;
use std::path::{Path, PathBuf};

mod limits;
pub use limits::{CHUNK_BYTES, MAX_INPUT_BYTES, MAX_OUTPUT_BYTES, MAX_REPORT_BYTES};
#[cfg(test)]
mod tests;

/// What the report says when the Keep editable history option is off: the plugin's own
/// option, not the command line's `--import-only`.
pub const EDITABLE_HISTORY_OFF: &str = "Keep editable history is turned off";

/// The formats the plugin converts, as its manifest lists them.
pub const FORMATS: [Format; 6] = [
    Format::FusionArchive,
    Format::FusionMultiDocument,
    Format::InventorPart,
    Format::InventorAssembly,
    Format::SolidWorksPart,
    Format::SolidWorksAssembly,
];

/// The plugin's formats in a message.
const FORMAT_NAMES: &str =
    "Fusion (.f3d, .f3z), Inventor (.ipt, .iam) or SolidWorks (.sldprt, .sldasm)";

/// Why FreeCAD is not a plugin format: SolidCraft's own reader keeps more of its history.
pub const FREECAD: &str = "Open FreeCAD files with File > Open: SolidCraft's own reader keeps \
     their editable history, which the Dutchman plugin does not match yet.";

/// What to do first when a file is over the plugin's input limit.
pub fn input_limit_message(limit: u64) -> String {
    format!(
        "Choose a file under {}, or convert this one with the dutchman command line, which has \
         no such limit: {}.",
        size(limit),
        dutchman_core::RELEASES
    )
}

/// A byte count in the unit a person would say.
fn size(bytes: u64) -> String {
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    if bytes >= GIB && bytes % GIB == 0 {
        format!("{} GiB", bytes / GIB)
    } else if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{} MiB", bytes.div_ceil(MIB))
    } else {
        format!("{bytes} bytes")
    }
}

/// What the plugin tells the user when a conversion stops: what to do first, then the detail.
/// It names only the plugin's own formats and limits.
pub fn message(error: &Error) -> String {
    match error {
        Error::NotSupportedYet {
            format: Format::FreeCadDocument,
        } => FREECAD.to_owned(),
        Error::UnknownFormat { .. } | Error::NotSupportedYet { .. } => format!(
            "Choose a {FORMAT_NAMES} file: the Dutchman plugin converts only those. SolidCraft \
             opens STEP, IGES and mesh files itself with File > Import."
        ),
        Error::TooLarge { bytes, limit, .. } => format!(
            "{} This file is {}.",
            input_limit_message(*limit),
            size(*bytes)
        ),
        Error::MemoryBudget(detail) => format!(
            "Convert this design with the dutchman command line, which has no plugin memory \
             limit: {}. Detail: {detail}",
            dutchman_core::RELEASES
        ),
        Error::Assembly(detail) => format!(
            "Keep the assembly's part files in its folder, under their saved names, and open it \
             again. Detail: {detail}"
        ),
        Error::NoGeometry(detail) => format!(
            "Open a design with a solid or surface in it: this one has none SolidCraft can take. \
             Detail: {detail}"
        ),
        // Already says what to do: convert fewer parts at once.
        Error::Encode(detail) if detail.contains("exceeds") => detail.clone(),
        _ => {
            let text = error.to_string();
            // The reader's own advice names the command line's anonymous report; say it here
            // first, in the plugin's words, and keep only the cause as the detail.
            let cause = text
                .split_once(". The file may be incomplete")
                .map_or(text.as_str(), |(cause, _)| cause);
            format!(
                "Download or copy the file again and open it once more. If the original opens \
                 in the program that made it, report the problem on Dutchman's issue form ({}); \
                 `dutchman convert FILE --report-anon report.json` makes an anonymous report to \
                 attach. Detail: {cause}",
                dutchman_core::FILE_DID_NOT_CONVERT
            )
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpAxis {
    #[default]
    AsIs,
    StandUp,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub editable_history: bool,
    pub up_axis: UpAxis,
    pub include_hidden_bodies: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            editable_history: true,
            up_axis: UpAxis::AsIs,
            include_hidden_bodies: false,
        }
    }
}

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 255
        || matches!(name, "." | "..")
        || name.contains(['/', '\\', ':', '\0'])
    {
        return Err("Choose a named CAD file without folder components".into());
    }
    Ok(())
}

/// Resolve stored part paths within the input folder, preferring the longest matching
/// path suffix and nearest referring folder. The host enforces the folder grant.
pub fn resolve(stored: &str, folder: &Path, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    const MAX_COMPONENTS: usize = 64;
    const MAX_PATH_BYTES: usize = 4096;
    let safe = |name: &str| {
        !name.is_empty()
            && name.len() <= MAX_PATH_BYTES
            && !name.contains('\\')
            && name.split('/').count() <= MAX_COMPONENTS
            && name
                .split('/')
                .all(|p| !p.is_empty() && !matches!(p, "." | "..") && !p.contains([':', '\0']))
    };
    let normalized = stored.replace('\\', "/");
    let mut components: Vec<_> = normalized.rsplit('/').take(MAX_COMPONENTS).collect();
    components.reverse();
    let folder = PathBuf::from(folder.to_string_lossy().replace('\\', "/"));
    let folders: Vec<_> = folder
        .ancestors()
        .take(MAX_COMPONENTS)
        .filter(|p| p.as_os_str().is_empty() || safe(&p.to_string_lossy()))
        .collect();
    for start in 0..components.len() {
        let tail = components.get(start..).unwrap_or_default().join("/");
        if !safe(&tail) {
            continue;
        }
        for folder in &folders {
            let joined = folder.join(&tail);
            let candidate = PathBuf::from(joined.to_string_lossy().replace('\\', "/"));
            if safe(&candidate.to_string_lossy()) && exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// The plugin format a file name names, or why the plugin does not convert it. The adapter
/// asks before reading any bytes.
pub fn format(name: &str) -> Result<Format, Error> {
    let path = Path::new(name);
    let format =
        Format::from_path(path).ok_or_else(|| Error::UnknownFormat { path: path.into() })?;
    if FORMATS.contains(&format) {
        Ok(format)
    } else {
        Err(Error::NotSupportedYet { format })
    }
}

pub fn convert(
    name: &str,
    options: Options,
    reader: &dutchman_core::source::SourceReader<'_>,
    resolve: &dyn Fn(&str, &Path) -> Option<PathBuf>,
    evaluator: Option<std::sync::Arc<dyn SolidCraftEvaluator>>,
) -> Result<(Vec<u8>, String), Error> {
    validate_name(name).map_err(Error::Decode)?;
    let path = Path::new(name);
    let format = format(name)?;
    let mut report = Report::default();
    let model = dutchman_core::source::read(
        format,
        path,
        options.include_hidden_bodies,
        dutchman_core::PartPositions::Smart,
        reader,
        resolve,
        &mut report,
    )?;
    let mut design = Vec::new();
    // With the host's evaluator, the history is checked in SolidCraft itself, in a private
    // session apart from every open design, before stand-up below; without one it stays
    // unverified and hidden, as before. The trait doesn't discover the host's release.
    let writer = SolidCraftWriter::new()
        .threads(1)
        .target_version(Some(SolidCraftVersion::default()))
        .hide_unverified_history(true)
        .import_only(!options.editable_history)
        .import_only_reason(EDITABLE_HISTORY_OFF);
    let writer = match evaluator {
        Some(evaluator) => writer.evaluator(evaluator).verification(Verification::Auto),
        None => writer.verification(Verification::Off),
    };
    writer.write(&model, &mut design, &mut report)?;
    if matches!(options.up_axis, UpAxis::StandUp) {
        design = stand_up::apply(&design)?;
    }
    if design.len() > MAX_OUTPUT_BYTES {
        return Err(Error::Encode(
            "Converted design exceeds 256 MiB; convert fewer parts at once".into(),
        ));
    }
    let mut host_report = report::host(&report, EDITABLE_HISTORY_OFF);
    let mut text = serde_json::to_string(&host_report).map_err(|e| Error::Encode(e.to_string()))?;
    if text.len() > MAX_REPORT_BYTES {
        // The items already carry each loss; keep the rest of the details.
        if let Some(details) = host_report
            .get_mut("details")
            .and_then(serde_json::Value::as_object_mut)
        {
            details.remove("losses");
        }
        text = serde_json::to_string(&host_report).map_err(|e| Error::Encode(e.to_string()))?;
    }
    if text.len() > MAX_REPORT_BYTES {
        return Err(Error::Encode(
            "Conversion report exceeds 1 MiB; convert fewer parts at once".into(),
        ));
    }
    Ok((design, text))
}
