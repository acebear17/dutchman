//! Export from Fusion: the person picks designs, Dutchman asks the Fusion exporter running
//! inside their Fusion to export them (job protocol 1), follows its progress, and converts
//! the export folder like any other input.
//!
//! This part owns the job side: the inventory job and the design picker, the export job and
//! its progress, Stop, cleanup and the plain-words errors. Finding and installing the
//! exporter belong to other pieces; this one takes the exporter as an [`agent_min::Agent`].
pub mod agent_min;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod fake;
pub mod inventory;
pub mod job;
mod view;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_min::Agent;
use inventory::Inventory;
use job::{Kind, Next, Status, Update};

/// Where things live; replaced in tests and screenshots.
#[derive(Clone, Debug)]
pub struct Env {
    /// `~/.cad-export`; `None` when the person's home cannot be found.
    pub cad_dir: Option<PathBuf>,
    /// Dutchman's own folders for inventories and exports.
    pub work_dir: PathBuf,
    /// How often a job's status file is read, and the agent file looked for.
    pub poll: Duration,
}

impl Env {
    pub fn system() -> Self {
        let home = if cfg!(windows) {
            std::env::var_os("USERPROFILE")
        } else {
            std::env::var_os("HOME")
        }
        .map(PathBuf::from)
        .unwrap_or_default();
        Self {
            cad_dir: agent_min::cad_export_dir(),
            work_dir: home.join("Dutchman Fusion exports"),
            poll: Duration::from_millis(500),
        }
    }
}

/// Fusion only runs on Windows and macOS, so only they are offered the entry.
pub const fn supported_here() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// No exporter has announced itself.
    NoAgent,
    /// The inventory job is running.
    Listing,
    Picking,
    Exporting,
    /// The export ended without a clean `done`, or with failed designs.
    Finished,
}

struct Running {
    id: String,
    kind: Kind,
    stop: Arc<AtomicBool>,
    stopping: bool,
    status: Option<Status>,
}

/// How the last export ended, for the Finished screen.
struct Ended {
    status: Status,
    folder: PathBuf,
}

pub struct Panel {
    pub(crate) open: bool,
    offered: bool,
    env: Env,
    stage: Stage,
    agent: Option<Agent>,
    last_agent_check: Option<Instant>,
    inventory: Option<Inventory>,
    picked: BTreeSet<String>,
    query: String,
    running: Option<Running>,
    /// What an export of the picked designs is called, kept for "Try again".
    export_of: Option<(Vec<String>, PathBuf)>,
    ended: Option<Ended>,
    error: Option<(String, Next)>,
    repaint: Option<eframe::egui::Context>,
    sender: Sender<(String, Update)>,
    receiver: Receiver<(String, Update)>,
}

