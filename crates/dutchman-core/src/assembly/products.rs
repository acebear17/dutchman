//! The definition-owned product graph, projected into the assembly builder's placement view.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::{MAX_DEPTH, MAX_PLACEMENTS};

/// The small structure-only view used to find external part files.
pub(crate) fn model_view(model: &cadmpeg_ir::document::Model) -> Result<Value, String> {
    let mut document = json!({"model": {
        "product_definitions": model.product_definitions,
        "occurrences": model.occurrences,
    }});
    project(&mut document)?;
    document
        .get_mut("model")
        .map(Value::take)
        .ok_or_else(|| "the model has no product structure".to_owned())
}

/// Geometry remains packed; only the structure and already-expanded history are changed.
pub(crate) fn project(document: &mut Value) -> Result<(), String> {
    let Some(model) = document.get_mut("model").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    let occurrences = model
        .get("occurrences")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if !occurrences
        .iter()
        .any(|row| row.get("prototype").is_some_and(Value::is_string))
    {
        return Ok(());
    }
    let definitions = model
        .get("product_definitions")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if occurrences.len() > MAX_PLACEMENTS || definitions.len() > MAX_PLACEMENTS {
        return Err("its product structure exceeds the component limit".to_owned());
    }
    let definitions: BTreeMap<_, _> = definitions
        .iter()
        .filter_map(|row| Some((row.get("id")?.as_str()?, row)))
        .collect();
    let occurrences: BTreeMap<_, _> = occurrences
        .iter()
        .filter_map(|row| Some((row.get("id")?.as_str()?, row)))
        .collect();
    let roots: Vec<_> = definitions
        .iter()
        .filter(|(_, row)| {
            row.pointer("/storage/kind").and_then(Value::as_str) == Some("file")
                && row.pointer("/storage/source").is_none_or(Value::is_null)
        })
        .map(|(id, _)| *id)
        .collect();
    let root = match roots.as_slice() {
        [root] => *root,
        _ => return Err("its product structure does not identify one root definition".to_owned()),
    };
    let mut graph = Graph {
        definitions: &definitions,
        occurrences: &occurrences,
        rows: Vec::new(),
        active: BTreeSet::new(),
        ids: BTreeSet::new(),
    };
    graph.visit(root, None, 0)?;
    let rows = std::mem::take(&mut graph.rows);
    let root = root.to_owned();
    model.insert("occurrences".to_owned(), Value::Array(rows));
    // The older builder represents the root's own bodies as loose bodies, beside its parts.
    if let Some(definitions) = model
        .get_mut("product_definitions")
        .and_then(Value::as_array_mut)
    {
        for definition in definitions {
            if definition.get("id").and_then(Value::as_str) == Some(root.as_str()) {
                if let Some(fields) = definition.as_object_mut() {
                    fields.insert("bodies".to_owned(), json!([]));
                }
            }
        }
    }
    for arena in ["features", "parameters"] {
        if let Some(rows) = model.get_mut(arena).and_then(Value::as_array_mut) {
            for row in rows {
                if row.get("product_definition").and_then(Value::as_str) == Some(root.as_str()) {
                    if let Some(fields) = row.as_object_mut() {
                        fields.remove("product_definition");
                    }
                }
            }
        }
    }
    Ok(())
}

struct Graph<'a> {
    definitions: &'a BTreeMap<&'a str, &'a Value>,
    occurrences: &'a BTreeMap<&'a str, &'a Value>,
    rows: Vec<Value>,
    active: BTreeSet<&'a str>,
    ids: BTreeSet<String>,
}

impl<'a> Graph<'a> {
    fn visit(
        &mut self,
        definition: &'a str,
        parent: Option<&str>,
        depth: usize,
    ) -> Result<(), String> {
        if depth >= MAX_DEPTH {
            return Err(
                "its component definitions nest deeper than the component limit".to_owned(),
            );
        }
        if !self.active.insert(definition) {
            return Err("its component definitions contain a placement cycle".to_owned());
        }
        let row = self
            .definitions
            .get(definition)
            .copied()
            .ok_or("a placed definition is not resolved")?;
        if row
            .get("instance_bodies")
            .and_then(Value::as_array)
            .is_some_and(|rows| !rows.is_empty())
        {
            return Err("component instance body overrides are not supported yet".to_owned());
        }
        for id in row
            .get("occurrences")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if self.rows.len() >= MAX_PLACEMENTS {
                return Err("its expanded product structure exceeds the component limit".to_owned());
            }
            let id = id
                .as_str()
                .ok_or("a component occurrence identity is not resolved")?;
            let source = self
                .occurrences
                .get(id)
                .copied()
                .ok_or("a component occurrence is not resolved")?;
            if source
                .get("overrides")
                .and_then(Value::as_array)
                .is_some_and(|rows| !rows.is_empty())
                || source
                    .get("representation")
                    .is_some_and(|value| !value.is_null())
            {
                return Err(
                    "component occurrence overrides and representations are not supported yet"
                        .to_owned(),
                );
            }
            let prototype = source
                .get("prototype")
                .and_then(Value::as_str)
                .ok_or("a component prototype is not resolved")?;
            let target = self
                .definitions
                .get(prototype)
                .copied()
                .ok_or("a component prototype is not resolved")?;
            let storage = target
                .get("storage")
                .ok_or("a component's storage is not resolved")?;
            let unresolved = storage.get("kind").and_then(Value::as_str) == Some("external");
            let loaded_file = storage.get("kind").and_then(Value::as_str) == Some("file");
            let document = if loaded_file {
                storage.get("document").unwrap_or(&Value::Null)
            } else {
                storage
            };
            let mut occurrence = source.clone();
            let fields = occurrence
                .as_object_mut()
                .ok_or("a component occurrence is malformed")?;
            let mut placed = id.to_owned();
            if !self.ids.insert(placed.clone()) {
                placed = format!("{id}:placement#{}", self.rows.len());
                if !self.ids.insert(placed.clone()) {
                    return Err("its expanded component identities are ambiguous".to_owned());
                }
            }
            fields.insert("id".to_owned(), json!(placed));
            fields.insert("ordinal".to_owned(), json!(self.rows.len()));
            fields.insert("scale".to_owned(), json!([1, 1, 1]));
            fields.insert(
                "parent".to_owned(),
                parent.map_or_else(
                    || json!({"kind":"root"}),
                    |parent| json!({"kind":"occurrence", "occurrence":parent}),
                ),
            );
            fields.insert(
                "prototype".to_owned(),
                if unresolved {
                    json!({"kind":"external", "document":document, "object":document.get("object")})
                } else if loaded_file {
                    json!({"kind":"local", "definition":prototype, "document":document})
                } else {
                    json!({"kind":"local", "definition":prototype})
                },
            );
            self.rows.push(occurrence);
            if !unresolved {
                self.visit(prototype, Some(&placed), depth + 1)?;
            }
        }
        self.active.remove(definition);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn definition(id: &str, children: Value, root: bool) -> Value {
        json!({"id":id, "bodies":[], "occurrences":children,
            "storage":if root {json!({"kind":"file", "document":{}})} else {json!({"kind":"embedded"})}})
    }

