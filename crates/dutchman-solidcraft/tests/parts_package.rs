//! A placed-parts package (`dutchman.parts/1`) read by Dutchman's package reader and written as a
//! SolidCraft design: parts as components and named occurrences, overlays as sheet bodies.
//!
//! The SolidCraft check runs when `SOLIDCRAFT_CLI` names a SolidCraft CLI. Set
//! `DUTCHMAN_PARTS_PACKAGE` to a package folder (with its `manifest.json`) to check a package
//! a module wrote; `DUTCHMAN_PARTS_OUT` then names where to keep the design.

// Test helpers outside `#[test]` functions are not covered by clippy.toml's test allowances.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::io::Cursor;
use std::path::{Path, PathBuf};

use cadmpeg_ir::codec::write::{target::TargetRequest, EncodeInput};
use cadmpeg_registry::{build_encoder, Format};
use dutchman_core::package::{read_package, PARTS_SCHEMA};
use dutchman_core::{Report, Writer};
use dutchman_solidcraft::{SolidCraftWriter, Verification, CLI_ENV};
use serde_json::{json, Value};
use zip::ZipArchive;

fn cube_step() -> Vec<u8> {
    let ir = cadmpeg_ir::examples::unit_cube().unwrap();
    let encoder = build_encoder(Format::Step);
    let (_, target) = encoder.targets().default().unwrap();
    let plan = encoder
        .plan(
            EncodeInput::new(&ir, None),
            TargetRequest::Explicit(target.id.as_str()),
        )
        .unwrap();
    let mut bytes = Vec::new();
    plan.write_to(&mut bytes).unwrap();
    bytes
}

fn document(bytes: &[u8]) -> Value {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    serde_json::from_reader(archive.by_name("document.json").unwrap()).unwrap()
}

fn evaluate(cli: &Path, design: &[u8]) -> Vec<Value> {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("design.solidcraft");
    let output = dir.path().join("eval.json");
    std::fs::write(&input, design).unwrap();
    let status = std::process::Command::new(cli)
        .arg("eval")
        .arg(&input)
        .arg("--json-out")
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success());
    let result: Value = serde_json::from_str(&std::fs::read_to_string(output).unwrap()).unwrap();
    result["bodies"].as_array().unwrap().clone()
}

fn write(dir: &Path, manifest: &[u8]) -> (Vec<u8>, Report) {
    let mut report = Report::default();
    let model = read_package(dir, manifest, &mut report).unwrap();
    let mut bytes = Vec::new();
    SolidCraftWriter::new()
        .verification(Verification::Off)
        .write(&model, &mut bytes, &mut report)
        .unwrap();
    (bytes, report)
}

fn cli() -> Option<PathBuf> {
    std::env::var_os(CLI_ENV).map(PathBuf::from)
}

#[test]
fn a_package_becomes_components_occurrences_and_overlay_sheets() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("board.step"), cube_step()).unwrap();
    std::fs::write(dir.path().join("part.step"), cube_step()).unwrap();
    let square = |x: f64, y: f64, s: f64| json!([[x, y], [x + s, y], [x + s, y + s], [x, y + s]]);
    let manifest = json!({
        "schema": PARTS_SCHEMA,
        "name": "Synthetic board",
        "units": "mm",
        "models": [{"id": "board", "file": "board.step"}, {"id": "cube", "file": "part.step"}],
        "parts": [
            {"name": "Board", "model": "board", "transform": [[1,0,0,0],[0,1,0,0],[0,0,1,0]]},
            {"name": "R1", "model": "cube", "transform": [[1,0,0,10],[0,1,0,0],[0,0,1,0]]},
            {"name": "R2", "model": "cube", "transform": [[0,-1,0,20],[1,0,0,0],[0,0,1,0]]},
        ],
        "overlays": [
            {
                "name": "Copper top", "kind": "copper", "finish": "masked", "side": "top",
                "part": "Board", "z": 1.001,
                "areas": [{"outer": square(0.0, 0.0, 0.9), "holes": [[[0.2, 0.2], [0.2, 0.7], [0.7, 0.7], [0.7, 0.2]]]}],
            },
            {
                "name": "Silkscreen top", "kind": "silkscreen", "side": "top",
                "part": "Board", "z": 1.01, "color": "#ffee00",
                "areas": [{"outer": square(0.1, 0.1, 0.1), "holes": []}, {"outer": square(0.5, 0.5, 0.1), "holes": []}],
            },
        ],
        "notes": [],
    });
    let (bytes, report) = write(dir.path(), manifest.to_string().as_bytes());
    let doc = document(&bytes);
    let names: Vec<&str> = doc["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Board", "R1", "R2", "Copper top", "Silkscreen top"]);
    // One component for each model and one for each overlay: the cube is one definition.
    assert_eq!(doc["components"].as_array().unwrap().len(), 4);
    assert!(
        report
            .losses()
            .iter()
            .all(|loss| loss.severity != dutchman_core::Severity::Dropped),
        "{:?}",
        report.losses()
    );
    // The overlays' colours reach the design: solder mask green and the silkscreen's own.
    let mut archive = ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
    let mut steps = String::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        if file.name().unwrap().starts_with("blobs/") {
            let mut text = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut text).unwrap();
            steps.push_str(&String::from_utf8_lossy(&text));
        }
    }
    assert!(
        steps.contains("COLOUR_RGB('',0.05,0.45,0.22)"),
        "mask green"
    );
    assert!(steps.contains("COLOUR_RGB('',1.,0.93"), "silkscreen colour");
    if let Some(cli) = cli() {
        let bodies = evaluate(&cli, &bytes);
        let named: Vec<&str> = bodies.iter().filter_map(|b| b["name"].as_str()).collect();
        assert!(named.contains(&"Copper top"), "{named:?}");
        assert!(named.contains(&"Silkscreen top"), "{named:?}");
        // Board, two resistors, and the two overlay sheets.
        assert_eq!(bodies.len(), 5, "{bodies:?}");
    }
}

