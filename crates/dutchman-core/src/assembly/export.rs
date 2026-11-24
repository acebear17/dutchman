//! Designs a Fusion assembly inserts, read from a bulk export's folder.
//!
//! The assembly's `.f3d` holds only its own parts; cadmpeg states each insertion of another
//! design as an occurrence whose prototype is unresolved, with its placement. The export wrote
//! every inserted design as a `.f3d` of its own and listed them in `manifest.json`
//! ([`crate::export_folder`]). Each insertion is matched to one of those designs by what cadmpeg
//! states about it, the design is read through the same decoder as any other file, and the
//! assembly places it at the insertion's transform, as it places its own components.

use super::*;
use crate::export_folder::{self, Reference};

/// An insertion of another design that the file doesn't contain, as cadmpeg states it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Insert {
    /// The occurrence's id in the CADIR.
    pub id: String,
    /// The occurrence's name: the insertion's role or label.
    pub name: String,
    /// The identity of the design the reader states the insertion places, when it does.
    pub document_id: Option<String>,
    /// The version of that design, when the reader states it.
    pub version: Option<String>,
}

/// The insertions in `model` whose design the file doesn't hold: the reader states their
/// prototype as unresolved, or (newer readers) as an empty placeholder definition named for
/// the unresolved occurrence.
pub(crate) fn unresolved_inserts(model: &Model) -> Vec<Insert> {
    let (Ok(definitions), Ok(occurrences)) = (
        serde_json::to_value(&model.ir.model.product_definitions),
        serde_json::to_value(&model.ir.model.occurrences),
    ) else {
        return Vec::new();
    };
    // The placeholder is either a definition with no bodies and no children, or no definition
    // at all (readers that name the unresolved prototype without stating it).
    let empty = |id: &str| {
        definitions
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row.get("id").and_then(Value::as_str) == Some(id))
            .is_none_or(|row| {
                ["bodies", "occurrences"].iter().all(|key| {
                    row.get(*key)
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty)
                })
            })
    };
    let stated = |row: &Value, key: &str| {
        let definition = row.pointer("/prototype/definition")?.as_str()?;
        definitions
            .as_array()?
            .iter()
            .find(|candidate| candidate.get("id").and_then(Value::as_str) == Some(definition))?
            .pointer(&format!("/storage/{key}"))?
            .as_str()
            .map(str::to_owned)
    };
    occurrences
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| {
            scope(row) == Some("unresolved")
                || row
                    .pointer("/prototype/definition")
                    .and_then(Value::as_str)
                    .is_some_and(|definition| {
                        definition.ends_with("/unresolved") && empty(definition)
                    })
        })
        .map(|row| Insert {
            id: row
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            name: row
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            document_id: stated(row, "document_id"),
            version: stated(row, "version"),
        })
        .collect()
}

