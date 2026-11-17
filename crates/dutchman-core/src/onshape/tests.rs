#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
use std::io::{Cursor, Read, Write};

use serde_json::{json, Value};

use super::*;

const FIXTURE: &[u8] =
    include_bytes!("../../tests/fixtures/onshape-default-plane-regions.onshape-bundle");

fn changed(mut edit: impl FnMut(&mut Value)) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(Cursor::new(FIXTURE)).unwrap();
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..archive.len() {
        let mut member = archive.by_index(i).unwrap();
        let name = member.name().unwrap().into_owned();
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).unwrap();
        if name.ends_with("features.json") {
            let mut response = serde_json::from_slice(&bytes).unwrap();
            edit(&mut response);
            bytes = serde_json::to_vec(&response).unwrap();
        }
        output
            .start_file(name.as_str(), zip::write::SimpleFileOptions::default())
            .unwrap();
        output.write_all(&bytes).unwrap();
    }
    output.finish().unwrap().into_inner()
}

fn read(bytes: &[u8]) -> (Value, Report) {
    let mut report = Report::default();
    let model = crate::read_bytes(
        Format::OnshapeBundle,
        std::path::Path::new("synthetic.onshape-bundle"),
        bytes,
        &mut report,
    )
    .unwrap();
    let mut wire = serde_json::to_value(model.ir.model).unwrap();
    wire["features"]
        .as_array_mut()
        .unwrap()
        .sort_by_key(|f| f["ordinal"].as_u64());
    (wire, report)
}

// A deliberately simple literal encoder, independent of the fixture generator's
// dictionary/reference encoder. Only synthetic records pass through this helper.
fn compact(value: &Value) -> String {
    match value {
        Value::String(s) => format!("S{:x}${s}", s.len()),
        Value::Number(n) => format!("D{:x}", n.as_i64().unwrap()),
        Value::Array(a) => format!(
            "A{:x}{}",
            a.len(),
            a.iter().map(compact).collect::<String>()
        ),
        Value::Object(o) => format!(
            "M{:x}{}",
            o.len(),
            o.iter()
                .map(|(k, v)| compact(&json!(k)) + &compact(v))
                .collect::<String>()
        ),
        _ => panic!("unsupported synthetic compact value"),
    }
}

fn replace_query(query: &mut Value, value: &Value) {
    query["queryString"] = json!(format!(
        "query=qCompressed(1.0,{},id);",
        serde_json::to_string(&format!("%{}", compact(value))).unwrap()
    ));
}

fn selected(response: &mut Value) -> &mut Value {
    &mut response["features"][1]["parameters"][2]["queries"][0]
}