#[test]
fn a_package_a_module_wrote() {
    let Some(dir) = std::env::var_os("DUTCHMAN_PARTS_PACKAGE").map(PathBuf::from) else {
        eprintln!("skipped: set DUTCHMAN_PARTS_PACKAGE to a package folder");
        return;
    };
    let manifest = std::fs::read(dir.join("manifest.json")).unwrap();
    let (bytes, report) = write(&dir, &manifest);
    for loss in report.losses() {
        println!("{loss}");
    }
    let doc = document(&bytes);
    println!(
        "{}",
        serde_json::to_string_pretty(&doc["occurrences"]).unwrap()
    );
    // The one-resistor package: R1's body (model box x -0.8..0.8, y -0.4..0.4, z 0..0.45) lands
    // at x 24.2..25.8, y 14.6..15.4, z 1.6..2.05 mm.
    let mut again = Report::default();
    let model = read_package(&dir, &manifest, &mut again).unwrap();
    let assembly = model.assembly.unwrap();
    let r1 = assembly
        .placements
        .iter()
        .find(|p| p.name.as_deref() == Some("R1"));
    if let Some(r1) = r1 {
        let (mut low, mut high) = ([f64::MAX; 3], [f64::MIN; 3]);
        for x in [-0.8, 0.8] {
            for y in [-0.4, 0.4] {
                for z in [0.0, 0.45] {
                    for axis in 0..3 {
                        let row = r1.transform[axis];
                        let v = row[0] * x + row[1] * y + row[2] * z + row[3];
                        low[axis] = f64::min(low[axis], v);
                        high[axis] = f64::max(high[axis], v);
                    }
                }
            }
        }
        for (got, want) in low
            .iter()
            .chain(high.iter())
            .zip([24.2, 14.6, 1.6, 25.8, 15.4, 2.05])
        {
            assert!((got - want).abs() < 0.01, "R1 box {low:?}..{high:?}");
        }
    } else {
        eprintln!("no part R1 in this package; the R1 position check is skipped");
    }
    if let Some(out) = std::env::var_os("DUTCHMAN_PARTS_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
    if let Some(cli) = cli() {
        let bodies = evaluate(&cli, &bytes);
        println!("{}", serde_json::to_string_pretty(&bodies).unwrap());
    }
}

#[test]
fn a_part_colour_reaches_the_design_as_a_body_look() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("part.step"), cube_step()).unwrap();
    let manifest = json!({
        "schema": PARTS_SCHEMA,
        "models": [{"id": "cube", "file": "part.step"}],
        "parts": [
            {"name": "Painted", "model": "cube", "color": "#336699"},
            {"name": "Plain", "model": "cube", "transform": [[1,0,0,5],[0,1,0,0],[0,0,1,0]]},
        ],
    });
    let (bytes, report) = write(dir.path(), manifest.to_string().as_bytes());
    assert!(
        report
            .losses()
            .iter()
            .all(|loss| loss.severity != dutchman_core::Severity::Dropped),
        "{:?}",
        report.losses()
    );
    let doc = document(&bytes);
    assert!(
        doc["appearances"]
            .to_string()
            .to_lowercase()
            .contains("336699"),
        "{}",
        doc["appearances"]
    );
}
