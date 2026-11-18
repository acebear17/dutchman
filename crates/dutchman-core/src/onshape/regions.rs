//! Resolve region picks through their source sketch-entity disambiguation, not
//! deterministic-id ordering or a guess that the pick means the whole sketch.
use std::collections::BTreeSet;

use cadmpeg_ir::document::Model;
use serde_json::{json, Value};

use super::{id, query, rows, text, topology, Mapping};

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

pub(super) fn profile(
    queries: &[Value],
    model: &Model,
    inputs: &topology::Inputs<'_>,
) -> Mapping<Value> {
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
        let mut outside = true;
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
            outside &= side.as_i64() == Some(-1);
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
            let source_id = text(derived, "sketchEntityId")?;
            let entity_id = id("entity", &format!("{native}/{source_id}"));
            let entity = model
                .sketch_entities
                .iter()
                .find(|e| e.id().as_str() == entity_id)
                .ok_or("a sketch-region boundary has no solved source curve")?;
            let mut entity = serde_json::to_value(entity)
                .map_err(|_| "a sketch-region source curve is unreadable")?;
            if entity.get("construction").and_then(Value::as_bool) == Some(true) {
                return Err("a sketch-region boundary selects construction geometry".into());
            }
            outside &= match entity.pointer("/geometry/kind").and_then(Value::as_str) {
                Some("circle") => true,
                Some("ellipse") => entity
                    .pointer("/geometry/bounds")
                    .is_none_or(Value::is_null),
                _ => false,
            };
            if edge.get("disambiguationData").is_some() {
                let range = ellipse_interval(edge, native, source_id, inputs)?;
                entity
                    .get_mut("geometry")
                    .and_then(Value::as_object_mut)
                    .ok_or("the sketch-region ellipse geometry is missing")?
                    .insert("bounds".to_owned(), json!(range));
            }
            let resolved = Edge::new(entity_id.clone(), &entity)?;
            // Repeated carriers are legitimate when the query identifies distinct
            // intervals. Repeating an interval still has no unique boundary.
            let key = (entity_id, resolved.range.map(f64::to_bits));
            if !selected.insert(key) {
                return Err("a sketch-region boundary repeats a source curve; its trimmed intervals are ambiguous".into());
            }
            edges.push(resolved);
        }
        if edges.is_empty() {
            return Err("the sketch region has no resolved boundary edges".into());
        }
        let region = if outside {
            exterior_region(&sketch_id, &edges, model)?
        } else {
            region(edges)?
        };
        if regions.contains(&region) {
            return Err("different sketch-region ids describe the same boundary".into());
        }
        regions.push(region);
    }
    Ok(
        json!({"kind": "sketch_regions", "value": {"sketch": sketch.ok_or("the sketch-region selection is empty")?, "regions": regions}}),
    )
}