fn kinds(wire: &Value) -> Vec<&str> {
    wire["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["definition"]["value"]["kind"].as_str().unwrap())
        .collect()
}

#[test]
fn real_shaped_synthetic_bundle_resolves_plane_and_selected_region() {
    let (wire, report) = read(FIXTURE);
    assert_eq!(kinds(&wire), ["sketch", "extrude"], "{:?}", report.losses());
    assert!(
        report.losses().iter().all(|loss| !loss
            .code
            .as_deref()
            .unwrap_or("")
            .starts_with("onshape/")),
        "{:?}",
        report.losses()
    );
    assert_eq!(
        wire["sketches"][0]["placement"]["frame"]["normal"],
        json!({"x":0.0,"y":0.0,"z":1.0})
    );
    let profile = &wire["features"][1]["definition"]["value"]["value"]["profile"]["value"];
    assert_eq!(profile["kind"], "sketch_regions");
    assert_eq!(
        profile["value"]["regions"][0]["outer_boundary"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        wire["features"][1]["definition"]["value"]["value"]["extent"]["side"]["termination"]
            ["length"],
        5.0
    );
}

#[test]
fn compressed_default_planes_resolve_their_own_full_frames() {
    for (plane, normal, u) in [
        ("Top", [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ("Front", [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]),
        ("Right", [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ] {
        let bytes = changed(|response| {
            let query = &mut response["features"][0]["parameters"][0]["queries"][0];
            let mut value = query::resolved(query).unwrap();
            value["operationId"] = json!([format!("{plane}.planeOp")]);
            replace_query(query, &value);
        });
        let (wire, report) = read(&bytes);
        assert_eq!(
            kinds(&wire),
            ["sketch", "extrude"],
            "{plane}: {:?}",
            report.losses()
        );
        let frame = &wire["sketches"][0]["placement"]["frame"];
        assert_eq!(frame["origin"], topology::point([0.0; 3]));
        assert_eq!(frame["normal"], topology::point(normal));
        assert_eq!(frame["u_axis"], topology::point(u));
    }
}

#[test]
fn several_regions_keep_their_boundaries_and_never_expand_to_the_whole_sketch() {
    let bytes = changed(|response| {
        let mut second = response["features"][0]["entities"].as_array().unwrap()[..4].to_vec();
        for entity in &mut second {
            for field in ["entityId", "startPointId", "endPointId"] {
                entity[field] = json!(format!("second.{}", entity[field].as_str().unwrap()));
            }
            entity["geometry"]["pntX"] = json!(entity["geometry"]["pntX"].as_f64().unwrap() + 0.03);
        }
        response["features"][0]["entities"]
            .as_array_mut()
            .unwrap()
            .extend(second);
        let first = selected(response);
        let mut value = query::resolved(first).unwrap();
        for boundary in value["disambiguationData"][0]["entities"]
            .as_array_mut()
            .unwrap()
        {
            let entity = &mut boundary[0]["derivedFrom"]["sketchEntityId"];
            *entity = json!(format!("second.{}", entity.as_str().unwrap()));
        }
        let mut query = first.clone();
        query["deterministicIds"] = json!(["second-region"]);
        replace_query(&mut query, &value);
        response["features"][1]["parameters"][2]["queries"]
            .as_array_mut()
            .unwrap()
            .push(query);
    });
    let (wire, report) = read(&bytes);
    assert_eq!(kinds(&wire), ["sketch", "extrude"], "{:?}", report.losses());
    let profile = &wire["features"][1]["definition"]["value"]["value"]["profile"]["value"];
    assert_eq!(profile["kind"], "sketch_regions");
    assert_eq!(profile["value"]["regions"].as_array().unwrap().len(), 2);
}

#[test]
fn ambiguous_region_queries_get_specific_losses_and_keep_the_sketch() {
    for (case, reason) in [
        ("source", "another sketch"),
        ("entity", "no solved source curve"),
        ("repeat", "repeats a source curve"),
        ("ids", "one boundary description"),
        ("disambiguation", "unique boundary"),
        ("side", "resolved side"),
    ] {
        let bytes = changed(|response| {
            let query = selected(response);
            let mut value = query::resolved(query).unwrap();
            let boundaries = &mut value["disambiguationData"][0]["entities"];
            match case {
                "source" => {
                    boundaries[0][0]["derivedFrom"]["operationId"] = json!(["different.wireOp"])
                }
                "entity" => boundaries[0][0]["derivedFrom"]["sketchEntityId"] = json!("absent"),
                "repeat" => {
                    let repeated = boundaries[0].clone();
                    boundaries.as_array_mut().unwrap().push(repeated);
                }
                "ids" => query["deterministicIds"] = json!(["one", "two"]),
                "disambiguation" => value["disambiguationData"] = json!([]),
                "side" => boundaries[0][1] = json!(0),
                _ => panic!("unknown test case"),
            }
            replace_query(query, &value);
        });
        let (wire, report) = read(&bytes);
        assert_eq!(kinds(&wire), ["sketch", "native"]);
        assert!(
            report
                .losses()
                .iter()
                .any(|loss| loss.detail.contains(reason)),
            "{case}: {:?}",
            report.losses()
        );
    }
}

#[test]
fn compressed_queries_are_bounded_and_truncation_never_panics() {
    let mut query = Value::Null;
    changed(|response| query = selected(response).clone());
    let original = query["queryString"].as_str().unwrap().to_owned();
    assert!(query::resolved(&query).is_ok());
    for n in 0..original.trim_end_matches(';').len() {
        query["queryString"] = json!(&original[..n]);
        assert!(query::resolved(&query).is_err());
    }
    for encoded in [
        "%Rffff",
        "%Affff",
        "%M1S1$kR1",
        "%M2S1$kD1R0D2",
        "&100001$eA==",
    ] {
        query["queryString"] = json!(format!(
            "query=qCompressed(1.0,{},id);",
            serde_json::to_string(encoded).unwrap()
        ));
        assert!(query::resolved(&query).is_err(), "{encoded}");
    }
}

#[test]
fn a_selected_region_excludes_other_closed_geometry_of_its_sketch() {
    let bytes = changed(|response| {
        let mut other = response["features"][0]["entities"].as_array().unwrap()[..4].to_vec();
        for entity in &mut other {
            for field in ["entityId", "startPointId", "endPointId"] {
                entity[field] = json!(format!("unused.{}", entity[field].as_str().unwrap()));
            }
            entity["geometry"]["pntX"] = json!(entity["geometry"]["pntX"].as_f64().unwrap() + 0.03);
        }
        response["features"][0]["entities"]
            .as_array_mut()
            .unwrap()
            .extend(other);
    });
    let (wire, report) = read(&bytes);
    assert_eq!(kinds(&wire), ["sketch", "extrude"], "{:?}", report.losses());
    let profile = &wire["features"][1]["definition"]["value"]["value"]["profile"]["value"];
    let boundary = profile["value"]["regions"][0]["outer_boundary"]
        .as_array()
        .unwrap();
    assert_eq!(boundary.len(), 4);
    assert!(boundary
        .iter()
        .all(|b| !b["entity"].as_str().unwrap().contains("unused")));
}

#[test]
fn a_region_can_keep_an_outer_circle_and_an_inner_hole() {
    let bytes = changed(|response| {
        response["features"][0]["entities"] = json!([
            {"btType":"BTMSketchCurve-4", "entityId":"outer", "geometry":{"btType":"BTCurveGeometryCircle-115", "xCenter":0.01, "yCenter":0.01, "radius":0.01}},
            {"btType":"BTMSketchCurve-4", "entityId":"inner", "geometry":{"btType":"BTCurveGeometryCircle-115", "xCenter":0.01, "yCenter":0.01, "radius":0.005}}
        ]);
        let query = selected(response);
        let mut value = query::resolved(query).unwrap();
        let mut boundary = value["disambiguationData"][0]["entities"][0].clone();
        boundary[0]["derivedFrom"]["sketchEntityId"] = json!("outer");
        let mut hole = boundary.clone();
        hole[0]["derivedFrom"]["sketchEntityId"] = json!("inner");
        value["disambiguationData"][0]["entities"] = json!([hole, boundary]);
        replace_query(query, &value);
    });
    let (wire, report) = read(&bytes);
    assert_eq!(kinds(&wire), ["sketch", "extrude"], "{:?}", report.losses());
    let region = &wire["features"][1]["definition"]["value"]["value"]["profile"]["value"]["value"]
        ["regions"][0];
    assert!(region["outer_boundary"][0]["entity"]
        .as_str()
        .unwrap()
        .ends_with("outer"));
    assert_eq!(region["hole_boundaries"].as_array().unwrap().len(), 1);
}

#[test]
fn compressed_reference_expansion_has_a_budget_and_a_depth_limit() {
    // The literal itself stays small while its backward references would make
    // the neutral JSON exponentially large without accounting for each clone.
    let mut literal = "%A29S1$x".to_owned();
    for i in 0..40 {
        literal.push_str(&format!("A2R{i:x}R{i:x}"));
    }
    let mut query = json!({});
    query["queryString"] = json!(format!(
        "query=qCompressed(1.0,{},id);",
        serde_json::to_string(&literal).unwrap()
    ));
    assert!(query::resolved(&query)
        .unwrap_err()
        .contains("reference limit"));
    let mut literal = "%A47S1$x".to_owned();
    for i in 0..70 {
        // Each completed array wraps the preceding cached value, keeping the
        // encoded nesting shallow while the referenced tree grows deeper.
        literal.push_str(&format!("A1R{i:x}"));
    }
    query["queryString"] = json!(format!(
        "query=qCompressed(1.0,{},id);",
        serde_json::to_string(&literal).unwrap()
    ));
    assert!(query::resolved(&query)
        .unwrap_err()
        .contains("nesting limit"));
}

#[test]
fn adjacent_regions_keep_the_shared_edge_in_each_directed_boundary() {
    let bytes = changed(|response| {
        let line = |name: &str, x, y, dx, dy| {
            json!({
                "btType":"BTMSketchCurveSegment-155", "entityId":name,
                "startParam":0.0, "endParam":0.01,
                "geometry":{"btType":"BTCurveGeometryLine-117", "pntX":x, "pntY":y, "dirX":dx, "dirY":dy}
            })
        };
        response["features"][0]["entities"] = json!([
            line("bottom-left", 0.0, 0.0, 1.0, 0.0),
            line("bottom-right", 0.01, 0.0, 1.0, 0.0),
            line("right", 0.02, 0.0, 0.0, 1.0),
            line("top-right", 0.02, 0.01, -1.0, 0.0),
            line("top-left", 0.01, 0.01, -1.0, 0.0),
            line("left", 0.0, 0.01, 0.0, -1.0),
            line("divider", 0.01, 0.0, 0.0, 1.0)
        ]);
        let original = selected(response).clone();
        let resolved = query::resolved(&original).unwrap();
        let edge = resolved["disambiguationData"][0]["entities"][0].clone();
        let mut queries = Vec::new();
        for (i, names) in [
            ["bottom-left", "divider", "top-left", "left"],
            ["bottom-right", "right", "top-right", "divider"],
        ]
        .iter()
        .enumerate()
        {
            let mut value = resolved.clone();
            value["disambiguationData"][0]["entities"] = Value::Array(
                names
                    .iter()
                    .map(|name| {
                        let mut edge = edge.clone();
                        edge[0]["derivedFrom"]["sketchEntityId"] = json!(name);
                        edge
                    })
                    .collect(),
            );
            let mut query = original.clone();
            query["deterministicIds"] = json!([format!("region-{i}")]);
            replace_query(&mut query, &value);
            queries.push(query);
        }
        response["features"][1]["parameters"][2]["queries"] = json!(queries);
    });
    let (wire, report) = read(&bytes);
    assert_eq!(kinds(&wire), ["sketch", "extrude"], "{:?}", report.losses());
    let regions = wire["features"][1]["definition"]["value"]["value"]["profile"]["value"]["value"]
        ["regions"]
        .as_array()
        .unwrap();
    assert_eq!(regions.len(), 2);
    for region in regions {
        let boundary = region["outer_boundary"].as_array().unwrap();
        assert_eq!(boundary.len(), 4);
        assert_eq!(
            boundary
                .iter()
                .filter(|b| b["entity"].as_str().unwrap().ends_with("divider"))
                .count(),
            1
        );
    }
}

#[test]
fn two_sketch_dimensions_get_distinct_ordinals_within_their_feature() {
    let bytes = changed(|response| {
        let dimension = |id: &str, line: &str, expression: &str| {
            json!({"btType": "BTMSketchConstraint-2", "entityId": id, "constraintType": "LENGTH",
                "parameters": [
                    {"btType": "BTMParameterString-149", "parameterId": "localFirst", "value": line},
                    {"btType": "BTMParameterQuantity-147", "parameterId": "length", "expression": expression}]})
        };
        response["features"][0]["constraints"] = json!([
            dimension("length", "outline.side0", "20 mm"),
            dimension("width", "outline.side1", "10 mm")
        ]);
    });
    let (wire, report) = read(&bytes);
    assert_eq!(kinds(&wire), ["sketch", "extrude"], "{:?}", report.losses());
    let parameters = wire["parameters"].as_array().unwrap();
    assert_eq!(parameters.len(), 2, "{parameters:?}");
    let mut ordinals: Vec<u64> = parameters
        .iter()
        .map(|p| p["ordinal"].as_u64().unwrap())
        .collect();
    ordinals.sort_unstable();
    assert_eq!(ordinals, [0, 1]);
    assert_eq!(parameters[0]["owner"], parameters[1]["owner"]);
}

#[test]
fn sketch_dimension_ordinals_restart_for_each_owner() {
    let bytes = changed(|response| {
        let first = &mut response["features"][0];
        first["constraints"] = json!([{
            "entityId": "length", "constraintType": "LENGTH", "parameters": [
                {"parameterId": "localFirst", "value": "outline.side0"},
                {"parameterId": "length", "expression": "20 mm"}
            ]
        }]);
        let mut other = first.clone();
        other["featureId"] = json!("otherSketch");
        other.as_object_mut().unwrap().remove("name");
        response["features"]
            .as_array_mut()
            .unwrap()
            .insert(1, other);
    });
    let (wire, report) = read(&bytes);
    assert_eq!(
        kinds(&wire),
        ["sketch", "sketch", "extrude"],
        "{:?}",
        report.losses()
    );
    let parameters = wire["parameters"].as_array().unwrap();
    assert_eq!(parameters.len(), 2, "{parameters:?}");
    assert_ne!(parameters[0]["owner"], parameters[1]["owner"]);
    assert!(parameters.iter().all(|parameter| parameter["ordinal"] == 0));
    let unnamed = wire["sketches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|sketch| sketch["id"] == id("sketch", "otherSketch"))
        .unwrap();
    assert!(unnamed.get("name").is_none());
}
