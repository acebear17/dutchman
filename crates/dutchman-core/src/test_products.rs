//! Authored synthetic CADIR product graphs for unit tests.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use serde_json::{json, Value};

pub(crate) const ROOT: &str = "f3d:model:component#root";

#[derive(Clone)]
pub(crate) struct Instance {
    pub owner: String,
    pub row: Value,
    pub definition: Option<Value>,
}

pub(crate) fn definition(id: &str, storage: Value) -> Value {
    json!({"id":id, "storage":storage, "bom_properties":{}, "bodies":[],
        "sketches":[], "tessellations":[], "occurrences":[],
        "instance_bodies":[], "representations":[]})
}

pub(crate) fn external(id: &str, prototype: &str, owner: &str, path: &str, x: f64) -> Instance {
    let storage = if path.is_empty() {
        json!({"kind":"external"})
    } else {
        json!({"kind":"external", "path":path})
    };
    Instance {
        owner: owner.to_owned(),
        definition: Some(definition(prototype, storage)),
        row: json!({"id":id, "prototype":prototype, "overrides":[],
            "transform":[[1.0,0.0,0.0,x],[0.0,1.0,0.0,0.0],[0.0,0.0,1.0,0.0]]}),
    }
}

/// List each authored occurrence in its owning definition, in source order.
pub(crate) fn set_placements(wire: &mut Value, instances: Vec<Instance>) {
    let definitions = wire["model"]["product_definitions"].as_array_mut().unwrap();
    if !definitions.iter().any(|d| d["storage"]["kind"] == "file") {
        definitions.push(definition(ROOT, json!({"kind":"file", "document":{}})));
    }
    for instance in &instances {
        if let Some(definition) = &instance.definition {
            if !definitions.iter().any(|d| d["id"] == definition["id"]) {
                definitions.push(definition.clone());
            }
        }
    }
    for definition in definitions.iter_mut() {
        definition["occurrences"] = json!([]);
    }
    for instance in &instances {
        let owner = definitions
            .iter_mut()
            .find(|d| d["id"] == instance.owner)
            .unwrap();
        owner["occurrences"]
            .as_array_mut()
            .unwrap()
            .push(instance.row["id"].clone());
    }
    wire["model"]["occurrences"] = json!(instances.into_iter().map(|i| i.row).collect::<Vec<_>>());
}
