//! The job side of Export from Fusion against a fake add-in that follows the protocol.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::fake::{sample_inventory, Ending, FakeAddin, Service, FEATURES};
use super::inventory::Inventory;
use super::job::{self, Kind, Next, Update};
use super::{Env, Panel, Stage};

const FAST: Duration = Duration::from_millis(10);

fn setup() -> (tempfile::TempDir, FakeAddin) {
    let dir = tempfile::tempdir().unwrap();
    let addin = FakeAddin::new(&dir.path().join(".cad-export"));
    addin.announce(FEATURES, None);
    (dir, addin)
}

fn designs() -> Vec<String> {
    vec![
        "urn:adsk.wipprod:dm.lineage:a1".into(),
        "urn:adsk.wipprod:dm.lineage:a2".into(),
    ]
}

fn export_job(dir: &Path) -> (String, Value) {
    let id = job::new_id();
    let request = job::build_job(&id, Kind::Export, &dir.join("out"), &designs());
    (id, request)
}

#[test]
fn a_job_moves_from_queued_to_running_to_done_and_its_files_are_cleaned_up() {
    let (dir, addin) = setup();
    let (id, request) = export_job(dir.path());
    let sent = job::submit(&addin.agent(), Kind::Export, &request).unwrap();
    assert_eq!(sent, id);
    assert!(job::read_status(&addin.jobs_dir(), &id).is_none());
    let (sender, receiver) = channel();
    job::follow(
        addin.jobs_dir(),
        id.clone(),
        Default::default(),
        FAST,
        sender,
        || {},
    );
    let out = dir.path().join("out");
    assert!(addin.claim(&id));
    let next = || receiver.recv_timeout(Duration::from_secs(5)).unwrap().1;
    let Update::Status(queued) = next() else {
        panic!()
    };
    assert_eq!(queued.state, "queued");
    addin.status(
        &id,
        "running",
        None,
        "Exporting design 1 of 3.",
        (0, Some(3), Some(1)),
        0,
        0,
        &out,
    );
    let Update::Status(running) = next() else {
        panic!()
    };
    assert_eq!(
        (running.done, running.total, running.current_number),
        (0, Some(3), Some(1))
    );
    addin.status(
        &id,
        "done",
        None,
        "Exported 3 designs.",
        (3, Some(3), None),
        3,
        0,
        &out,
    );
    let Update::Final(done) = next() else {
        panic!()
    };
    assert_eq!(
        (done.state.as_str(), done.exported, done.failed),
        ("done", 3, 0)
    );
    // After a final state the job's three files are gone.
    for name in [
        format!("{id}.json"),
        format!("{id}.status.json"),
        format!("{id}.cancel"),
    ] {
        assert!(!addin.jobs_dir().join(name).exists());
    }
}

#[test]
fn the_job_is_written_as_the_spec_says_and_never_seen_half_written() {
    let (dir, addin) = setup();
    let (id, request) = export_job(dir.path());
    job::submit(&addin.agent(), Kind::Export, &request).unwrap();
    let written = addin.job(&id);
    assert_eq!(written["protocol"], 1);
    assert_eq!(written["action"], "export");
    assert_eq!(written["designs"], json!(designs()));
    assert_eq!(written["include_references"], true);
    assert_eq!(written["formats"], json!(["f3d", "step"]));
    assert_eq!(written["rebuilt_f3z"], true);
    assert_eq!(written["older_versions"], "move");
    assert!(written["requested"].as_str().unwrap().ends_with("+00:00"));
    // The temporary file is renamed away: only the final name is left.
    let names: Vec<String> = std::fs::read_dir(addin.jobs_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec![format!("{id}.json")]);
    assert_eq!(addin.unclaimed(), vec![id]);
}

#[test]
fn an_inventory_job_asks_for_the_list_and_nothing_else() {
    let (dir, _addin) = setup();
    let request = job::build_job("inv-1", Kind::Inventory, dir.path(), &[]);
    assert_eq!(request["action"], "inventory");
    assert!(request.get("designs").is_none());
    job::validate(&request).unwrap();
}

#[test]
fn an_invalid_job_is_refused_before_it_is_written() {
    let (dir, addin) = setup();
    let (_, good) = export_job(dir.path());
    let broken = |edit: &dyn Fn(&mut Value)| {
        let mut request = good.clone();
        edit(&mut request);
        request
    };
    for request in [
        broken(&|j| j["surprise"] = json!(1)),
        broken(&|j| j["id"] = json!("bad id/../x")),
        broken(&|j| j["id"] = json!("x.status")),
        broken(&|j| j["output_dir"] = json!("relative/path")),
        broken(&|j| j["designs"] = json!([])),
        broken(&|j| j["formats"] = json!(["step"])),
        broken(&|j| j["formats"] = json!(["f3d", "f3d"])),
        broken(&|j| j["protocol"] = json!(2)),
        broken(&|j| j["hubs"] = json!(["h"])),
        broken(&|j| j["older_versions"] = json!("shred")),
    ] {
        assert!(submit_ok(&addin, &request).is_err(), "{request}");
    }
    assert!(addin.unclaimed().is_empty());
    job::validate(&good).unwrap();
}