/// An imprint edge's TOPOLOGY record names its start (+1) and end (-1).
/// INTERSECT/ORDER ranks the intersections along the first source carrier's
/// native angular chart. Only solved chord endpoints on a full ellipse are
/// admitted here: no intersection solver, nearest-point choice or final body.
fn ellipse_interval(
    edge: &Value,
    native: &str,
    source: &str,
    inputs: &topology::Inputs<'_>,
) -> Mapping<[f64; 2]> {
    let refused = "the sketch-region intersection trimming is not supported yet";
    let entity = inputs.sketch_entity(native, source)?;
    if text(entity, "btType")? != "BTMSketchCurve-4" {
        return Err(refused.into());
    }
    let geometry = entity.get("geometry").ok_or(refused)?;
    if text(geometry, "btType")? != "BTCurveGeometryEllipse-1189" {
        return Err(refused.into());
    }
    let cx = super::coordinate(geometry, "xCenter")? * 1000.0;
    let cy = super::coordinate(geometry, "yCenter")? * 1000.0;
    let angle = super::coordinate(geometry, "yDir")?.atan2(super::coordinate(geometry, "xDir")?);
    let major = super::number(geometry, "radius")? * 1000.0;
    let minor = super::number(geometry, "minorRadius")? * 1000.0;
    let clockwise = geometry.get("clockwise").and_then(Value::as_bool) == Some(true);
    let sign = if clockwise { -1.0 } else { 1.0 };
    let [data] = rows(edge, "disambiguationData")? else {
        return Err(refused.into());
    };
    if text(data, "disambiguationType")? != "TOPOLOGY" {
        return Err(refused.into());
    }
    let mut start = None;
    let mut end = None;
    for vertex in rows(data, "entities")? {
        let [vertex, side] = vertex.as_array().map(Vec::as_slice).ok_or(refused)? else {
            return Err(refused.into());
        };
        kind(vertex, "VERTEX", "INTERSECT")?;
        if operation(vertex, ".imprint")? != native {
            return Err(refused.into());
        }
        let [first, second] = rows(vertex, "derivedFrom")? else {
            return Err(refused.into());
        };
        for curve in [first, second] {
            kind(curve, "EDGE", "SKETCH_ENTITY")?;
            if operation(curve, ".wireOp")? != native {
                return Err(refused.into());
            }
        }
        if text(first, "sketchEntityId")? != source {
            return Err(refused.into());
        }
        let chord = super::sketch::geometry(
            inputs.sketch_entity(native, text(second, "sketchEntityId")?)?,
        )?;
        if text(&chord, "kind")? != "line" {
            return Err(refused.into());
        }
        let mut candidates = Vec::new();
        for key in ["start", "end"] {
            let point = chord.get(key).ok_or(refused)?;
            let x = super::number(point, "u")? - cx;
            let y = super::number(point, "v")? - cy;
            let u = (angle.cos() * x + angle.sin() * y) / major;
            let v = (-angle.sin() * x + angle.cos() * y) / minor;
            let residual = (u * u + v * v - 1.0).abs();
            // A roundoff bound, not a joining tolerance. Endpoint residuals
            // measured for this change are recorded in the review notes.
            if !residual.is_finite() || residual > 32.0 * f64::EPSILON {
                return Err(
                    "the sketch-region intersection is not a solved ellipse endpoint".into(),
                );
            }
            candidates.push((sign * v.atan2(u)).rem_euclid(std::f64::consts::TAU));
        }
        candidates.sort_by(f64::total_cmp);
        let [a, b] = candidates.as_slice() else {
            return Err(refused.into());
        };
        if a == b {
            return Err("the sketch-region intersection order is ambiguous".into());
        }
        let [order] = rows(vertex, "disambiguationData")? else {
            return Err(refused.into());
        };
        if text(order, "disambiguationType")? != "ORDER" {
            return Err(refused.into());
        }
        let parameter = match order.get("order").and_then(Value::as_u64) {
            Some(0) => *a,
            Some(1) => *b,
            _ => return Err(refused.into()),
        };
        let target = match side.as_i64() {
            Some(1) => &mut start,
            Some(-1) => &mut end,
            _ => return Err(refused.into()),
        };
        if target.replace(parameter).is_some() {
            return Err(refused.into());
        }
    }
    let (start, end) = (start.ok_or(refused)?, end.ok_or(refused)?);
    let (start, mut end) = if clockwise {
        (-end, -start)
    } else {
        (start, end)
    };
    if end < start {
        end += std::f64::consts::TAU;
    }
    if end == start {
        return Err("the sketch-region intersection interval is ambiguous".into());
    }
    Ok([start, end])
}

struct Edge {
    id: String,
    a: [f64; 2],
    b: [f64; 2],
    range: [f64; 2],
    area: f64,
    closed: bool,
    geometry: Value,
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
            "ellipse" => {
                let [cx, cy] = point("center")?;
                let major = super::number(geometry, "major_radius")?;
                let minor = super::number(geometry, "minor_radius")?;
                let angle = super::number(geometry, "major_angle")?;
                let bounds = geometry.get("bounds").filter(|v| !v.is_null());
                let (from, to) = if let Some(bounds) = bounds {
                    let bounds = bounds.as_array().ok_or("ellipse bounds are missing")?;
                    let from = bounds
                        .first()
                        .and_then(Value::as_f64)
                        .ok_or("ellipse start is missing")?;
                    let to = bounds
                        .get(1)
                        .and_then(Value::as_f64)
                        .ok_or("ellipse end is missing")?;
                    (from, to)
                } else {
                    (0.0, std::f64::consts::TAU)
                };
                let at = |t: f64| {
                    [
                        cx + major * angle.cos() * t.cos() - minor * angle.sin() * t.sin(),
                        cy + major * angle.sin() * t.cos() + minor * angle.cos() * t.sin(),
                    ]
                };
                let a = at(from);
                let b = if bounds.is_none() { a } else { at(to) };
                let [ax, ay] = a;
                let [bx, by] = b;
                let area = cx * (by - ay) - cy * (bx - ax) + major * minor * (to - from);
                (a, b, [from, to], area, bounds.is_none())
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
            geometry: geometry.clone(),
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
    Ok(json!({"kind": "trimmed", "outer_boundary": outer, "hole_boundaries": holes}))
}

struct Ring {
    edges: Vec<(Edge, bool)>,
    area: f64,
}

impl Ring {
    fn boundary(&self) -> Vec<Value> {
        self.edges.iter().map(|(e, r)| e.boundary(*r)).collect()
    }

