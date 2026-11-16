//! Resolve region picks through their source sketch-entity disambiguation, not
//! deterministic-id ordering or a guess that the pick means the whole sketch.
use std::collections::BTreeSet;

use cadmpeg_ir::document::Model;
use serde_json::{json, Value};

use super::{id, query, rows, text, Mapping};

fn operation<'a>(query: &'a Value, suffix: &str) -> Mapping<&'a str> {
    let [operation] = rows(query, "operationId")? else {
        return Err("the sketch-region query has multiple source operations".into());
    };
    operation
        .as_str()
        .and_then(|s| s.strip_suffix(suffix))
        .ok_or_else(|| "the sketch-region query has no resolved sketch operation".into())
}

fn kind(query: &Value, entity: &str, query_type: &str) -> Mapping<()> {
    if query.get("entityType").and_then(Value::as_str) != Some(entity)
        || query.get("historyType").and_then(Value::as_str) != Some("CREATION")
        || query.get("queryType").and_then(Value::as_str) != Some(query_type)
    {
        return Err("this sketch-region boundary query is not supported yet".into());
    }
    Ok(())
}

pub(super) fn profile(queries: &[Value], model: &Model) -> Mapping<Value> {
    let mut sketch = None;
    let mut regions = Vec::new();
    let mut picks = BTreeSet::new();
    for source in queries {
        let [pick] = rows(source, "deterministicIds")? else {
            return Err(
                "a sketch-region query needs one boundary description per deterministic region"
                    .into(),
            );
        };
        let pick = pick
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("a sketch-region id is missing")?;
        if !picks.insert(pick) {
            return Err("the sketch-region selection repeats a deterministic id".into());
        }
        let resolved = query::resolved(source)?;
        kind(&resolved, "FACE", "IMPRINT")?;
        let native = operation(&resolved, ".imprint")?;
        let sketch_id = id("sketch", native);
        if !model.sketches.iter().any(|s| s.id.as_str() == sketch_id) {
            return Err("the selected sketch has no resolved geometry and plane".into());
        }
        if sketch.as_ref().is_some_and(|s| s != &sketch_id) {
            return Err("regions from multiple sketches are not supported yet".into());
        }
        sketch = Some(sketch_id.clone());
        let [disambiguation] = rows(&resolved, "disambiguationData")? else {
            return Err("the sketch region has no unique boundary disambiguation".into());
        };
        if text(disambiguation, "disambiguationType")? != "TOPOLOGY" {
            return Err("the sketch region has no topology boundary disambiguation".into());
        }
        let mut selected = BTreeSet::new();
        let mut edges = Vec::new();
        for boundary in rows(disambiguation, "entities")? {
            let [edge, side] = boundary
                .as_array()
                .map(Vec::as_slice)
                .ok_or("a sketch-region boundary is missing")?
            else {
                return Err("a sketch-region boundary has no directed edge".into());
            };
            if !matches!(side.as_i64(), Some(-1 | 1)) {
                return Err("a sketch-region boundary has no resolved side".into());
            }
            kind(edge, "EDGE", "IMPRINT")?;
            if operation(edge, ".imprint")? != native {
                return Err("a sketch-region boundary belongs to another sketch".into());
            }
            let derived = edge
                .get("derivedFrom")
                .ok_or("a sketch-region edge has no source sketch entity")?;
            kind(derived, "EDGE", "SKETCH_ENTITY")?;
            if operation(derived, ".wireOp")? != native {
                return Err("a sketch-region edge belongs to another sketch".into());
            }
            let entity_id = id(
                "entity",
                &format!("{native}/{}", text(derived, "sketchEntityId")?),
            );
            if !selected.insert(entity_id.clone()) {
                return Err("a sketch-region boundary repeats a source curve; its trimmed intervals are ambiguous".into());
            }
            let entity = model
                .sketch_entities
                .iter()
                .find(|e| e.id().as_str() == entity_id)
                .ok_or("a sketch-region boundary has no solved source curve")?;
            let entity = serde_json::to_value(entity)
                .map_err(|_| "a sketch-region source curve is unreadable")?;
            if entity.get("construction").and_then(Value::as_bool) == Some(true) {
                return Err("a sketch-region boundary selects construction geometry".into());
            }
            edges.push(Edge::new(entity_id, &entity)?);
        }
        let region = region(edges)?;
        if regions.contains(&region) {
            return Err("different sketch-region ids describe the same boundary".into());
        }
        regions.push(region);
    }
    Ok(
        json!({"kind": "sketch_regions", "value": {"sketch": sketch.ok_or("the sketch-region selection is empty")?, "regions": regions}}),
    )
}