    #[test]
    fn a_shared_definition_expands_once_for_each_placement() {
        let mut wire = json!({"model":{
            "product_definitions":[definition("root",json!(["a","b"]),true),
                definition("sub",json!(["child"]),false),definition("part",json!([]),false)],
            "occurrences":[{"id":"a","prototype":"sub"},{"id":"b","prototype":"sub"},
                {"id":"child","prototype":"part"}],
            "features":[{"product_definition":"root"},{"product_definition":"part"}]
        }});
        project(&mut wire).unwrap();
        let rows = wire["model"]["occurrences"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1]["parent"]["occurrence"], "a");
        assert_eq!(rows[3]["parent"]["occurrence"], "b");
        assert_ne!(rows[1]["id"], rows[3]["id"]);
        assert!(wire["model"]["features"][0]
            .get("product_definition")
            .is_none());
        assert_eq!(wire["model"]["features"][1]["product_definition"], "part");
    }

    #[test]
    fn a_definition_cycle_is_refused() {
        let mut wire = json!({"model":{"product_definitions":[definition("root",json!(["a"]),true)],
            "occurrences":[{"id":"a","prototype":"root"}]}});
        assert!(project(&mut wire).unwrap_err().contains("cycle"));
    }

    #[test]
    fn an_external_definition_keeps_its_document_path() {
        let mut external = definition("part", json!([]), false);
        external["storage"] = json!({"kind":"external", "path":"synthetic.ipt"});
        let mut wire = json!({"model":{
            "product_definitions":[definition("root",json!(["a"]),true),external],
            "occurrences":[{"id":"a","prototype":"part"}]
        }});
        project(&mut wire).unwrap();
        assert_eq!(
            wire["model"]["occurrences"][0]["prototype"]["kind"],
            "external"
        );
        assert_eq!(
            wire["model"]["occurrences"][0]["prototype"]["document"]["path"],
            "synthetic.ipt"
        );
    }

    #[test]
    fn current_product_parts_still_become_repeated_assembly_parts() {
        let model = crate::read(
            crate::Format::InventorPart,
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/synthetic_cube.ipt"),
            &mut crate::Report::default(),
        )
        .unwrap();
        let mut ir = model.ir;
        let body = ir.geometry.bodies[0].id.clone();
        let mut root = definition(
            "synthetic:test:product#root",
            json!(["synthetic:test:occurrence#a", "synthetic:test:occurrence#b"]),
            true,
        );
        let mut part = definition("synthetic:test:product#part", json!([]), false);
        part["bodies"] = json!([body]);
        for row in [&mut root, &mut part] {
            row["bom_properties"] = json!({});
            row["tessellations"] = json!([]);
            row["instance_bodies"] = json!([]);
            row["representations"] = json!([]);
            row["sketches"] = json!([]);
        }
        ir.model.product_definitions = serde_json::from_value(json!([root, part])).unwrap();
        ir.model.occurrences = serde_json::from_value(json!([
            {"id":"synthetic:test:occurrence#a","prototype":"synthetic:test:product#part","transform":super::super::IDENTITY,"overrides":[]},
            {"id":"synthetic:test:occurrence#b","prototype":"synthetic:test:product#part","transform":super::super::IDENTITY,"overrides":[]}
        ])).unwrap();
        let model = crate::Model {
            ir,
            fidelity: None,
            name: "Synthetic".into(),
            source_file: None,
            assembly: None,
            source_step: None,
        };
        let assembly = super::super::local_assembly(&model).unwrap().unwrap();
        assert_eq!(assembly.placements.len(), 2);
        assert_eq!(assembly.parts.len(), 1);
    }
}