impl Panel {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = channel();
        Self {
            open: false,
            offered: supported_here(),
            env: Env::system(),
            stage: Stage::NoAgent,
            agent: None,
            last_agent_check: None,
            inventory: None,
            picked: BTreeSet::new(),
            query: String::new(),
            running: None,
            export_of: None,
            ended: None,
            error: None,
            repaint: None,
            sender,
            receiver,
        }
    }

    /// Whether the main screen offers "Export from Fusion…".
    pub(crate) fn offered(&self) -> bool {
        self.offered
    }

    /// Points the panel at another home and shows the entry on any system (tests and
    /// screenshots only).
    pub(crate) fn preview(&mut self, env: Env) {
        self.env = env;
        self.offered = true;
        self.stage = Stage::NoAgent;
        self.agent = None;
        self.last_agent_check = None;
        self.inventory = None;
        self.running = None;
    }

    pub(crate) fn stage(&self) -> Stage {
        self.stage
    }

    pub(crate) fn toggle(&mut self) {
        self.open = !self.open;
        self.last_agent_check = None;
    }

    fn find_agent(&mut self) {
        let Some(cad) = &self.env.cad_dir else {
            return;
        };
        let found = agent_min::read_minimal(&agent_min::agent_file(cad));
        if found.is_some() && self.agent.is_none() {
            self.error = None;
        }
        self.agent = found;
        if self.agent.is_some() && self.stage == Stage::NoAgent {
            self.stage = Stage::Listing;
            self.list_designs();
        }
    }

    /// Starts the inventory job: the list of the person's hubs, projects and designs.
    pub(crate) fn list_designs(&mut self) {
        self.error = None;
        self.stage = Stage::Listing;
        let Some(agent) = self.agent.clone() else {
            self.stage = Stage::NoAgent;
            return;
        };
        let folder = self.env.work_dir.join("inventory");
        let started = std::fs::create_dir_all(&folder)
            .map_err(|why| format!("Cannot use Dutchman's folder for Fusion exports: {why}"))
            .and_then(|()| {
                let _ = std::fs::remove_file(folder.join("inventory.json"));
                let id = job::new_id();
                let request = job::build_job(&id, Kind::Inventory, &folder, &[]);
                job::submit(&agent, Kind::Inventory, &request)
            });
        match started {
            Ok(id) => self.follow(agent, id, Kind::Inventory),
            Err(why) => {
                self.error = Some((why, Next::TryAgain));
                self.stage = if self.inventory.is_some() {
                    Stage::Picking
                } else {
                    Stage::NoAgent
                };
            }
        }
    }

    /// Exports the picked designs, with everything they insert.
    pub(crate) fn export_picked(&mut self) {
        let designs = inventory::sorted(&self.picked);
        if designs.is_empty() {
            return;
        }
        let folder = job::export_folder(&self.env.work_dir.join("exports"), &designs);
        self.export_of = Some((designs, folder));
        self.start_export();
    }

    /// Runs the same export again; the exporter carries on where it stopped.
    pub(crate) fn start_export(&mut self) {
        self.error = None;
        self.ended = None;
        let (Some(agent), Some((designs, folder))) = (self.agent.clone(), self.export_of.clone())
        else {
            return;
        };
        let started = std::fs::create_dir_all(&folder)
            .map_err(|why| format!("Cannot use Dutchman's folder for Fusion exports: {why}"))
            .and_then(|()| {
                let id = job::new_id();
                let request = job::build_job(&id, Kind::Export, &folder, &designs);
                job::submit(&agent, Kind::Export, &request)
            });
        match started {
            Ok(id) => {
                self.stage = Stage::Exporting;
                self.follow(agent, id, Kind::Export);
            }
            Err(why) => self.error = Some((why, Next::TryAgain)),
        }
    }

    fn follow(&mut self, agent: Agent, id: String, kind: Kind) {
        let stop = Arc::new(AtomicBool::new(false));
        self.running = Some(Running {
            id: id.clone(),
            kind,
            stop: stop.clone(),
            stopping: false,
            status: None,
        });
        let ctx = self.repaint.clone();
        job::follow(
            agent.jobs_dir,
            id,
            stop,
            self.env.poll,
            self.sender.clone(),
            move || {
                if let Some(ctx) = &ctx {
                    ctx.request_repaint();
                }
            },
        );
    }

    /// Asks the exporter to stop: the designs finished so far are kept.
    pub(crate) fn stop(&mut self) {
        if let Some(running) = &mut self.running {
            running.stop.store(true, Ordering::SeqCst);
            running.stopping = true;
        }
    }

    /// Reads what the exporter wrote since the last frame. Returns the export folder when
    /// it is ready to convert.
    pub(crate) fn poll(&mut self, ctx: &eframe::egui::Context) -> Option<PathBuf> {
        self.repaint = Some(ctx.clone());
        if self.open
            && self.stage == Stage::NoAgent
            && self
                .last_agent_check
                .is_none_or(|at| at.elapsed() >= self.env.poll)
        {
            self.last_agent_check = Some(Instant::now());
            self.find_agent();
            ctx.request_repaint_after(self.env.poll);
        }
        let mut convert = None;
        while let Ok((id, update)) = self.receiver.try_recv() {
            let Some(running) = self.running.as_mut().filter(|r| r.id == id) else {
                continue;
            };
            match update {
                Update::Status(status) => running.status = Some(status),
                Update::Lost(why) => {
                    let kind = running.kind;
                    self.running = None;
                    self.error = Some((why, Next::TryAgain));
                    self.stage = if kind == Kind::Export || self.inventory.is_some() {
                        Stage::Picking
                    } else {
                        Stage::NoAgent
                    };
                }
                Update::Final(status) => {
                    let kind = running.kind;
                    self.running = None;
                    convert = convert.or(self.finished(kind, status));
                }
            }
        }
        convert
    }

    fn finished(&mut self, kind: Kind, status: Status) -> Option<PathBuf> {
        let reason_text = |status: &Status| {
            job::reason_info(status.reason.as_deref().unwrap_or("error"), &status.message)
        };
        match kind {
            Kind::Inventory => {
                let listed = if status.state == "done" {
                    status
                        .inventory
                        .as_ref()
                        .ok_or_else(|| {
                            "The Fusion exporter did not say where the list is.".to_owned()
                        })
                        .and_then(|path| Inventory::read(path))
                } else {
                    Err(reason_text(&status).0)
                };
                match listed {
                    Ok(inventory) => {
                        let known: BTreeSet<String> =
                            inventory.matching_ids("").into_iter().collect();
                        self.picked.retain(|id| known.contains(id));
                        self.inventory = Some(inventory);
                        self.stage = Stage::Picking;
                    }
                    Err(why) => {
                        self.error = Some((why, Next::TryAgain));
                        self.stage = if self.inventory.is_some() {
                            Stage::Picking
                        } else {
                            Stage::NoAgent
                        };
                    }
                }
                None
            }
            Kind::Export => {
                let folder = self
                    .export_of
                    .as_ref()
                    .map(|(_, folder)| folder.clone())
                    .unwrap_or_default();
                if status.state == "done" && status.failed == 0 {
                    self.stage = Stage::Picking;
                    self.open = false;
                    return Some(folder);
                }
                self.stage = Stage::Finished;
                self.ended = Some(Ended { status, folder });
                None
            }
        }
    }

    /// Takes what exported so far to the converter.
    pub(crate) fn convert_what_exported(&mut self) -> Option<PathBuf> {
        let ended = self.ended.take()?;
        self.stage = Stage::Picking;
        self.open = false;
        Some(ended.folder)
    }

    /// Back to the list of designs.
    pub(crate) fn back_to_picking(&mut self) {
        self.ended = None;
        self.error = None;
        self.stage = if self.inventory.is_some() {
            Stage::Picking
        } else {
            Stage::NoAgent
        };
    }
}

#[cfg(test)]
mod tests;