    fn contains(&self, [x, y]: [f64; 2]) -> Mapping<bool> {
        let mut crossings = 0;
        for (edge, _) in &self.edges {
            let g = &edge.geometry;
            if text(g, "kind")? == "line" {
                let [ax, ay] = edge.a;
                let [bx, by] = edge.b;
                if (ay > y) != (by > y) && ax + (y - ay) * (bx - ax) / (by - ay) > x {
                    crossings += 1;
                }
                continue;
            }
            let center = g.get("center").ok_or("the region carrier has no center")?;
            let cx = super::number(center, "u")?;
            let cy = super::number(center, "v")?;
            let (a, b, angle) = if text(g, "kind")? == "ellipse" {
                (
                    super::number(g, "major_radius")?,
                    super::number(g, "minor_radius")?,
                    super::number(g, "major_angle")?,
                )
            } else {
                let radius = super::number(g, "radius")?;
                (radius, radius, 0.0)
            };
            let ax = a * angle.cos();
            let ay = a * angle.sin();
            let bx = -b * angle.sin();
            let by = b * angle.cos();
            let level = (y - cy) / ay.hypot(by);
            if level.abs() >= 1.0 {
                continue;
            }
            let phase = by.atan2(ay);
            let [from, to] = edge.range;
            for root in [phase - level.acos(), phase + level.acos()] {
                let t = from + (root - from).rem_euclid(std::f64::consts::TAU);
                if t < to && cx + ax * t.cos() + bx * t.sin() > x {
                    crossings += 1;
                }
            }
        }
        Ok(crossings % 2 == 1)
    }
}

/// TOPOLOGY disambiguation is a witness list, not necessarily the complete
/// face boundary. A face outside every named closed carrier also needs its
/// enclosing source loop and the other enclosed holes, even if not witnessed.
/// Recover only isolated, endpoint-connected loops; a branch is a named loss.
fn exterior_region(sketch: &str, witnesses: &[Edge], model: &Model) -> Mapping<Value> {
    let refused = "the outside sketch region has no unique enclosing source loop";
    let mut edges = Vec::new();
    for entity in &model.sketch_entities {
        if entity.sketch.as_str() != sketch || entity.construction {
            continue;
        }
        let wire = serde_json::to_value(entity).map_err(|_| refused)?;
        if wire.pointer("/geometry/kind").and_then(Value::as_str) == Some("point") {
            continue;
        }
        edges.push(Edge::new(entity.id().as_str().to_owned(), &wire)?);
    }
    // Permit only f64 arithmetic roundoff. Source-loop closure measurements
    // for this change are recorded in the review notes.
    let scale = edges
        .iter()
        .flat_map(|e| e.a.into_iter().chain(e.b))
        .map(f64::abs)
        .fold(1.0_f64, f64::max);
    let roundoff = 64.0 * f64::EPSILON * scale;
    let mut rings = Vec::new();
    while let Some(first) = edges.pop() {
        let start = first.a;
        let mut end = first.b;
        let mut area = first.area;
        let mut closed = first.closed || distance(start, end) <= roundoff;
        let mut members = vec![(first, false)];
        while !closed {
            let candidates: Vec<_> = edges
                .iter()
                .enumerate()
                .filter(|(_, e)| !e.closed)
                .flat_map(|(i, e)| {
                    [
                        (i, false, distance(end, e.a)),
                        (i, true, distance(end, e.b)),
                    ]
                })
                .filter(|(_, _, gap)| *gap <= roundoff)
                .collect();
            let [(index, reverse, _)] = candidates.as_slice() else {
                if candidates.is_empty() {
                    break;
                }
                return Err("the outside sketch region has tied source-loop connections".into());
            };
            let edge = edges.remove(*index);
            end = if *reverse { edge.a } else { edge.b };
            area += if *reverse { -edge.area } else { edge.area };
            members.push((edge, *reverse));
            closed = distance(start, end) <= roundoff;
        }
        if closed {
            rings.push(Ring {
                edges: members,
                area: area.abs(),
            });
        }
    }
    let mut candidates = Vec::new();
    for (index, ring) in rings.iter().enumerate() {
        // A witnessed carrier cannot itself be the enclosing boundary of the
        // face on its outside. Every witness point must lie inside the loop.
        if ring
            .edges
            .iter()
            .any(|(e, _)| witnesses.iter().any(|w| w.id == e.id))
        {
            continue;
        }
        if witnesses
            .iter()
            .map(|w| ring.contains(w.a))
            .collect::<Mapping<Vec<_>>>()?
            .iter()
            .all(|v| *v)
        {
            candidates.push(index);
        }
    }
    // Multiple enclosing loops need more topology evidence. Do not choose
    // between them by area or source order.
    let [index] = candidates.as_slice() else {
        return Err(refused.into());
    };
    let index = *index;
    let outer = rings.get(index).ok_or(refused)?;
    let mut holes = Vec::new();
    for (i, ring) in rings.iter().enumerate() {
        if i == index {
            continue;
        }
        let point = ring.edges.first().ok_or(refused)?.0.a;
        if !outer.contains(point)? {
            continue;
        }
        let mut nested = false;
        for (j, other) in rings.iter().enumerate() {
            if j != i
                && j != index
                && other.area > ring.area
                && other.area < outer.area
                && other.contains(point)?
            {
                nested = true;
                break;
            }
        }
        if !nested {
            holes.push(ring.boundary());
        }
    }
    Ok(json!({"kind":"trimmed","outer_boundary":outer.boundary(),"hole_boundaries":holes}))
}
