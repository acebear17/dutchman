//! A bulk export's `manifest.json`: which designs an exported Fusion assembly inserts, and
//! where the export wrote each one.
//!
//! A Fusion assembly saved as one `.f3d` holds only its own parts: the designs it inserts live in
//! other documents. A bulk export writes each of those as a `.f3d` of its own beside the
//! assembly and lists them in the manifest, per assembly, as `references` (the design's `id` and,
//! in newer manifests, its `version` and the exported file's `path`). This module reads that JSON
//! listing and nothing else: it never opens a design file. Every path it returns lies inside the
//! export folder.

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// The manifest's file name.
pub(crate) const MANIFEST: &str = "manifest.json";
/// The largest manifest read.
const MAX_MANIFEST_BYTES: u64 = 256 << 20;
/// The most designs a manifest may list.
const MAX_ENTRIES: usize = crate::locate::MAX_SEARCHED;
/// How many folders above a design are searched for its manifest.
const LEVELS: usize = 8;

/// One design the manifest lists.
#[derive(Debug, Default)]
struct Entry {
    id: String,
    name: Option<String>,
    version: Option<u64>,
    /// Exported files, relative to the manifest's folder, as written.
    files: Vec<String>,
    references: Vec<RawReference>,
}

/// A reference as the manifest states it.
#[derive(Debug, Default)]
struct RawReference {
    id: Option<String>,
    version: Option<u64>,
    path: Option<String>,
}

/// A manifest, parsed once.
#[derive(Debug)]
pub(crate) struct Manifest {
    /// The folder the manifest is in, as the real path every exported file must stay under.
    root: PathBuf,
    entries: Vec<Entry>,
}

/// One design an assembly inserts, resolved to the file the export wrote for it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Reference {
    /// The design's id in the manifest, when the reference has it.
    pub id: Option<String>,
    /// The design's name, when the manifest has it.
    pub name: Option<String>,
    /// The design's version, when the manifest has it.
    pub version: Option<u64>,
    /// The exported file, inside the export folder.
    pub file: PathBuf,
}

/// What the manifest says an assembly inserts.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Inserted {
    /// The designs it inserts whose files are in the export.
    pub references: Vec<Reference>,
    /// The references whose file is not in the export, by id, and why (a path inside the
    /// message is the manifest's own).
    pub missing: Vec<(String, String)>,
}

type Cache = Mutex<HashMap<PathBuf, (SystemTime, u64, Arc<Manifest>)>>;

fn cache() -> &'static Cache {
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The manifest of the export folder `design` is in, read once per change of the file.
fn manifest_for(design: &Path, top: Option<&Path>) -> Option<Result<Arc<Manifest>, String>> {
    let design = design.canonicalize().ok()?;
    let top = top.and_then(|top| top.canonicalize().ok());
    let mut folder = design.parent()?;
    for _ in 0..LEVELS {
        let path = folder.join(MANIFEST);
        if path.is_file() {
            return Some(load(&path));
        }
        if top.as_deref() == Some(folder) {
            break;
        }
        folder = folder.parent()?;
    }
    None
}

fn load(path: &Path) -> Result<Arc<Manifest>, String> {
    let meta = std::fs::metadata(path).map_err(|err| format!("{MANIFEST}: {err}"))?;
    if meta.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "{MANIFEST} is {} MB, over Dutchman's limit of {} MB",
            meta.len() >> 20,
            MAX_MANIFEST_BYTES >> 20
        ));
    }
    let stamp = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let key = path.to_path_buf();
    if let Ok(cache) = cache().lock() {
        if let Some((when, len, manifest)) = cache.get(&key) {
            if *when == stamp && *len == meta.len() {
                return Ok(manifest.clone());
            }
        }
    }
    let bytes = std::fs::read(path).map_err(|err| format!("{MANIFEST}: {err}"))?;
    let manifest = Arc::new(parse(&bytes, path.parent().unwrap_or(Path::new(".")))?);
    if let Ok(mut cache) = cache().lock() {
        if cache.len() > 16 {
            cache.clear();
        }
        cache.insert(key, (stamp, meta.len(), manifest.clone()));
    }
    Ok(manifest)
}

