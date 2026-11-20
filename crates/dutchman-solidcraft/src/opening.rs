//! Refuse to publish a design with a known failed timeline feature.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Write};

use dutchman_core::{Check, Error, FeatureStatus, Mode, Report, Severity};
use serde_json::Value;

use crate::{document, SolidCraftWriter};

impl SolidCraftWriter {
    pub(crate) fn check_opening(
        &self,
        mut bytes: Vec<u8>,
        report: &mut Report,
    ) -> Result<Vec<u8>, Error> {
        if self.memory_refusal().is_some() {
            return Ok(bytes);
        }
        let Ok(cli) = self.selected_evaluator(&self.verification) else {
            return Ok(bytes);
        };
        let started = dutchman_core::time::Instant::now();
        let mut repaired = false;
        for _ in 0..64 {
            let remaining = self.timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() || self.memory_refusal().is_some() {
                if repaired {
                    return Err(failure("the repaired design could not be checked"));
                }
                return Ok(bytes);
            }
            let result = match cli.evaluate(&bytes, remaining, "opening") {
                Ok(result) => result,
                Err(why) if repaired => return Err(failure(why)),
                Err(_) => return Ok(bytes),
            };
            if let Some(why) = result.get("evaluation_failed").and_then(Value::as_str) {
                return Err(failure(why));
            }
            let failures: Vec<_> = result
                .get("timeline")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|row| {
                    let why = row.get("error")?.as_str().filter(|why| !why.is_empty())?;
                    Some((row.clone(), why.to_owned()))
                })
                .collect();
            if failures.is_empty() {
                return Ok(bytes);
            }
            let mut archive =
                zip::ZipArchive::new(Cursor::new(bytes.as_slice())).map_err(failure)?;
            let mut doc: Value =
                serde_json::from_reader(archive.by_name("document.json").map_err(failure)?)
                    .map_err(failure)?;
            let design_name = doc
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Design")
                .to_owned();
            let component_names: BTreeMap<_, _> = doc
                .get("components")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|component| {
                    Some((
                        component.get("id")?.as_u64()?,
                        component.get("name")?.as_str()?.to_owned(),
                    ))
                })
                .collect();
            let component_parents: BTreeMap<_, _> = doc
                .get("components")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|component| {
                    let hidden =
                        component.get("history_hidden").and_then(Value::as_bool) == Some(true);
                    if !hidden {
                        return None;
                    }
                    Some((
                        component.get("id")?.as_u64()?,
                        component.get("parent").and_then(Value::as_u64).unwrap_or(0),
                    ))
                })
                .collect();
            let features = doc
                .get_mut("features")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| failure("the design has no feature list"))?;
            let mut removed = BTreeSet::new();
            for (row, why) in failures {
                let name = row.get("name").and_then(Value::as_str).unwrap_or("?");
                let matches = |feature: &Value| {
                    if let Some(id) = row.get("id").and_then(Value::as_u64) {
                        feature.get("id").and_then(Value::as_u64) == Some(id)
                    } else {
                        feature.get("name").and_then(Value::as_str) == Some(name)
                    }
                };
                let at = features.iter().position(matches).ok_or_else(|| {
                    failure(format!("cannot locate failed feature \"{name}\": {why}"))
                })?;
                let failed = features.get(at).ok_or_else(|| failure("missing feature"))?;
                let component = failed.get("component").cloned();
                let part_name = component
                    .as_ref()
                    .and_then(Value::as_u64)
                    .and_then(|id| component_names.get(&id))
                    .unwrap_or(&design_name);
                let parent = component
                    .as_ref()
                    .and_then(Value::as_u64)
                    .and_then(|id| component_parents.get(&id))
                    .copied();
                let is_import = failed.get("type").and_then(Value::as_str) == Some("import");
                // The normal replay repair already keeps a source prefix. This last check
                // also covers hidden prefixes, geometry-only designs and assembly imports.
                let original = features
                    .iter()
                    .skip(at + 1)
                    .find(|feature| {
                        let same_component = feature.get("component") == component.as_ref();
                        let visible_parent = parent.is_some_and(|parent| {
                            let in_parent = feature
                                .get("component")
                                .and_then(Value::as_u64)
                                .unwrap_or(0)
                                == parent;
                            let standalone_original = parent == 0
                                && feature.get("name").and_then(Value::as_str)
                                    == Some(document::ORIGINAL);
                            (in_parent || standalone_original)
                                && feature.get("suppressed").and_then(Value::as_bool) != Some(true)
                        });
                        feature.get("type").and_then(Value::as_str) == Some("import")
                            && ((same_component
                                && feature.get("name").and_then(Value::as_str)
                                    == Some(document::ORIGINAL))
                                || (!same_component && visible_parent))
                    })
                    .and_then(|feature| feature.get("id"))
                    .and_then(Value::as_u64);
                if !is_import && original.is_none() {
                    return Err(failure(format!(
                        "feature \"{name}\" failed: {why}; no final source shape is available"
                    )));
                }
                if original.is_some() {
                    // Retain the prefix for editing without displaying its unfinished
                    // bodies on top of the final source shape in this component.
                    for feature in features.iter_mut().take(at) {
                        if feature.get("component") == component.as_ref() {
                            feature
                                .as_object_mut()
                                .ok_or_else(|| failure("a feature is not an object"))?
                                .insert("suppressed".to_owned(), Value::Bool(true));
                        }
                    }
                }
                for feature in features.iter_mut().skip(at) {
                    if feature.get("component") != component.as_ref() {
                        continue;
                    }
                    let id = feature
                        .get("id")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| failure("a failed feature has no id"))?;
                    if Some(id) == original {
                        feature
                            .as_object_mut()
                            .ok_or_else(|| failure("a feature is not an object"))?
                            .insert("suppressed".to_owned(), Value::Bool(false));
                    } else {
                        removed.insert(id);
                    }
                }
                if let Some(original) = original {
                    if let Some(source) = features
                        .iter_mut()
                        .find(|feature| feature.get("id").and_then(Value::as_u64) == Some(original))
                    {
                        source
                            .as_object_mut()
                            .ok_or_else(|| failure("a feature is not an object"))?
                            .insert("suppressed".to_owned(), Value::Bool(false));
                    }
                }
                let reason = if original.is_some() {
                    format!("Feature \"{name}\" in \"{part_name}\" failed to rebuild: {why}; the final source shape is retained")
                } else {
                    format!("Feature \"{name}\" in \"{part_name}\" was not transferred because SolidCraft could not rebuild it: {why}; its geometry is left out")
                };
                report.push_coded(
                    if original.is_some() {
                        Severity::Degraded
                    } else {
                        Severity::Dropped
                    },
                    name,
                    if original.is_some() {
                        "history/rebuild-failed"
                    } else {
                        "geometry/import-failed"
                    },
                    &reason,
                );
                if let Some(outcome) = report.outcome_mut() {
                    for feature in &mut outcome.features {
                        if feature.output.ids.iter().any(|id| removed.contains(id)) {
                            feature.status = FeatureStatus::Blocked;
                            feature.reason = Some(reason.clone());
                            feature.output.ids.clear();
                            feature.output.method = dutchman_core::OutputMatch::NoOutput;
                        }
                    }
                    if outcome.mode == Mode::Import && original.is_none() {
                        outcome.check = Check::Flattened(reason);
                    } else {
                        outcome.mode = Mode::PartialHistory;
                        outcome.check = Check::Partial(reason);
                    }
                }
            }
            features.retain(|feature| {
                !feature
                    .get("id")
                    .and_then(Value::as_u64)
                    .is_some_and(|id| removed.contains(&id))
            });
            bytes = replace_document(archive, &doc)?;
            repaired = true;
        }
        Err(failure("the design exceeded the rebuild repair limit"))
    }
}