/// The design an insertion places, among the designs the manifest lists for the assembly.
///
/// cadmpeg states an insertion's name as `<design name> v<version>:<instance>` (Fusion's own
/// occurrence name) when the file keeps it, and this is the design's name and version in the
/// manifest. Only an exact, unambiguous match counts: a name that fits no listed design, or
/// more than one, matches none.
fn choose<'a>(insert: &Insert, references: &'a [Reference]) -> Option<&'a Reference> {
    // The reader states which design the insertion places: that identity (and version) is the
    // key. A stated identity that fits no listed design matches none, never a guess by name.
    if let Some(document_id) = insert.document_id.as_deref() {
        let version = insert
            .version
            .as_deref()
            .and_then(|version| version.trim().trim_start_matches('v').parse::<u64>().ok());
        let mut found = references.iter().filter(|reference| {
            reference.id.as_deref() == Some(document_id)
                && version.is_none_or(|stated| reference.version.is_none_or(|own| own == stated))
        });
        return match (found.next(), found.next()) {
            (Some(only), None) => Some(only),
            _ => None,
        };
    }
    let name = insert.name.trim();
    if name.is_empty() {
        return None;
    }
    // Drop the instance number after the last colon: `Screw v2:3` is the third `Screw v2`.
    let design = match name.rsplit_once(':') {
        Some((design, instance)) if instance.chars().all(|c| c.is_ascii_digit()) => design,
        _ => name,
    };
    let mut found = references.iter().filter(|reference| {
        let Some(own) = &reference.name else {
            return false;
        };
        design.eq_ignore_ascii_case(own)
            || reference
                .version
                .is_some_and(|version| design.eq_ignore_ascii_case(&format!("{own} v{version}")))
    });
    match (found.next(), found.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

type Loaded = Result<(usize, std::rc::Rc<Model>), (Stage, String)>;

/// Reads the designs an export folder holds for the insertions in `model` (read from
/// `input`), found through its manifest: each insertion that matches a listed design, by the
/// insertion's id. An insertion with no manifest, no match, or a design that cannot be read is
/// left out of the result when the file says nothing more (the caller reports it as missing),
/// and left out with its own reason when the export names a design that cannot be read.
pub(crate) fn resolve(
    model: &Model,
    input: &Path,
    top: Option<&Path>,
    report: &mut Report,
    decode: &Decode,
    finder: &Finder,
    open: &mut Vec<PathBuf>,
) -> ExternalParts {
    let mut external = ExternalParts::new();
    let inserts = unresolved_inserts(model);
    if inserts.is_empty() {
        return external;
    }
    let listing = match export_folder::inserted(input, top) {
        None => return external,
        Some(Ok(listing)) => listing,
        Some(Err(why)) => {
            report.push_at(Stage::Input, Severity::Info, "export manifest", why);
            return external;
        }
    };
    if !listing.missing.is_empty() {
        let shown: Vec<&str> = listing
            .missing
            .iter()
            .take(5)
            .map(|(_, why)| why.as_str())
            .collect();
        report.push_at(
            Stage::Input,
            Severity::Info,
            "export manifest",
            format!(
                "{} designs the manifest lists for this assembly are not in the export ({}{})",
                listing.missing.len(),
                shown.join("; "),
                if listing.missing.len() > shown.len() {
                    "; ..."
                } else {
                    ""
                }
            ),
        );
    }
    let key = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    if open.contains(&key) || open.len() >= MAX_DEPTH {
        return external;
    }
    open.push(key);
    let mut files: BTreeMap<PathBuf, Loaded> = BTreeMap::new();
    for insert in &inserts {
        let Some(reference) = choose(insert, &listing.references) else {
            continue;
        };
        let file = reference
            .file
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let count = files.len();
        let loaded = files
            .entry(reference.file.clone())
            .or_insert_with(|| {
                if count >= MAX_PARTS {
                    return Err((
                        Stage::Input,
                        format!("over Dutchman's limit of {MAX_PARTS} part files"),
                    ));
                }
                let mut part_report = Report::default();
                let read = read_design(reference, top, &mut part_report, decode, finder, open)
                    .map(|part| (count, std::rc::Rc::new(part)))
                    .map_err(|err| (err.stage(), err.to_string()));
                report.append_prefixed(part_report, &file);
                read
            })
            .clone();
        let name = Some(insert.name.clone()).filter(|name| !name.is_empty());
        external.insert(
            insert.id.clone(),
            loaded.map_err(|(stage, why)| {
                let subject = format!("occurrence \"{}\"", name.as_deref().unwrap_or("unnamed"));
                report.push_at(stage, Severity::Dropped, subject, format!("{file}: {why}"));
                LeftOut {
                    name,
                    file: Some(file.clone()),
                    reason: format!("{file} could not be read: {why}"),
                    stage,
                }
            }),
        );
    }
    open.pop();
    let unmatched = inserts
        .iter()
        .filter(|insert| !external.contains_key(&insert.id))
        .count();
    if unmatched > 0 && !listing.references.is_empty() {
        report.push_at(
            Stage::Input,
            Severity::Info,
            "export manifest",
            format!(
                "{unmatched} of {} inserted components could not be matched to the {} designs the \
                 manifest lists for this assembly: the file doesn't state which design each \
                 insertion places, and Dutchman doesn't guess",
                inserts.len(),
                listing.references.len()
            ),
        );
    }
    external
}

/// One inserted design, read, with the designs it inserts in turn and its own components.
fn read_design(
    reference: &Reference,
    top: Option<&Path>,
    report: &mut Report,
    decode: &Decode,
    finder: &Finder,
    open: &mut Vec<PathBuf>,
) -> Result<Model, Error> {
    crate::names::saw_path(&reference.file, report);
    let mut part = decode(Format::FusionArchive, &reference.file, report)?;
    if let Some(name) = &reference.name {
        part.name.clone_from(name);
    }
    let nested = resolve(&part, &reference.file, top, report, decode, finder, open);
    match local_components_with_threads(
        &part,
        &nested,
        finder.threads,
        finder.part_positions,
        report,
    ) {
        Ok(Some((assembly, _))) => part.assembly = Some(assembly),
        Ok(None) => {}
        Err(reason) => report.push_at(
            Stage::Map,
            Severity::Degraded,
            "components",
            format!("the design places components, but {reason}; it converts as stored"),
        ),
    }
    Ok(part)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reference(name: &str, version: u64) -> Reference {
        Reference {
            id: Some(format!("urn:x:{name}")),
            name: Some(name.to_owned()),
            version: Some(version),
            file: PathBuf::from(format!("{name} v{version}.f3d")),
        }
    }

    fn insert(name: &str) -> Insert {
        Insert {
            id: "i".into(),
            name: name.into(),
            document_id: None,
            version: None,
        }
    }

    #[test]
    fn a_stated_design_identity_is_the_key_and_a_misfit_is_never_guessed_by_name() {
        let listed = [reference("Screw", 2), reference("Plate", 1)];
        let stated = |id: &str, version: Option<&str>, name: &str| Insert {
            id: "i".into(),
            name: name.into(),
            document_id: Some(id.into()),
            version: version.map(str::to_owned),
        };
        let file = |insert: &Insert| choose(insert, &listed).map(|r| r.file.clone());
        assert_eq!(
            file(&stated("urn:x:Plate", Some("1"), "role label")),
            Some(PathBuf::from("Plate v1.f3d"))
        );
        assert_eq!(
            file(&stated("urn:x:Screw", None, "")),
            Some(PathBuf::from("Screw v2.f3d"))
        );
        // A stated version the listed design doesn't have, or an identity no design has,
        // matches none even though the name fits.
        assert_eq!(file(&stated("urn:x:Screw", Some("3"), "Screw v2:1")), None);
        assert_eq!(file(&stated("urn:x:Nut", None, "Screw v2:1")), None);
    }

    #[test]
    fn an_insertion_matches_the_design_it_names_without_its_instance_number() {
        let listed = [reference("Screw", 2), reference("Plate", 1)];
        for name in ["Screw v2:1", "Screw v2:12", "Screw v2", "screw", "Screw:4"] {
            assert_eq!(
                choose(&insert(name), &listed).map(|r| r.file.clone()),
                Some(PathBuf::from("Screw v2.f3d")),
                "{name}"
            );
        }
    }

    #[test]
    fn an_insertion_that_fits_no_design_or_two_matches_none() {
        let listed = [reference("Screw", 2), reference("Screw", 3)];
        // The name alone fits both versions; the version separates them.
        assert!(choose(&insert("Screw:1"), &listed).is_none());
        assert_eq!(
            choose(&insert("Screw v3:1"), &listed).map(|r| r.version),
            Some(Some(3))
        );
        assert!(choose(&insert("Nut v1:1"), &listed).is_none());
        // A role that is only an identifier names no design.
        assert!(choose(&insert("1b3f2c7e-0000-4000-8000-000000000000"), &listed).is_none());
        assert!(choose(&insert(""), &listed).is_none());
    }

    fn cube() -> Model {
        crate::read(
            Format::FusionArchive,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generated_unit_cube.f3d"),
            &mut Report::default(),
        )
        .unwrap()
    }

    /// A synthetic export folder: the assembly, the two designs it inserts and a manifest.
    fn export_folder() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for file in ["Asm v1.f3d", "Screw v2.f3d", "Plate v1.f3d"] {
            std::fs::write(dir.path().join(file), b"placeholder").unwrap();
        }
        let entry = |id: &str, name: &str, version: u64, references: Value| {
            json!({"id": id, "name": name, "version": version, "references": references,
                   "files": [{"path": format!("{name} v{version}.f3d")}]})
        };
        let manifest = json!({"schema": 2, "entries": [
            entry("urn:x:asm", "Asm", 1, json!([{"id": "urn:x:screw", "name": "Screw"},
                                                {"id": "urn:x:plate", "name": "Plate"}])),
            entry("urn:x:screw", "Screw", 2, json!([])),
            entry("urn:x:plate", "Plate", 1, json!([])),
        ]});
        std::fs::write(
            dir.path().join(export_folder::MANIFEST),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        dir
    }

    fn insertion(key: usize, name: &str, x: f64) -> Value {
        json!({"id": format!("synthetic:assembly:occurrence#{key}"), "ordinal": key, "name": name,
               "parent": {"kind": "root"}, "prototype": {"kind": "unresolved"},
               "transform": [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
               "scale": [1.0, 1.0, 1.0]})
    }

    #[test]
    fn insertions_are_placed_from_the_export_folder_at_their_transforms() {
        let dir = export_folder();
        let mut parent = cube();
        parent.ir.model.occurrences = serde_json::from_value(json!([
            insertion(1, "Screw v2:1", 10.0),
            insertion(2, "Screw v2:2", 20.0),
            insertion(3, "Plate v1:1", 30.0),
            // A role that names no design stays missing.
            insertion(4, "0f1e2d3c-0000-4000-8000-000000000000", 40.0),
        ]))
        .unwrap();
        parent.ir.model.product_definitions = Vec::new();
        let reads = std::cell::Cell::new(0);
        let decode = |_: Format, _: &Path, _: &mut Report| -> Result<Model, Error> {
            reads.set(reads.get() + 1);
            Ok(cube())
        };
        let finder = Finder::default();
        let mut report = Report::default();
        let external = resolve(
            &parent,
            &dir.path().join("Asm v1.f3d"),
            None,
            &mut report,
            &decode,
            &finder,
            &mut Vec::new(),
        );
        assert_eq!(external.len(), 3);
        assert_eq!(
            reads.get(),
            2,
            "each design is read once, however often it is placed"
        );
        assert!(!external.contains_key("synthetic:assembly:occurrence#4"));
        assert_eq!(
            crate::missing_linked_components(&parent, &external),
            ["0f1e2d3c-0000-4000-8000-000000000000"]
        );

        let (assembly, _) = local_components_with_threads(
            &parent,
            &external,
            1,
            crate::PartPositions::LastShown,
            &mut report,
        )
        .unwrap()
        .unwrap();
        let placed: Vec<(Option<&str>, f64)> = assembly
            .placements
            .iter()
            .filter(|placement| placement.name.is_some())
            .map(|placement| (placement.name.as_deref(), placement.transform[0][3]))
            .collect();
        assert_eq!(
            placed,
            [
                (Some("Screw v2:1"), 10.0),
                (Some("Screw v2:2"), 20.0),
                (Some("Plate v1:1"), 30.0)
            ]
        );
        // The two screws share one part; the plate and the assembly's own bodies make three.
        let screw = |name: &str| {
            assembly
                .placements
                .iter()
                .find(|placement| placement.name.as_deref() == Some(name))
                .unwrap()
                .part
        };
        assert_eq!(screw("Screw v2:1"), screw("Screw v2:2"));
        assert_ne!(screw("Screw v2:1"), screw("Plate v1:1"));
        assert_eq!(assembly.parts[screw("Screw v2:1")].name, "Screw");
    }

    #[test]
    fn a_listed_design_that_cannot_be_read_is_left_out_with_its_reason() {
        let dir = export_folder();
        let mut parent = cube();
        parent.ir.model.occurrences =
            serde_json::from_value(json!([insertion(1, "Screw v2:1", 10.0)])).unwrap();
        let decode = |_: Format, _: &Path, _: &mut Report| -> Result<Model, Error> {
            Err(Error::Decode("not a Fusion design".to_owned()))
        };
        let mut report = Report::default();
        let external = resolve(
            &parent,
            &dir.path().join("Asm v1.f3d"),
            None,
            &mut report,
            &decode,
            &Finder::default(),
            &mut Vec::new(),
        );
        let Some(Err(left)) = external.get("synthetic:assembly:occurrence#1") else {
            panic!("expected a left-out placement");
        };
        assert!(
            left.reason.starts_with("Screw v2.f3d could not be read"),
            "{}",
            left.reason
        );
        // It is accounted for, not reported again as a missing design.
        assert!(crate::missing_linked_components(&parent, &external).is_empty());
    }

    #[test]
    fn without_a_manifest_nothing_is_resolved() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Asm v1.f3d"), b"x").unwrap();
        let mut parent = cube();
        parent.ir.model.occurrences =
            serde_json::from_value(json!([insertion(1, "Screw v2:1", 10.0)])).unwrap();
        let decode = |_: Format, _: &Path, _: &mut Report| -> Result<Model, Error> { Ok(cube()) };
        let external = resolve(
            &parent,
            &dir.path().join("Asm v1.f3d"),
            None,
            &mut Report::default(),
            &decode,
            &Finder::default(),
            &mut Vec::new(),
        );
        assert!(external.is_empty());
    }
}