struct Edge {
    id: String,
    a: [f64; 2],
    b: [f64; 2],
    range: [f64; 2],
    area: f64,
    closed: bool,
}

impl Edge {
    fn new(id: String, entity: &Value) -> Mapping<Self> {
        let geometry = entity
            .get("geometry")
            .ok_or("a sketch-region curve has no geometry")?;
        let point = |key| -> Mapping<[f64; 2]> {
            let point = geometry
                .get(key)
                .ok_or("a sketch-region endpoint is missing")?;
            Ok([super::number(point, "u")?, super::number(point, "v")?])
        };
        let (a, b, range, area, closed) = match text(geometry, "kind")? {
            "line" => {
                let a = point("start")?;
                let b = point("end")?;
                (a, b, [0.0, 1.0], cross(a, b), false)
            }
            kind @ ("circle" | "arc") => {
                let [cx, cy] = point("center")?;
                let radius = super::number(geometry, "radius")?;
                let from = if kind == "circle" {
                    0.0
                } else {
                    super::number(geometry, "start_angle")?
                };
                let mut to = if kind == "circle" {
                    std::f64::consts::TAU
                } else {
                    super::number(geometry, "end_angle")?
                };
                if to < from {
                    to += std::f64::consts::TAU;
                }
                let at = |t: f64| [cx + radius * t.cos(), cy + radius * t.sin()];
                let a = at(from);
                let b = if kind == "circle" { a } else { at(to) };
                let [ax, ay] = a;
                let [bx, by] = b;
                let area = cx * (by - ay) - cy * (bx - ax) + radius * radius * (to - from);
                (a, b, [from, to], area, kind == "circle")
            }
            _ => {
                return Err(
                    "the sketch-region source curve has no resolved boundary parameter range"
                        .into(),
                )
            }
        };
        if !area.is_finite() || a.iter().chain(&b).any(|n| !n.is_finite()) {
            return Err("the sketch-region boundary exceeds the finite range".into());
        }
        Ok(Self {
            id,
            a,
            b,
            range,
            area,
            closed,
        })
    }

    fn boundary(&self, reverse: bool) -> Value {
        json!({"entity": self.id, "parameter_range": self.range, "reversed": reverse})
    }
}

fn cross([a, b]: [f64; 2], [c, d]: [f64; 2]) -> f64 {
    a * d - b * c
}

fn distance([a, b]: [f64; 2], [c, d]: [f64; 2]) -> f64 {
    (a - c).hypot(b - d)
}

fn region(mut edges: Vec<Edge>) -> Mapping<Value> {
    // Order carriers by geometric adjacency. This introduces no joining
    // tolerance: the writer must independently close the directed intervals,
    // find an interior point and prove the replay against the source STEP.
    // Equal alternatives are ambiguous and must not depend on query order.
    let mut rings = Vec::new();
    while let Some(first) = edges.pop() {
        let start = first.a;
        let mut end = first.b;
        let mut area = first.area;
        let mut boundary = vec![first.boundary(false)];
        while !first.closed && !edges.is_empty() {
            let mut candidates = edges
                .iter()
                .enumerate()
                .filter(|(_, e)| !e.closed)
                .flat_map(|(i, e)| {
                    [
                        (distance(end, e.a), i, false),
                        (distance(end, e.b), i, true),
                    ]
                })
                .collect::<Vec<_>>();
            candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
            let Some(&(gap, index, reverse)) = candidates.first() else {
                break;
            };
            if distance(end, start) < gap {
                break;
            }
            if !gap.is_finite() || candidates.get(1).is_some_and(|other| other.0 == gap) {
                return Err(
                    "the sketch-region boundary does not form unambiguous closed loops".into(),
                );
            }
            let edge = edges.remove(index);
            end = if reverse { edge.a } else { edge.b };
            area += if reverse { -edge.area } else { edge.area };
            boundary.push(edge.boundary(reverse));
        }
        if !area.is_finite() || area == 0.0 {
            return Err("the sketch-region boundary has no resolved enclosed area".into());
        }
        rings.push((area.abs(), boundary));
    }
    // A single planar face has one exterior; other rings are its holes. The
    // writer checks their closure and nesting when choosing an interior point.
    rings.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (_, outer) = rings
        .pop()
        .ok_or("the sketch region has no resolved boundary edges")?;
    let holes = rings
        .into_iter()
        .map(|(_, boundary)| boundary)
        .collect::<Vec<_>>();
    Ok(json!({"region": "trimmed", "outer_boundary": outer, "hole_boundaries": holes}))
}