/// Copy our own generated payloads byte for byte; only the document changes.
fn replace_document(
    mut archive: zip::ZipArchive<Cursor<&[u8]>>,
    doc: &Value,
) -> Result<Vec<u8>, Error> {
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(failure)?;
        if entry.name().map_err(failure)? == "document.json" {
            continue;
        }
        output.raw_copy_file(entry).map_err(failure)?;
    }
    output
        .start_file(
            "document.json",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .map_err(failure)?;
    output
        .write_all(&serde_json::to_vec(doc).map_err(failure)?)
        .map_err(failure)?;
    Ok(output.finish().map_err(failure)?.into_inner())
}

fn failure(why: impl std::fmt::Display) -> Error {
    Error::Encode(format!(
        "cannot write a design that fails to open in SolidCraft: {why}"
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[derive(Debug)]
    struct RefusedImport;

    impl crate::SolidCraftEvaluator for RefusedImport {
        fn evaluate(&self, design: &[u8], _: Duration, _: &str) -> Result<Value, String> {
            let mut archive = zip::ZipArchive::new(Cursor::new(design)).unwrap();
            let doc: Value =
                serde_json::from_reader(archive.by_name("document.json").unwrap()).unwrap();
            let errors: Vec<_> = doc["features"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|feature| feature["id"] == 1)
                .map(|feature| {
                    json!({"id": feature["id"], "name": feature["name"],
                    "error": "Synthetic STEP refusal"})
                })
                .collect();
            Ok(json!({"bodies": [], "timeline": errors}))
        }

        fn run(&self, _: &[u8], _: &[Value], _: Duration, _: &str) -> Result<Vec<Value>, String> {
            Err("No commands expected".to_owned())
        }
    }

    #[test]
    fn a_late_rebuild_failure_retains_the_prefix_and_activates_the_final_source() {
        for source_component in [10, 20, 30] {
            let bytes = crate::container::Container::empty().finish(&json!({"features": [
            {"id": 0, "name": "Stored base", "type": "import", "component": 10, "suppressed": false},
            {"id": 1, "name": "Edit", "type": "offset_faces", "component": 10, "suppressed": false},
            {"id": 2, "name": "Original geometry", "type": "import", "component": source_component, "suppressed": source_component == 10}
        ], "components": [{"id": 10, "parent": if source_component == 30 { 0 } else { 20 }, "name": "Editable history (partial)", "history_hidden": true}]})).unwrap();
            let writer = SolidCraftWriter::new().evaluator(std::sync::Arc::new(RefusedImport));
            let mut report = Report::default();
            let bytes = writer.check_opening(bytes, &mut report).unwrap();
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let doc: Value =
                serde_json::from_reader(archive.by_name("document.json").unwrap()).unwrap();
            assert_eq!(doc["features"].as_array().unwrap().len(), 2);
            assert_eq!(doc["features"][0]["name"], "Stored base");
            assert_eq!(doc["features"][0]["suppressed"], true);
            assert_eq!(doc["features"][1]["name"], "Original geometry");
            assert_eq!(doc["features"][1]["suppressed"], false);
            assert!(report.losses().iter().any(|loss| loss.code.as_deref()
                == Some("history/rebuild-failed")
                && loss.detail.contains("Edit")
                && loss.detail.contains("Synthetic STEP refusal")));
        }
    }

    #[test]
    fn a_failed_import_removes_only_its_component_suffix_and_retains_other_parts() {
        let mut container = crate::container::Container::new(b"synthetic STEP payload").unwrap();
        let blob = container.step_blob().to_owned();
        container
            .add_companion("synthetic.sc", b"synthetic companion")
            .unwrap();
        let bytes = container
            .finish(&json!({"features": [
                {"id": 0, "name": "Before", "type": "sketch", "component": 10},
                {"id": 1, "name": "Body", "type": "import", "component": 10, "step_blob": blob},
                {"id": 2, "name": "Later", "type": "extrude", "component": 10},
                {"id": 3, "name": "Body", "type": "import", "component": 20, "step_blob": blob}
            ]}))
            .unwrap();
        let writer = SolidCraftWriter::new().evaluator(std::sync::Arc::new(RefusedImport));
        let mut report = Report::default();
        let bytes = writer.check_opening(bytes, &mut report).unwrap();
        assert_eq!(
            writer.check_opening(bytes.clone(), &mut report).unwrap(),
            bytes
        );
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let doc: Value =
            serde_json::from_reader(archive.by_name("document.json").unwrap()).unwrap();
        let ids: Vec<_> = doc["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].clone())
            .collect();
        assert_eq!(ids, [json!(0), json!(3)]);
        assert!(archive.by_name(&blob).is_ok());
        assert!(archive.by_name("companions/synthetic.sc").is_ok());
        assert!(report
            .losses()
            .iter()
            .any(|loss| loss.detail.contains("Body")
                && loss.detail.contains("not transferred")
                && loss.detail.contains("Synthetic STEP refusal")));
    }
}