fn text(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .filter(|text| !text.is_empty())
}

fn parse(bytes: &[u8], folder: &Path) -> Result<Manifest, String> {
    let root = folder
        .canonicalize()
        .map_err(|err| format!("{MANIFEST}: {err}"))?;
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|err| format!("{MANIFEST} is not valid JSON: {err}"))?;
    let rows = value
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{MANIFEST} lists no entries"))?;
    if rows.len() > MAX_ENTRIES {
        return Err(format!(
            "{MANIFEST} lists {} designs, over Dutchman's limit of {MAX_ENTRIES}",
            rows.len()
        ));
    }
    let entries = rows
        .iter()
        .map(|row| Entry {
            id: text(row, "id").unwrap_or_default(),
            name: text(row, "name"),
            version: row.get("version").and_then(serde_json::Value::as_u64),
            files: row
                .get("files")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|file| text(file, "path"))
                .collect(),
            references: row
                .get("references")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .map(|reference| RawReference {
                    id: text(reference, "id"),
                    version: reference.get("version").and_then(serde_json::Value::as_u64),
                    path: text(reference, "path"),
                })
                .collect(),
        })
        .collect();
    Ok(Manifest { root, entries })
}

/// The real path of the file a manifest path names, when it names one inside `root`.
fn inside(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    for part in relative.split(['/', '\\']).filter(|part| !part.is_empty()) {
        if part == ".." || part == "." || part.contains(':') || part.contains('\0') {
            return Err(format!("{relative}: a path that leaves the export folder"));
        }
        path.push(part);
    }
    if Path::new(relative)
        .components()
        .any(|part| matches!(part, Component::RootDir | Component::Prefix(_)))
        || relative.starts_with(['/', '\\'])
    {
        return Err(format!("{relative}: a path that leaves the export folder"));
    }
    let real = path
        .canonicalize()
        .map_err(|_| format!("{relative}: the export does not hold this file"))?;
    if !real.starts_with(root) {
        return Err(format!("{relative}: a path that leaves the export folder"));
    }
    Ok(real)
}

fn is_design(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("f3d"))
}

impl Manifest {
    /// The entries whose exported file is `design` (a `.f3d`), or, for a rebuilt `.f3z`, whose
    /// `.f3d` has the same name and folder.
    fn entries_of(&self, design: &Path) -> Vec<&Entry> {
        let archive = design
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("f3z"));
        let stem = design.with_extension("");
        self.entries
            .iter()
            .filter(|entry| {
                entry.files.iter().any(|file| {
                    let Ok(real) = inside(&self.root, file) else {
                        return false;
                    };
                    if archive {
                        is_design(file) && real.with_extension("") == stem
                    } else {
                        real == design
                    }
                })
            })
            .collect()
    }

    /// The exported `.f3d` of the design with `id` (and `version`, else the newest).
    fn file_of(&self, id: &str, version: Option<u64>) -> Option<(&Entry, &str)> {
        let mut found: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.id == id && entry.files.iter().any(|file| is_design(file)))
            .collect();
        if let Some(version) = version {
            found.retain(|entry| entry.version == Some(version));
        }
        let entry = found.into_iter().max_by_key(|entry| entry.version)?;
        let file = entry.files.iter().find(|file| is_design(file))?;
        Some((entry, file))
    }
}

