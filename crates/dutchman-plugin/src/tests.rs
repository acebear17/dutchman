#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
use crate::{convert, resolve, Options};
use serde_json::Value;
use std::path::Path;
const CUBE: &[u8] = include_bytes!("../../dutchman-core/tests/fixtures/generated_unit_cube.f3d");
fn document(bytes: &[u8]) -> Value {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    serde_json::from_reader(archive.by_name("document.json").unwrap()).unwrap()
}
#[test]
fn native_options_and_structured_report() {
    let run = |params: &str| {
        convert(
            "cube.f3d",
            serde_json::from_str::<Options>(params).unwrap(),
            &|_, _| Ok(CUBE.into()),
            &|_, _| None,
            None,
        )
        .unwrap()
    };
    let (design, report) = run("{}");
    let report: Value = serde_json::from_str(&report).unwrap();
    assert!(report["summary"].as_str().unwrap().len() <= 240);
    assert!(report["items"].is_array());
    let details = &report["details"];
    assert_eq!(details["schema"], "dutchman.plugin.loss/1");
    assert_eq!(details["stages"].as_array().unwrap().len(), 6);
    assert!(details["outcome"]["verification"]["status"] != "verified");
    let (stood, _) = run(r#"{"up_axis":"stand_up"}"#);
    let original = document(&design);
    let stood = document(&stood);
    assert_eq!(
        stood["features"].as_array().unwrap().len(),
        original["features"].as_array().unwrap().len()
    );
    assert_eq!(
        stood["components"].as_array().unwrap().len(),
        original["components"].as_array().unwrap().len() + 1
    );
    for occurrence in stood["occurrences"].as_array().unwrap() {
        let parent = occurrence["parent"].as_u64().unwrap();
        assert!(
            parent == 0
                || stood["components"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c["id"] == parent),
            "occurrence parent must resolve to a component: {occurrence}"
        );
    }
    assert!(serde_json::from_str::<Options>(r#"{"up_axis":"other"}"#).is_err());
    assert!(serde_json::from_str::<Options>(r#"{"extra":true}"#).is_err());
}
#[test]
fn member_lookup_stays_relative_and_handles_stored_windows_names() {
    assert_eq!(
        resolve(r"C:\saved\part.ipt", Path::new(""), &|p| p
            == Path::new("part.ipt")),
        Some(std::path::PathBuf::from("part.ipt"))
    );
    assert_eq!(
        resolve(r"C:\project\parts\Pin.ipt", Path::new("assemblies"), &|p| p
            == Path::new("parts/Pin.ipt")),
        Some(std::path::PathBuf::from("parts/Pin.ipt"))
    );
    assert_eq!(
        resolve(".part.ipt", Path::new(""), &|p| p == Path::new(".part.ipt")),
        Some(std::path::PathBuf::from(".part.ipt"))
    );
    for name in ["../secret", "/etc/passwd", ".", "..", "C:secret"] {
        let found = resolve(name, Path::new(""), &|p| p == Path::new(name));
        assert!(found.is_none(), "{name}");
    }
}
#[test]
fn rejects_non_cad_inputs_and_bad_names() {
    assert!(crate::validate_name(".cube.f3d").is_ok());
    for name in ["../cube.f3d", "part.step", "", ".", "..", "part\0.f3d"] {
        assert!(convert(
            name,
            Options::default(),
            &|_, _| Ok(CUBE.into()),
            &|_, _| None,
            None
        )
        .is_err());
    }
}
#[test]
fn history_off_is_worded_as_the_plugin_option() {
    let (_, report) = convert(
        "cube.f3d",
        serde_json::from_str::<Options>(r#"{"editable_history":false}"#).unwrap(),
        &|_, _| Ok(CUBE.into()),
        &|_, _| None,
        None,
    )
    .unwrap();
    assert!(!report.contains("--import-only"), "{report}");
    let report: Value = serde_json::from_str(&report).unwrap();
    let verification = &report["details"]["outcome"]["verification"];
    assert_eq!(verification["status"], "not_applicable");
    assert_eq!(verification["message"], crate::EDITABLE_HISTORY_OFF);
    assert!(report["summary"]
        .as_str()
        .unwrap()
        .starts_with("Geometry only, as Keep editable history is off"));
}
#[test]
fn history_off_keeps_the_command_line_wording_elsewhere() {
    use dutchman_core::{Check, Report, Writer};
    let mut report = Report::default();
    let model = dutchman_core::source::read(
        dutchman_core::Format::FusionArchive,
        Path::new("cube.f3d"),
        false,
        dutchman_core::PartPositions::Smart,
        &|_, _| Ok(CUBE.into()),
        &|_, _| None,
        &mut report,
    )
    .unwrap();
    dutchman_solidcraft::SolidCraftWriter::new()
        .threads(1)
        .verification(dutchman_solidcraft::Verification::Off)
        .import_only(true)
        .write(&model, &mut Vec::new(), &mut report)
        .unwrap();
    assert_eq!(
        report.outcome().unwrap().check,
        Check::NotApplicable("--import-only".to_owned())
    );
}

#[test]
fn every_stood_up_assembly_occurrence_has_a_component_parent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dutchman-core/tests/fixtures");
    let (design, _) = convert(
        "nested_cubes.sldasm",
        Options {
            editable_history: false,
            up_axis: crate::UpAxis::StandUp,
            ..Options::default()
        },
        &|path, _| {
            std::fs::read(root.join(path)).map_err(|e| dutchman_core::Error::Decode(e.to_string()))
        },
        // The synthetic SolidWorks tree stores uppercase paths; its files are lowercase.
        &|stored, folder| {
            resolve(&stored.to_ascii_lowercase(), folder, &|path| {
                root.join(path).is_file()
            })
        },
        None,
    )
    .unwrap();
    let doc = document(&design);
    assert!(doc["occurrences"].as_array().unwrap().len() > 2);
    for occurrence in doc["occurrences"].as_array().unwrap() {
        let parent = occurrence["parent"].as_u64().unwrap();
        assert!(
            parent == 0
                || doc["components"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c["id"] == parent),
            "occurrence parent must resolve to a component: {occurrence}"
        );
    }
}

#[test]
fn the_report_lists_only_what_the_design_lost() {
    let (_, report) = convert(
        "cube.f3d",
        Options::default(),
        &|_, _| Ok(CUBE.into()),
        &|_, _| None,
        None,
    )
    .unwrap();
    let report: Value = serde_json::from_str(&report).unwrap();
    let items = report["items"].as_array().unwrap();
    let losses = report["details"]["losses"].as_array().unwrap();
    // Notes (part positions, STEP restatement records) stay in the details only.
    assert!(losses.len() > items.len(), "{report}");
    assert!(losses.iter().any(|loss| loss["severity"] == "info"));
    for item in items {
        assert_ne!(item["severity"], "info", "{item}");
        let title = item["title"].as_str().unwrap();
        assert!(!title.starts_with("Notes about"), "{title}");
        assert!(!title.starts_with("Parts of the file"), "{title}");
    }
    let summary = report["summary"].as_str().unwrap();
    // The cube has no feature history: that is not a loss.
    assert!(
        summary.starts_with("Shape converted; the file has no feature history to rebuild"),
        "{summary}"
    );
    match items.len() {
        0 => assert!(!summary.contains("loss"), "{summary}"),
        1 => assert!(summary.ends_with("; 1 loss"), "{summary}"),
        n => assert!(summary.ends_with(&format!("; {n} losses")), "{summary}"),
    }
}

#[test]
fn the_manifest_claims_exactly_the_plugin_formats() {
    let manifest: Value = serde_json::from_str(include_str!("../plugin.json")).unwrap();
    let mut claimed: Vec<String> = manifest["runtime"]["entries"][0]["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap().to_owned())
        .collect();
    claimed.sort();
    let mut formats: Vec<String> = crate::FORMATS
        .iter()
        .flat_map(|f| f.extensions().iter().map(|e| e.to_ascii_lowercase()))
        .collect();
    formats.sort();
    assert_eq!(claimed, formats);
    assert!(!claimed.iter().any(|e| e == "fcstd"));
    assert_eq!(crate::MAX_INPUT_BYTES, dutchman_core::MAX_INPUT_BYTES);
}

#[test]
fn freecad_is_left_to_solidcrafts_own_reader() {
    let error = convert(
        "design.FCStd",
        Options::default(),
        &|_, _| panic!("a FreeCAD file must not be read"),
        &|_, _| None,
        None,
    )
    .unwrap_err();
    assert_eq!(crate::message(&error), crate::FREECAD);
    assert!(crate::FREECAD.starts_with("Open FreeCAD files with File > Open"));
}

#[test]
fn messages_put_what_to_do_first_and_name_only_plugin_formats() {
    use dutchman_core::{Error, Format};
    let unknown = crate::message(&Error::UnknownFormat {
        path: "part.xyz".into(),
    });
    let rhino = crate::message(&Error::NotSupportedYet {
        format: Format::Rhino,
    });
    for text in [&unknown, &rhino] {
        assert!(
            text.starts_with(
                "Choose a Fusion (.f3d, .f3z), Inventor (.ipt, .iam) or SolidWorks \
                 (.sldprt, .sldasm) file"
            ),
            "{text}"
        );
        for other in ["FreeCAD", "Rhino", "CATIA", "Creo", "NX"] {
            assert!(!text.contains(other), "{text}");
        }
    }
    let large = crate::message(&Error::TooLarge {
        path: "big.f3d".into(),
        bytes: 3 << 29,
        limit: crate::MAX_INPUT_BYTES,
    });
    assert!(
        large.starts_with(
            "Choose a file under 1 GiB, or convert this one with the dutchman command line"
        ),
        "{large}"
    );
    assert!(large.ends_with("This file is 1.5 GiB."), "{large}");
    assert!(crate::input_limit_message(crate::MAX_INPUT_BYTES).contains(dutchman_core::RELEASES));
    // The reader's advice comes first in the plugin's words, then only the reader's cause.
    let truncated = convert(
        "cube.f3d",
        Options::default(),
        &|_, _| Ok(CUBE.get(..CUBE.len() / 2).unwrap().into()),
        &|_, _| None,
        None,
    )
    .unwrap_err();
    let text = crate::message(&truncated);
    assert!(
        text.starts_with("Download or copy the file again and open it once more."),
        "{text}"
    );
    assert!(text.contains(dutchman_core::FILE_DID_NOT_CONVERT), "{text}");
    assert_eq!(text.matches("download or copy").count(), 0, "{text}");
    assert!(text.contains("Detail: cannot read this design: "), "{text}");
}