fn submit_ok(addin: &FakeAddin, request: &Value) -> Result<String, String> {
    job::submit(&addin.agent(), Kind::Export, request)
}

#[test]
fn an_invalid_job_that_got_through_is_answered_failed_with_invalid_job() {
    let (dir, addin) = setup();
    let id = "hand-written";
    std::fs::write(
        addin.jobs_dir().join(format!("{id}.json")),
        json!({"protocol": 1, "id": id, "requested": "x", "output_dir": dir.path(), "wat": 1})
            .to_string(),
    )
    .unwrap();
    assert!(!addin.claim(id));
    let status = job::read_status(&addin.jobs_dir(), id).unwrap();
    assert_eq!(
        (status.state.as_str(), status.reason.as_deref()),
        ("failed", Some("invalid_job"))
    );
    assert!(status.is_final());
}

#[test]
fn an_exporter_without_a_needed_feature_is_not_sent_the_job() {
    let (dir, addin) = setup();
    addin.announce(&["export", "cancel"], None);
    let (_, request) = export_job(dir.path());
    let why = job::submit(&addin.agent(), Kind::Export, &request).unwrap_err();
    assert!(why.contains("designs_by_id"), "{why}");
    assert!(addin.unclaimed().is_empty());
}

#[test]
fn every_reason_code_has_plain_words_and_an_action() {
    let table = [
        ("invalid_job", Next::Report),
        ("unsupported_protocol", Next::UpdateExporter),
        ("folder_busy", Next::TryAgain),
        ("output_unusable", Next::TryAgain),
        ("not_found", Next::ChooseAgain),
        ("not_signed_in", Next::TryAgain),
        ("failing", Next::TryAgain),
        ("disk_full", Next::TryAgain),
        ("output_missing", Next::TryAgain),
        ("output_unwritable", Next::TryAgain),
        ("memory", Next::TryAgain),
        ("account_changed", Next::TryAgain),
        ("interrupted", Next::TryAgain),
        ("cancelled", Next::Nothing),
        ("error", Next::TryAgain),
        ("from_a_future_release", Next::TryAgain),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for (reason, next) in table {
        let (text, got) = job::reason_info(reason, "OSError");
        assert_eq!(got, next, "{reason}");
        assert!(text.len() > 30 && !text.contains('_'), "{reason}: {text}");
        if reason == "error" || reason == "from_a_future_release" {
            assert!(text.contains("unexpected error (OSError)"), "{text}");
        } else {
            assert!(seen.insert(text), "{reason} repeats another message");
        }
    }
}

#[test]
fn a_queued_job_is_cancelled_without_running() {
    let (dir, addin) = setup();
    let (id, request) = export_job(dir.path());
    job::submit(&addin.agent(), Kind::Export, &request).unwrap();
    assert!(addin.claim(&id));
    let (sender, receiver) = channel();
    let stop = Arc::new(Default::default());
    job::follow(
        addin.jobs_dir(),
        id.clone(),
        Arc::clone(&stop),
        FAST,
        sender,
        || {},
    );
    let Update::Status(status) = receiver.recv_timeout(Duration::from_secs(5)).unwrap().1 else {
        panic!()
    };
    assert_eq!(status.state, "queued");
    stop.store(true, Ordering::SeqCst);
    let started = Instant::now();
    while !addin.cancel_requested(&id) {
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(FAST);
    }
    // The add-in cancels a queued job without running it.
    addin.status(
        &id,
        "cancelled",
        Some("cancelled"),
        "Cancelled.",
        (0, None, None),
        0,
        0,
        &dir.path().join("out"),
    );
    let Update::Final(status) = receiver.recv_timeout(Duration::from_secs(5)).unwrap().1 else {
        panic!()
    };
    assert_eq!(status.state, "cancelled");
    assert!(
        !addin.cancel_requested(&id),
        "the .cancel file is cleaned up"
    );
}

fn panel_for(dir: &Path) -> (Panel, eframe::egui::Context) {
    let mut panel = Panel::new();
    panel.preview(Env {
        cad_dir: Some(dir.join(".cad-export")),
        work_dir: dir.join("Dutchman Fusion exports"),
        poll: FAST,
    });
    panel.open = true;
    (panel, eframe::egui::Context::default())
}

fn wait_for(
    panel: &mut Panel,
    ctx: &eframe::egui::Context,
    what: &str,
    mut ready: impl FnMut(&Panel) -> bool,
) -> Option<std::path::PathBuf> {
    let started = Instant::now();
    let mut folder = None;
    loop {
        folder = folder.or(panel.poll(ctx));
        if ready(panel) {
            return folder;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "timed out waiting for {what}"
        );
        std::thread::sleep(FAST);
    }
}

fn pick_all(panel: &mut Panel) {
    let ids = panel.inventory.as_ref().unwrap().matching_ids("");
    panel.picked.extend(ids);
}

#[test]
fn the_panel_lists_designs_exports_the_picked_ones_and_hands_the_folder_on() {
    let (dir, addin) = setup();
    let service = Service::spawn(addin.clone(), sample_inventory(false), Ending::done(), FAST);
    let (mut panel, ctx) = panel_for(dir.path());
    wait_for(&mut panel, &ctx, "the list", |p| {
        p.stage() == Stage::Picking
    });
    assert_eq!(panel.inventory.as_ref().unwrap().count(), 4);
    pick_all(&mut panel);
    panel.export_picked();
    assert_eq!(panel.stage(), Stage::Exporting);
    let folder = wait_for(&mut panel, &ctx, "the export", |p| {
        p.running.is_none() && p.stage() != Stage::Exporting
    });
    let folder = folder.expect("a clean export is handed to the converter");
    assert_eq!(
        folder,
        job::export_folder(
            &dir.path().join("Dutchman Fusion exports/exports"),
            &panel.export_of.as_ref().unwrap().0
        )
    );
    assert!(folder.is_dir());
    assert_eq!(service.exports_seen.load(Ordering::SeqCst), 1);
    assert!(
        std::fs::read_dir(addin.jobs_dir())
            .unwrap()
            .next()
            .is_none(),
        "no job files left"
    );
    // The same picks are the same folder: asking again resumes.
    let again = job::export_folder(
        &dir.path().join("Dutchman Fusion exports/exports"),
        &panel.export_of.as_ref().unwrap().0,
    );
    assert_eq!(folder, again);
}

#[test]
fn done_with_failed_designs_offers_try_again_with_the_same_job_folder() {
    let (dir, addin) = setup();
    let mut ending = Ending::done();
    ending.failed = 2;
    let _service = Service::spawn(addin.clone(), sample_inventory(false), ending, FAST);
    let (mut panel, ctx) = panel_for(dir.path());
    wait_for(&mut panel, &ctx, "the list", |p| {
        p.stage() == Stage::Picking
    });
    pick_all(&mut panel);
    panel.export_picked();
    let folder = wait_for(&mut panel, &ctx, "the end", |p| {
        p.stage() == Stage::Finished
    });
    assert!(
        folder.is_none(),
        "failed designs are not converted without a choice"
    );
    let ended = panel.ended.as_ref().unwrap();
    assert_eq!(
        (ended.status.state.as_str(), ended.status.failed),
        ("done", 2)
    );
    let first = panel.export_of.clone().unwrap().1;
    panel.start_export();
    assert_eq!(panel.stage(), Stage::Exporting);
    assert_eq!(panel.export_of.clone().unwrap().1, first);
    wait_for(&mut panel, &ctx, "the second end", |p| {
        p.stage() == Stage::Finished
    });
    // Convert what exported hands the folder on.
    assert_eq!(panel.convert_what_exported(), Some(first));
}

#[test]
fn every_failure_reason_ends_in_its_plain_message() {
    for reason in [
        "folder_busy",
        "not_found",
        "not_signed_in",
        "memory",
        "interrupted",
        "disk_full",
        "failing",
        "invalid_job",
        "unsupported_protocol",
    ] {
        let (dir, addin) = setup();
        let _service = Service::spawn(
            addin.clone(),
            sample_inventory(false),
            Ending::failed_with(reason),
            FAST,
        );
        let (mut panel, ctx) = panel_for(dir.path());
        wait_for(&mut panel, &ctx, "the list", |p| {
            p.stage() == Stage::Picking
        });
        pick_all(&mut panel);
        panel.export_picked();
        wait_for(&mut panel, &ctx, reason, |p| p.stage() == Stage::Finished);
        let status = &panel.ended.as_ref().unwrap().status;
        assert_eq!(
            (status.state.as_str(), status.reason.as_deref()),
            ("failed", Some(reason))
        );
    }
}

#[test]
fn stop_while_running_cancels_and_keeps_what_was_exported() {
    let (dir, addin) = setup();
    let service = Service::spawn(
        addin.clone(),
        sample_inventory(false),
        Ending::done(),
        Duration::from_millis(30),
    );
    service.hold_at.store(1, Ordering::SeqCst);
    service.hold.store(true, Ordering::SeqCst);
    let (mut panel, ctx) = panel_for(dir.path());
    wait_for(&mut panel, &ctx, "the list", |p| {
        p.stage() == Stage::Picking
    });
    pick_all(&mut panel);
    panel.export_picked();
    wait_for(&mut panel, &ctx, "progress", |p| {
        p.running
            .as_ref()
            .and_then(|r| r.status.as_ref())
            .is_some_and(|s| s.state == "running")
    });
    panel.stop();
    assert!(panel.running.as_ref().unwrap().stopping);
    wait_for(&mut panel, &ctx, "cancelled", |p| {
        p.stage() == Stage::Finished
    });
    assert_eq!(panel.ended.as_ref().unwrap().status.state, "cancelled");
    assert!(std::fs::read_dir(addin.jobs_dir())
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn the_panel_notices_an_exporter_that_appears_later() {
    let (dir, addin) = setup();
    std::fs::remove_file(addin.cad.join("agents/fusion-bulk-export.json")).unwrap();
    let (mut panel, ctx) = panel_for(dir.path());
    wait_for(&mut panel, &ctx, "a first look", |p| {
        p.last_agent_check.is_some()
    });
    assert_eq!(panel.stage(), Stage::NoAgent);
    addin.announce(FEATURES, None);
    let _service = Service::spawn(addin.clone(), sample_inventory(false), Ending::done(), FAST);
    wait_for(&mut panel, &ctx, "the list", |p| {
        p.stage() == Stage::Picking
    });
    assert!(panel.error.is_none());
}

#[test]
fn the_list_of_designs_nests_by_hub_project_and_folder_and_searches() {
    let inventory = Inventory::from_value(&sample_inventory(true)).unwrap();
    assert_eq!(inventory.count(), 4, "the PDF is not a design");
    let hub = &inventory.hubs[0];
    assert_eq!(hub.name, "Konstruktionsabteilung Süd");
    assert_eq!(hub.projects.len(), 2);
    let gearbox = hub
        .projects
        .iter()
        .find(|p| p.name == "Getriebegehäuse Baureihe 7")
        .unwrap();
    assert_eq!(gearbox.root.designs.len(), 1);
    assert_eq!(
        gearbox.root.folders[0].name,
        "Gehäuseteile für Straßenfahrzeuge"
    );
    assert_eq!(gearbox.root.folders[0].designs.len(), 2);
    assert_eq!(inventory.matching_ids("").len(), 4);
    assert_eq!(inventory.matching_ids("lagerdeckel").len(), 1);
    assert_eq!(
        inventory.matching_ids("STRAßENFAHRZEUGE").len(),
        2,
        "a folder's name matches its designs"
    );
    assert_eq!(
        inventory.matching_ids("getriebegehäuse").len(),
        3,
        "a project's name matches its designs"
    );
    assert_eq!(inventory.matching_ids("konstruktionsabteilung").len(), 4);
    assert!(inventory.matching_ids("nothing like this").is_empty());
}

#[test]
fn an_inventory_of_an_unknown_schema_version_is_refused_in_plain_words() {
    let mut newer = sample_inventory(false);
    newer["schema_id"] = json!("cad-export-inventory/2");
    let why = Inventory::from_value(&newer).unwrap_err();
    assert!(
        why.contains("newer form") && why.contains("Update Dutchman"),
        "{why}"
    );
    newer["schema_id"] = json!("something-else/1");
    assert!(Inventory::from_value(&newer).is_err());
    // A file without the id is an older one with the same layout.
    let mut older = sample_inventory(false);
    older.as_object_mut().unwrap().remove("schema_id");
    assert_eq!(Inventory::from_value(&older).unwrap().count(), 4);
    // A stopped listing is flagged.
    let mut stopped = sample_inventory(false);
    stopped["stopped"] = json!(true);
    assert!(Inventory::from_value(&stopped).unwrap().stopped);
}

#[test]
fn the_inventory_must_have_hubs() {
    assert!(Inventory::from_value(&json!({"nope": 1})).is_err());
    assert_eq!(
        Inventory::from_value(&json!({"hubs": {}})).unwrap().count(),
        0
    );
}

#[test]
fn the_export_folder_depends_only_on_the_set_of_designs() {
    let base = Path::new("/base");
    let a = job::export_folder(base, &["x".into(), "y".into()]);
    assert_eq!(
        a,
        job::export_folder(base, &["y".into(), "x".into(), "x".into()])
    );
    assert_ne!(a, job::export_folder(base, &["x".into()]));
}

#[test]
fn ids_are_valid_and_distinct() {
    let ids: std::collections::BTreeSet<String> = (0..50).map(|_| job::new_id()).collect();
    assert_eq!(ids.len(), 50);
    assert!(ids.iter().all(|id| job::valid_id(id)));
}

#[test]
fn the_time_is_iso_8601_with_an_offset() {
    let now = job::iso_now();
    assert_eq!(now.len(), 25, "{now}");
    assert!(now.starts_with("20") && now.ends_with("+00:00"), "{now}");
}