/// What the manifest of the export folder `design` is in says it inserts: `None` when no
/// manifest covers it, an error when the manifest is unusable.
pub(crate) fn inserted(design: &Path, top: Option<&Path>) -> Option<Result<Inserted, String>> {
    let manifest = match manifest_for(design, top)? {
        Ok(manifest) => manifest,
        Err(why) => return Some(Err(why)),
    };
    let design = design.canonicalize().ok()?;
    let entries = manifest.entries_of(&design);
    if entries.is_empty() {
        return None;
    }
    let mut out = Inserted::default();
    let mut seen: BTreeMap<PathBuf, ()> = BTreeMap::new();
    for entry in entries {
        for reference in &entry.references {
            if out.references.len() + out.missing.len() >= crate::assembly::MAX_PARTS {
                return Some(Err(format!(
                    "the assembly inserts over {} designs, Dutchman's limit",
                    crate::assembly::MAX_PARTS
                )));
            }
            let found = match (&reference.path, &reference.id) {
                (Some(path), id) => {
                    let (name, version) = id
                        .as_deref()
                        .and_then(|id| manifest.file_of(id, reference.version))
                        .map_or((None, None), |(entry, _)| {
                            (entry.name.clone(), entry.version)
                        });
                    Ok((path.as_str(), name, reference.version.or(version)))
                }
                (None, Some(id)) => manifest
                    .file_of(id, reference.version)
                    .map(|(entry, file)| (file, entry.name.clone(), entry.version))
                    .ok_or_else(|| "the export does not hold this design".to_owned()),
                (None, None) => Err("the reference names no design".to_owned()),
            };
            let label = reference.id.clone().unwrap_or_default();
            match found.and_then(|(file, name, version)| {
                inside(&manifest.root, file).map(|real| (real, name, version))
            }) {
                Ok((file, name, version)) => {
                    if seen.insert(file.clone(), ()).is_none() {
                        out.references.push(Reference {
                            id: reference.id.clone(),
                            name,
                            version,
                            file,
                        });
                    }
                }
                Err(why) => out.missing.push((label, why)),
            }
        }
    }
    Some(Ok(out))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    fn export(manifest: serde_json::Value) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for file in ["Asm v3.f3d", "Screw v2.f3d", "Plate v1.f3d"] {
            std::fs::write(dir.path().join(file), b"x").unwrap();
        }
        std::fs::write(
            dir.path().join(MANIFEST),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        dir
    }

    fn entry(
        id: &str,
        name: &str,
        version: u64,
        references: serde_json::Value,
    ) -> serde_json::Value {
        json!({"id": id, "name": name, "version": version, "references": references,
               "files": [{"path": format!("{name} v{version}.f3d")}, {"path": format!("{name} v{version}.step")}]})
    }

    #[test]
    fn references_resolve_by_id_to_the_exported_design() {
        let dir = export(json!({"entries": [
            entry("urn:a:1", "Asm", 3, json!([{"id": "urn:a:2", "name": "Screw"}, {"id": "urn:a:3"}])),
            entry("urn:a:2", "Screw", 2, json!([])),
            entry("urn:a:3", "Plate", 1, json!([])),
        ]}));
        let inserted = inserted(&dir.path().join("Asm v3.f3d"), None)
            .unwrap()
            .unwrap();
        let names: Vec<_> = inserted
            .references
            .iter()
            .map(|r| r.name.clone().unwrap())
            .collect();
        assert_eq!(names, ["Screw", "Plate"]);
        assert_eq!(inserted.references[0].version, Some(2));
        assert!(inserted.missing.is_empty());
        // A design that lists nothing, or no manifest, is not an assembly of inserts.
        let plate = self::inserted(&dir.path().join("Plate v1.f3d"), None)
            .unwrap()
            .unwrap();
        assert!(plate.references.is_empty());
    }

    #[test]
    fn a_stated_path_and_version_win_and_a_missing_design_is_named() {
        let dir = export(json!({"entries": [
            entry("urn:a:1", "Asm", 3, json!([
                {"id": "urn:a:2", "version": 2, "path": "Screw v2.f3d"},
                {"id": "urn:a:9", "version": 1, "path": "Gone v1.f3d"},
                {"id": "urn:a:8"}])),
            entry("urn:a:2", "Screw", 2, json!([])),
        ]}));
        let inserted = inserted(&dir.path().join("Asm v3.f3d"), None)
            .unwrap()
            .unwrap();
        assert_eq!(inserted.references.len(), 1);
        assert_eq!(inserted.references[0].name.as_deref(), Some("Screw"));
        assert_eq!(inserted.missing.len(), 2);
    }

    #[test]
    fn a_path_that_leaves_the_export_folder_is_refused() {
        let outer = tempfile::tempdir().unwrap();
        std::fs::write(outer.path().join("secret.f3d"), b"x").unwrap();
        let dir = outer.path().join("export");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("Asm v3.f3d"), b"x").unwrap();
        std::fs::write(
            dir.join(MANIFEST),
            serde_json::to_vec(&json!({"entries": [entry("urn:a:1", "Asm", 3, json!([
                {"id": "urn:a:2", "path": "../secret.f3d"},
                {"id": "urn:a:3", "path": "/etc/passwd.f3d"},
                {"id": "urn:a:4", "path": "C:\\x\\y.f3d"}]))]}))
            .unwrap(),
        )
        .unwrap();
        let inserted = inserted(&dir.join("Asm v3.f3d"), None).unwrap().unwrap();
        assert!(inserted.references.is_empty());
        assert_eq!(inserted.missing.len(), 3);
        assert!(inserted
            .missing
            .iter()
            .all(|(_, why)| why.contains("leaves the export folder")));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_export_folder_is_refused() {
        let outer = tempfile::tempdir().unwrap();
        std::fs::write(outer.path().join("secret.f3d"), b"x").unwrap();
        let dir = outer.path().join("export");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("Asm v3.f3d"), b"x").unwrap();
        std::os::unix::fs::symlink(outer.path().join("secret.f3d"), dir.join("Link v1.f3d"))
            .unwrap();
        std::fs::write(
            dir.join(MANIFEST),
            serde_json::to_vec(&json!({"entries": [entry("urn:a:1", "Asm", 3, json!([
                {"id": "urn:a:2", "path": "Link v1.f3d"}]))]}))
            .unwrap(),
        )
        .unwrap();
        let inserted = inserted(&dir.join("Asm v3.f3d"), None).unwrap().unwrap();
        assert!(inserted.references.is_empty());
        assert_eq!(inserted.missing.len(), 1);
    }

    #[test]
    fn a_rebuilt_archive_takes_its_design_s_references() {
        let dir = export(json!({"entries": [
            entry("urn:a:1", "Asm", 3, json!([{"id": "urn:a:2"}])),
            entry("urn:a:2", "Screw", 2, json!([])),
        ]}));
        std::fs::write(dir.path().join("Asm v3.f3z"), b"x").unwrap();
        let inserted = inserted(&dir.path().join("Asm v3.f3z"), None)
            .unwrap()
            .unwrap();
        assert_eq!(inserted.references.len(), 1);
    }

    #[test]
    fn no_manifest_or_an_unlisted_file_is_not_an_export() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.f3d"), b"x").unwrap();
        assert!(inserted(&dir.path().join("a.f3d"), None).is_none());
        let dir = export(json!({"entries": []}));
        assert!(inserted(&dir.path().join("Asm v3.f3d"), None).is_none());
    }

    #[test]
    fn an_unreadable_manifest_is_an_error_and_a_huge_listing_is_refused() {
        let dir = export(json!({"entries": []}));
        std::fs::write(dir.path().join(MANIFEST), b"{not json").unwrap();
        assert!(inserted(&dir.path().join("Asm v3.f3d"), None)
            .unwrap()
            .is_err());
        let many: Vec<_> = (0..MAX_ENTRIES + 1)
            .map(|n| json!({"id": n.to_string()}))
            .collect();
        let dir = export(json!({"entries": many}));
        assert!(inserted(&dir.path().join("Asm v3.f3d"), None)
            .unwrap()
            .is_err());
    }
}
