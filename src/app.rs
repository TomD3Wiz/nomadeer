use crate::auto_intel::{self, AutoBriefings, IntelQueue, IntelRequest, LocationFix};
use crate::chain_archive::{self, ChainSnapshot, ChainSnapshotMeta};
use crate::config::theme;
use crate::esi::{self, AccountAuthorization, LocationSync, ONLINE_SCOPE, OnlineSync, SystemIntel};
use crate::forms::*;
use crate::live_intel;
use crate::local_intel::{self, BriefingEvent, BriefingWorker, CoreStatus, DownloadProgress};
use crate::model::*;
use crate::paths;
use crate::portrait;
use crate::recon::{self, MAX_RECON_CHARACTERS, OnlinePolls};
use crate::store::{KILLMAIL_RETENTION_SECS, Store, TRAFFIC_RETENTION_SECS};
use crate::util::{clean, now, open_url};
use crate::widgets::{pill, record_frame};
use crate::zkill::{self, ZkillKill, ZkillReport};
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

mod chain_art;
mod chain_map;
mod chain_view;
mod layout;
mod live;
mod scan;
mod umbra;
mod updates;
mod wormholes;

const AUTO_SYNC_INTERVAL: Duration = Duration::from_secs(5);
const WARNING_TEXT: Color32 = Color32::from_rgb(255, 194, 87);
/// The tabs and Umbra scroll within whatever height is left under the chain map, but
/// never less than this; below it the bottom is cut off instead.
const MIN_DASHBOARD_PANES_HEIGHT: f32 = 140.0;
const RECON_HINT: &str = "Nomadeer gathers intel and Umbra briefs wherever recon pilots arrive.";

/// Traffic recent enough to show as describing a system now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecentTraffic {
    /// End of the oldest hour shown.
    data_time: u64,
    jumps: Option<u32>,
    kills: Option<KillCounts>,
}

enum EsiTaskResult {
    Authorized(AccountAuthorization),
    Location(LocationSync),
    Online(OnlineSync),
}

enum ZkillTaskResult {
    Report {
        report: ZkillReport,
        traffic: SystemTrafficHistory,
        /// The system's cached week of kills, for checking pasted local against.
        killmails: Option<Vec<CachedKillmail>>,
        save_error: Option<String>,
    },
}

struct TrafficTaskResult {
    feed: TrafficFeed,
    /// When ESI will publish the next hour of this feed.
    result: Result<u64, String>,
}

struct TrafficFeedState {
    feed: TrafficFeed,
    next_poll: Instant,
    failures: u32,
    busy: bool,
}

enum LocalIntelTaskResult {
    Installed,
    Imported,
    Checked,
}

enum LocalIntelEvent {
    Progress(DownloadProgress),
    Finished(Result<LocalIntelTaskResult, String>),
    Briefing(BriefingEvent),
}

/// What Umbra is busy with; she does one thing at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
enum UmbraTask {
    Installing,
    Importing,
    Checking,
    /// `auto` briefings are ones she writes on her own once fresh intel is in; they
    /// stay in her panels and out of the status bar.
    Briefing {
        system_id: String,
        waking: bool,
        auto: bool,
    },
}

impl UmbraTask {
    fn label(&self) -> &'static str {
        match self {
            Self::Installing => "Installing Umbra...",
            Self::Importing => "Installing Umbra from your file...",
            Self::Checking => "Umbra is checking her core...",
            Self::Briefing { waking: true, .. } => {
                "Umbra is waking up for her first briefing this session..."
            }
            Self::Briefing { waking: false, .. } => "Umbra is writing a briefing...",
        }
    }

    fn is_auto(&self) -> bool {
        matches!(self, Self::Briefing { auto: true, .. })
    }

    fn is_waking(&self) -> bool {
        matches!(self, Self::Briefing { waking: true, .. })
    }

    /// The system she is briefing, if any.
    fn briefing_system(&self) -> Option<&str> {
        match self {
            Self::Briefing { system_id, .. } => Some(system_id),
            _ => None,
        }
    }

    /// The system she is briefing, when a panel for a different system is asking.
    fn briefing_elsewhere(&self, panel_system_id: Option<&str>) -> Option<&str> {
        match self {
            Self::Briefing { system_id, .. } => panel_system_id
                .filter(|panel| *panel != system_id)
                .map(|_| system_id.as_str()),
            _ => None,
        }
    }
}

/// Umbra's last word, kept in her panels until her next task.
struct UmbraNotice {
    /// The system the notice is about, or None for install and repair news.
    system_id: Option<String>,
    message: String,
    is_error: bool,
}

struct PortraitTaskResult {
    account_id: String,
    image: egui::ColorImage,
}

#[derive(Debug, Clone)]
enum ChainMapTarget {
    Map,
    System(String),
    Connection(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DashboardTab {
    Movement,
    Connections,
    Systems,
    Intel,
    Chain,
}

pub(crate) struct NomadeerApp {
    data: AppData,
    store: Store,
    status: String,
    selected_system_id: String,
    active_tab: DashboardTab,
    esi_settings_form: EsiSettingsForm,
    show_settings: bool,
    show_local_intel_settings: bool,
    id_counter: u64,
    esi_tx: Sender<Result<EsiTaskResult, String>>,
    esi_rx: Receiver<Result<EsiTaskResult, String>>,
    zkill_tx: Sender<Result<ZkillTaskResult, String>>,
    zkill_rx: Receiver<Result<ZkillTaskResult, String>>,
    portrait_tx: Sender<Result<PortraitTaskResult, String>>,
    portrait_rx: Receiver<Result<PortraitTaskResult, String>>,
    local_intel_tx: Sender<LocalIntelEvent>,
    local_intel_rx: Receiver<LocalIntelEvent>,
    traffic_tx: Sender<TrafficTaskResult>,
    traffic_rx: Receiver<TrafficTaskResult>,
    traffic_feeds: Vec<TrafficFeedState>,
    esi_busy: bool,
    /// The ESI task in flight is a background location sync, which only speaks up when a
    /// character moves, so it does not bury other news every few seconds.
    background_esi_task: bool,
    /// Killboard refreshes waiting and in flight, one at a time.
    intel_queue: IntelQueue,
    /// Why the last refresh of a system failed, by solar system ID, until one succeeds.
    intel_errors: HashMap<u64, String>,
    /// Characters located since launch, so their first fix counts as an arrival.
    located_this_session: HashSet<String>,
    /// Whether each character is logged in, for those whose sign-in can tell.
    online_polls: OnlinePolls,
    /// The character whose online status the ESI task in flight is checking.
    online_poll_in_flight: Option<String>,
    auto_briefings: AutoBriefings,
    zkill_reports: HashMap<u64, ZkillReport>,
    /// The traffic each killboard refresh read, by solar system ID, for showing and
    /// for re-assessing the system when live intel is pasted.
    refreshed_traffic: HashMap<u64, SystemTrafficHistory>,
    /// Each system's cached week of kills, by solar system ID, for checking pasted
    /// local against.
    system_killmails: HashMap<u64, live_intel::SystemKills>,
    /// The latest pasted local and d-scan of each system, by map system ID.
    live_intel: HashMap<String, live_intel::SystemLiveIntel>,
    /// Pastes being read: the system and what was pasted.
    live_reading: Vec<(String, live::LiveKind)>,
    live_tx: Sender<live::LiveTaskResult>,
    live_rx: Receiver<live::LiveTaskResult>,
    /// When the live intel in each system's assessment runs out, so it is
    /// re-assessed without it.
    live_expiry: HashMap<String, u64>,
    /// The selected system's local paste, checked against its kills.
    local_check: Option<live::LocalCheck>,
    selected_zkill_kill: Option<ZkillKill>,
    system_assessments: HashMap<String, SystemAssessment>,
    umbra_task: Option<UmbraTask>,
    umbra_worker: Option<BriefingWorker>,
    umbra_notice: Option<UmbraNotice>,
    /// Her core failed its checksum or could not be loaded; reinstalling repairs it.
    umbra_needs_repair: bool,
    /// Checks run once per process on their own; after that the player asks for them.
    umbra_check_attempted: bool,
    /// Her briefings, oldest first, as shown in her panel.
    umbra_feed: Vec<UmbraBriefing>,
    /// Her portrait once loaded; None inside when it could not be decoded.
    umbra_avatar: Option<Option<egui::TextureHandle>>,
    /// The player pressed Clear history and has not yet confirmed.
    umbra_confirm_clear: bool,
    local_intel_progress: Option<DownloadProgress>,
    local_model_import_path: String,
    portrait_textures: HashMap<String, egui::TextureHandle>,
    portrait_requested: HashSet<String>,
    chain_snapshot_name: String,
    chain_snapshots: Vec<ChainSnapshotMeta>,
    chain_context_target: Option<ChainMapTarget>,
    /// The chain map's view this session; None until it is next fitted to the chain.
    chain_camera: Option<chain_view::Camera>,
    chain_drag: Option<chain_map::ChainDrag>,
    /// The player pressed Reset layout and has not yet confirmed.
    chain_confirm_reset_layout: bool,
    /// The key to the chain map's artwork is open.
    chain_show_legend: bool,
    /// Where the legend and the corner controls covered the map last frame, so a
    /// connection is never dropped on a system hidden under them.
    chain_overlays: Vec<egui::Rect>,
    /// What the last pasted probe scan did, shown beside the system's signatures.
    scan_notice: Option<scan::ScanNotice>,
    /// When the Paste scan button asked for the clipboard, until it arrives.
    paste_requested: Option<Instant>,
    next_auto_sync: Instant,
    auto_sync_index: usize,
    updates: updates::UpdateUi,
}

impl NomadeerApp {
    pub(crate) fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let store = Store::new(paths::database_path()).with_legacy_dirs(paths::legacy_data_dirs());
        let (esi_tx, esi_rx) = mpsc::channel();
        let (zkill_tx, zkill_rx) = mpsc::channel();
        let (portrait_tx, portrait_rx) = mpsc::channel();
        let (local_intel_tx, local_intel_rx) = mpsc::channel();
        let (traffic_tx, traffic_rx) = mpsc::channel();
        let (live_tx, live_rx) = mpsc::channel();
        let mut app = Self {
            data: AppData::default(),
            store,
            status: "Ready".to_owned(),
            selected_system_id: String::new(),
            active_tab: DashboardTab::Movement,
            esi_settings_form: EsiSettingsForm::default(),
            show_settings: false,
            show_local_intel_settings: false,
            id_counter: 0,
            esi_tx,
            esi_rx,
            zkill_tx,
            zkill_rx,
            portrait_tx,
            portrait_rx,
            local_intel_tx,
            local_intel_rx,
            traffic_tx,
            traffic_rx,
            traffic_feeds: TrafficFeed::ALL
                .into_iter()
                .map(|feed| TrafficFeedState {
                    feed,
                    next_poll: Instant::now(),
                    failures: 0,
                    busy: false,
                })
                .collect(),
            esi_busy: false,
            background_esi_task: false,
            intel_queue: IntelQueue::default(),
            intel_errors: HashMap::new(),
            located_this_session: HashSet::new(),
            online_polls: OnlinePolls::default(),
            online_poll_in_flight: None,
            auto_briefings: AutoBriefings::default(),
            zkill_reports: HashMap::new(),
            refreshed_traffic: HashMap::new(),
            system_killmails: HashMap::new(),
            live_intel: HashMap::new(),
            live_reading: Vec::new(),
            local_check: None,
            live_tx,
            live_rx,
            live_expiry: HashMap::new(),
            selected_zkill_kill: None,
            system_assessments: HashMap::new(),
            umbra_task: None,
            umbra_worker: None,
            umbra_notice: None,
            umbra_needs_repair: false,
            umbra_check_attempted: false,
            umbra_feed: Vec::new(),
            umbra_avatar: None,
            umbra_confirm_clear: false,
            local_intel_progress: None,
            local_model_import_path: String::new(),
            portrait_textures: HashMap::new(),
            portrait_requested: HashSet::new(),
            chain_snapshot_name: String::new(),
            chain_snapshots: Vec::new(),
            chain_context_target: None,
            chain_camera: None,
            chain_drag: None,
            chain_confirm_reset_layout: false,
            chain_show_legend: false,
            chain_overlays: Vec::new(),
            scan_notice: None,
            paste_requested: None,
            next_auto_sync: Instant::now() + Duration::from_secs(5),
            auto_sync_index: 0,
            updates: updates::UpdateUi::new(),
        };
        app.load();
        app
    }

    fn load(&mut self) {
        match self.store.load() {
            Ok(data) => {
                self.data = data;
                if self.data.esi_settings.client_id.is_empty() {
                    self.data.esi_settings.client_id = DEFAULT_ESI_CLIENT_ID.to_owned();
                }
                self.system_assessments = self
                    .data
                    .system_assessments
                    .iter()
                    .cloned()
                    .map(|assessment| (assessment.system_id.clone(), assessment))
                    .collect();
                self.load_live_intel();
                self.sync_esi_settings_form();
                self.reconcile_selected_system();
                self.refresh_chain_snapshots();
                self.default_chain_snapshot_name();
                self.status = "Loaded".to_owned();
                // After "Loaded", so a feed that cannot be read is not hidden behind it.
                self.load_umbra_feed();
                self.load_update_state();
                if recon::apply_recon_default(&mut self.data)
                    && let Err(error) = self.store.save(&self.data)
                {
                    self.status = format!("Save failed: {error}");
                }
            }
            Err(error) => {
                self.status = format!("Could not load database: {error}");
            }
        }
    }

    fn save(&mut self) {
        match self.store.save(&self.data) {
            Ok(()) => self.status = "Saved".to_owned(),
            Err(error) => self.status = format!("Save failed: {error}"),
        }
    }

    /// Saves without announcing it, for layout changes the player can already see.
    fn save_quietly(&mut self) {
        if let Err(error) = self.store.save(&self.data) {
            self.status = format!("Save failed: {error}");
        }
    }

    fn refresh_chain_snapshots(&mut self) {
        match chain_archive::list_snapshots(&paths::chain_archive_dir()) {
            Ok(snapshots) => self.chain_snapshots = snapshots,
            Err(error) => self.status = format!("Could not list saved chains: {error}"),
        }
    }

    fn default_chain_snapshot_name(&mut self) {
        if self.chain_snapshot_name.trim().is_empty() {
            self.chain_snapshot_name = format!("Chain {}", now());
        }
    }

    fn new_id(&mut self, prefix: &str) -> String {
        self.id_counter += 1;
        format!("{prefix}_{}_{}", now(), self.id_counter)
    }

    fn sync_esi_settings_form(&mut self) {
        self.esi_settings_form.client_id = self.data.esi_settings.client_id.clone();
        self.esi_settings_form.callback_port = self.data.esi_settings.callback_port.to_string();
        self.esi_settings_form.scopes = self.data.esi_settings.scopes.clone();
    }

    fn save_esi_settings_from_form(&mut self) -> bool {
        let Ok(callback_port) = self.esi_settings_form.callback_port.trim().parse::<u16>() else {
            self.status = "ESI callback port must be a number from 1 to 65535".to_owned();
            return false;
        };

        let client_id = clean(&self.esi_settings_form.client_id);
        self.data.esi_settings.client_id = if client_id.is_empty() {
            DEFAULT_ESI_CLIENT_ID.to_owned()
        } else {
            client_id
        };
        self.esi_settings_form.client_id = self.data.esi_settings.client_id.clone();
        self.data.esi_settings.callback_port = callback_port;
        self.data.esi_settings.scopes = clean(&self.esi_settings_form.scopes);
        self.save();
        true
    }

    fn system_name(&self, id: &str) -> String {
        self.data
            .systems
            .iter()
            .find(|system| system.id == id)
            .map(|system| system.name.clone())
            .unwrap_or_else(|| "Unknown".to_owned())
    }

    fn reconcile_selected_system(&mut self) {
        let selected_is_valid = self
            .data
            .systems
            .iter()
            .any(|system| system.id == self.selected_system_id);
        if selected_is_valid {
            return;
        }

        self.selected_system_id = self
            .data
            .accounts
            .iter()
            .find(|account| !account.last_system_id.is_empty())
            .map(|account| account.last_system_id.clone())
            .or_else(|| self.data.systems.first().map(|system| system.id.clone()))
            .unwrap_or_default();
    }

    fn selected_system(&self) -> Option<&SystemNode> {
        self.data
            .systems
            .iter()
            .find(|system| system.id == self.selected_system_id)
    }

    fn system_by_id(&self, id: &str) -> Option<&SystemNode> {
        self.data.systems.iter().find(|system| system.id == id)
    }

    /// The class pill: the w-space class, or HS/LS/NS for known space. W-space is told
    /// apart the same way the assessment does it, by system ID rather than by name.
    fn inferred_system_class(system: &SystemNode) -> String {
        if local_intel::is_wormhole_space(system) {
            return Self::wormhole_class(system).unwrap_or("WH").to_owned();
        }
        if system.solar_system_id == 0 && system.region_id == 0 && system.security_status == 0.0 {
            "Unknown"
        } else if system.security_status >= 0.45 {
            "HS"
        } else if system.security_status > 0.0 {
            "LS"
        } else {
            "NS"
        }
        .to_owned()
    }

    /// W-space regions and constellations are named by class letter, e.g. "B-R00008"
    /// for a C2 region; G is Thera, H the C13 shattered systems and K the Drifter
    /// systems, which each have their own class.
    fn wormhole_class(system: &SystemNode) -> Option<&'static str> {
        let entered = system.class.trim().to_ascii_uppercase();
        let entered = match entered.as_str() {
            "C1" => Some("C1"),
            "C2" => Some("C2"),
            "C3" => Some("C3"),
            "C4" => Some("C4"),
            "C5" => Some("C5"),
            "C6" => Some("C6"),
            "C13" => Some("C13"),
            "C14" => Some("C14"),
            "C15" => Some("C15"),
            "C16" => Some("C16"),
            "C17" => Some("C17"),
            "C18" => Some("C18"),
            "THERA" => Some("Thera"),
            _ => None,
        };
        let by_id = match system.solar_system_id {
            31_000_001 => Some("C14"),
            31_000_002 => Some("C15"),
            31_000_003 => Some("C16"),
            31_000_004 => Some("C17"),
            31_000_005 => Some("Thera"),
            31_000_006 => Some("C18"),
            _ => None,
        };
        entered.or(by_id).or_else(|| {
            [&system.region_name, &system.constellation_name]
                .into_iter()
                .find_map(|name| {
                    let mut parts = name.split('-');
                    let letter = parts.next()?;
                    parts.next()?.strip_prefix(['R', 'C'])?;
                    Some(match letter {
                        "A" => "C1",
                        "B" => "C2",
                        "C" => "C3",
                        "D" => "C4",
                        "E" => "C5",
                        "F" => "C6",
                        "G" => "Thera",
                        "H" => "C13",
                        "K" => "Drifter",
                        _ => return None,
                    })
                })
        })
    }

    /// A system's newest traffic that still describes it, from the map node or the
    /// last killboard refresh. W-space has no traffic feed, so it never has any.
    fn recent_traffic(
        system: &SystemNode,
        refreshed: Option<&SystemTrafficHistory>,
        current_time: u64,
    ) -> Option<RecentTraffic> {
        if local_intel::is_wormhole_space(system) {
            return None;
        }
        let is_fresh = |data_time: u64| {
            data_time != 0
                && current_time.saturating_sub(data_time) <= local_intel::TRAFFIC_FRESH_SECS
        };
        let node_time = system.intel_updated_at;
        let jumps = match refreshed.and_then(|traffic| traffic.latest_jumps) {
            Some(reading) if reading.data_time >= node_time => (reading.data_time, reading.value),
            _ => (node_time, system.jumps_1h),
        };
        let kills = match refreshed.and_then(|traffic| traffic.latest_kills) {
            Some(reading) if reading.data_time >= node_time => (reading.data_time, reading.value),
            _ => (
                node_time,
                KillCounts {
                    ship_kills: system.ship_kills_1h,
                    pod_kills: system.pod_kills_1h,
                    npc_kills: system.npc_kills_1h,
                },
            ),
        };
        let jumps = Some(jumps).filter(|(data_time, _)| is_fresh(*data_time));
        let kills = Some(kills).filter(|(data_time, _)| is_fresh(*data_time));
        let data_time = match (jumps, kills) {
            (Some((jumps_time, _)), Some((kills_time, _))) => jumps_time.min(kills_time),
            (Some((data_time, _)), None) | (None, Some((data_time, _))) => data_time,
            (None, None) => return None,
        };
        Some(RecentTraffic {
            data_time,
            jumps: jumps.map(|(_, jumps)| jumps),
            kills: kills.map(|(_, kills)| kills),
        })
    }

    fn recent_traffic_label(traffic: &RecentTraffic) -> String {
        let mut facts = Vec::new();
        if let Some(jumps) = traffic.jumps {
            facts.push(format!("{jumps} jumps"));
        }
        if let Some(kills) = traffic.kills {
            facts.push(format!(
                "{} ships · {} pods · {} NPCs",
                kills.ship_kills, kills.pod_kills, kills.npc_kills
            ));
        }
        format!(
            "Hour to {} EVE time: {}",
            local_intel::eve_clock(traffic.data_time),
            facts.join(" · ")
        )
    }

    fn system_intel_summary(&self, system: &SystemNode) -> String {
        let region = if system.region_name.is_empty() {
            "Unknown region"
        } else {
            &system.region_name
        };
        let traffic = Self::recent_traffic(
            system,
            self.refreshed_traffic.get(&system.solar_system_id),
            now(),
        );
        match traffic {
            Some(traffic) => format!(
                "{} · {} · {}",
                Self::inferred_system_class(system),
                region,
                Self::recent_traffic_label(&traffic)
            ),
            None => format!("{} · {}", Self::inferred_system_class(system), region),
        }
    }

    fn open_external_url(&mut self, url: &str) {
        match open_url(url) {
            Ok(()) => self.status = "Opened browser".to_owned(),
            Err(error) => self.status = error,
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Nomadeer");
                if ui.button("Settings").clicked() {
                    self.show_settings = true;
                }
                if ui.button("Umbra").clicked() {
                    self.show_local_intel_settings = true;
                }
                if ui
                    .add_enabled(!self.esi_busy, egui::Button::new("Add character"))
                    .clicked()
                {
                    self.start_add_character();
                }
                self.update_pill(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(&self.status);
                });
            });
        });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut close_requested = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.update_settings(ui);
                    ui.separator();
                    ui.heading("EVE sign-in");
                    close_requested = self.esi_settings_contents(ui);
                });
            });
        if close_requested {
            open = false;
        }
        self.show_settings = open;
    }

    fn esi_settings_contents(&mut self, ui: &mut egui::Ui) -> bool {
        let mut close_requested = false;
        ui.label("Nomadeer signs in with its own EVE app. To use your own instead, register it with this callback URL:");
        ui.monospace(format!(
            "http://localhost:{}/callback",
            self.esi_settings_form.callback_port
        ));
        ui.add_space(8.0);
        ui.label("Client ID");
        ui.add(
            egui::TextEdit::singleline(&mut self.esi_settings_form.client_id)
                .hint_text("Leave empty to use Nomadeer's EVE app"),
        );
        ui.label("Callback port");
        ui.add(
            egui::TextEdit::singleline(&mut self.esi_settings_form.callback_port).hint_text("8788"),
        );
        ui.label("Scopes");
        ui.add(
            egui::TextEdit::multiline(&mut self.esi_settings_form.scopes)
                .desired_rows(5)
                .hint_text("Space-separated ESI scopes"),
        );
        ui.horizontal(|ui| {
            if ui.button("Save and close").clicked() && self.save_esi_settings_from_form() {
                close_requested = true;
            }
            if ui.button("Reload database").clicked() {
                self.load();
            }
            if ui.button("Save database").clicked() {
                self.save();
            }
        });
        ui.separator();
        ui.label(format!("Database: {}", self.store.path().display()));
        close_requested
    }

    fn local_intel_settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_local_intel_settings;
        egui::Window::new("Umbra")
            .open(&mut open)
            .resizable(true)
            .default_width(560.0)
            .show(ctx, |ui| {
                ui.heading("Umbra");
                ui.label(local_intel::UMBRA_TAGLINE);
                egui::CollapsingHeader::new("About Umbra")
                    .default_open(false)
                    .show(ui, |ui| {
                        for paragraph in local_intel::UMBRA_LORE {
                            ui.label(*paragraph);
                        }
                    });
                ui.add_space(4.0);
                ui.label("Umbra turns Nomadeer's threat and activity evidence into short briefings. She never sets the scores herself.");
                ui.label(
                    egui::RichText::new(format!(
                        "Core: {} · {} · {:.0} MB download · runs entirely on this computer",
                        local_intel::MODEL_NAME,
                        local_intel::MODEL_LICENSE,
                        local_intel::MODEL_DOWNLOAD_SIZE as f64 / 1_000_000.0
                    ))
                    .color(theme::muted_text()),
                );
                ui.add_space(8.0);

                let core = local_intel::core_status();
                // Briefings she writes on her own never keep the player from her controls.
                let idle = self.umbra_free_for_player();
                ui.horizontal_wrapped(|ui| {
                    pill(ui, self.umbra_state_label(core));
                    let button = |ui: &mut egui::Ui, text: &str| {
                        ui.add_enabled(idle, egui::Button::new(text)).clicked()
                    };
                    match core {
                        CoreStatus::Missing => {
                            if button(ui, "Install Umbra") {
                                self.start_local_model_install();
                            }
                            if local_intel::has_legacy_cores()
                                && button(ui, "Clear old Umbra files")
                            {
                                self.remove_local_model();
                            }
                        }
                        _ if self.umbra_needs_repair => {
                            if button(ui, "Reinstall Umbra") {
                                self.start_local_model_install();
                            }
                            if button(ui, "Remove Umbra") {
                                self.remove_local_model();
                            }
                        }
                        CoreStatus::Unchecked => {
                            if button(ui, "Check Umbra") {
                                self.start_umbra_core_check();
                            }
                            if button(ui, "Remove Umbra") {
                                self.remove_local_model();
                            }
                        }
                        CoreStatus::Ready => {
                            if button(ui, "Remove Umbra") {
                                self.remove_local_model();
                            }
                        }
                    }
                });

                self.umbra_auto_brief_toggle(ui);

                if let Some(progress) = &self.local_intel_progress {
                    let fraction = if progress.total == 0 {
                        0.0
                    } else {
                        progress.done as f32 / progress.total as f32
                    };
                    ui.add(
                        egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                            .text(format!("{} · {:.0}%", progress.label, fraction * 100.0)),
                    );
                }
                self.umbra_feedback(ui, None);

                if core != CoreStatus::Ready {
                    ui.separator();
                    egui::CollapsingHeader::new("Already downloaded her core?")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.label(format!(
                                "Point Nomadeer at your copy of {} to skip the download. Umbra only accepts the official release.",
                                local_intel::MODEL_NAME
                            ));
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.local_model_import_path)
                                        .hint_text(local_intel::import_path_hint()),
                                );
                                if ui
                                    .add_enabled(idle, egui::Button::new("Install from file"))
                                    .clicked()
                                {
                                    self.start_local_model_import();
                                }
                            });
                        });
                }
            });
        self.show_local_intel_settings = open;
    }

    fn umbra_state_label(&self, core: CoreStatus) -> &'static str {
        match (core, &self.umbra_task) {
            (_, Some(UmbraTask::Checking)) => "Checking",
            (CoreStatus::Missing, _) => "Not installed",
            _ if self.umbra_needs_repair => "Needs repair",
            (CoreStatus::Unchecked, _) => "Not checked yet",
            (CoreStatus::Ready, _) => "Online",
        }
    }

    /// Shows what Umbra is doing and her last word. With a system, a briefing elsewhere
    /// is named, and notices about other systems are left out.
    fn umbra_feedback(&self, ui: &mut egui::Ui, system_id: Option<&str>) {
        let concerns = |about: Option<&str>| system_id.is_none() || about == system_id;
        // Any task keeps her from briefing anywhere, so every panel says what it is.
        if let Some(task) = &self.umbra_task {
            ui.horizontal(|ui| {
                ui.spinner();
                match task.briefing_elsewhere(system_id) {
                    Some(other) => ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label("Umbra is briefing ");
                        ui.label(
                            egui::RichText::new(self.system_name(other))
                                .color(theme::system_text()),
                        );
                        ui.label("...");
                    }),
                    None => ui.horizontal(|ui| {
                        ui.label(task.label());
                    }),
                };
            });
        }
        if let Some(notice) = &self.umbra_notice
            && (concerns(notice.system_id.as_deref())
                || (notice.system_id.is_none() && notice.is_error))
        {
            let color = if notice.is_error {
                WARNING_TEXT
            } else {
                theme::muted_text()
            };
            ui.label(egui::RichText::new(&notice.message).color(color));
        }
        if let Some(system_id) = system_id
            && self.auto_briefings.is_pending(system_id)
            && self
                .umbra_task
                .as_ref()
                .and_then(UmbraTask::briefing_system)
                != Some(system_id)
        {
            ui.label(
                egui::RichText::new("Umbra will brief this system as soon as she is free.")
                    .color(theme::muted_text()),
            );
        }
    }

    fn set_umbra_notice(&mut self, system_id: Option<String>, message: String, is_error: bool) {
        self.status = message.clone();
        self.note_umbra(system_id, message, is_error);
    }

    /// Leaves Umbra's word in her panels without touching the status bar.
    fn note_umbra(&mut self, system_id: Option<String>, message: String, is_error: bool) {
        self.umbra_notice = Some(UmbraNotice {
            system_id,
            message,
            is_error,
        });
    }

    fn start_umbra_task(&mut self, task: UmbraTask) {
        if task.is_auto() {
            // Work she starts on her own only clears earlier news about the same system.
            if self
                .umbra_notice
                .as_ref()
                .is_some_and(|notice| notice.system_id.as_deref() == task.briefing_system())
            {
                self.umbra_notice = None;
            }
        } else {
            self.status = task.label().to_owned();
            self.umbra_notice = None;
        }
        self.umbra_task = Some(task);
        self.local_intel_progress = None;
    }

    /// Whether the player can give Umbra something to do: she is idle, or only busy
    /// with a briefing she started on her own.
    fn umbra_free_for_player(&self) -> bool {
        self.umbra_task.as_ref().is_none_or(UmbraTask::is_auto)
    }

    /// Pulls Umbra off a briefing she started on her own so the player's request can
    /// go first. She picks it up again later, while she still briefs on her own.
    fn interrupt_auto_briefing(&mut self) {
        let Some(UmbraTask::Briefing {
            system_id,
            auto: true,
            ..
        }) = &self.umbra_task
        else {
            return;
        };
        if let Some(worker) = &self.umbra_worker {
            worker.stop_current();
        }
        if let Some(assessment) = self.system_assessments.get(system_id).cloned() {
            self.queue_auto_briefing(&assessment);
        }
        self.umbra_task = None;
    }

    /// Clears the way for the player's request: false while Umbra is busy with
    /// something the player asked for.
    fn make_umbra_free_for_player(&mut self) -> bool {
        if !self.umbra_free_for_player() {
            return false;
        }
        self.interrupt_auto_briefing();
        true
    }

    fn start_local_model_install(&mut self) {
        if !self.make_umbra_free_for_player() {
            return;
        }
        // Her files may be replaced, so the loaded core must go.
        self.umbra_worker = None;
        self.start_umbra_task(UmbraTask::Installing);
        let tx = self.local_intel_tx.clone();
        thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = local_intel::install_model(|progress| {
                let _ = progress_tx.send(LocalIntelEvent::Progress(progress));
            })
            .map(|()| LocalIntelTaskResult::Installed);
            let _ = tx.send(LocalIntelEvent::Finished(result));
        });
    }

    fn start_local_model_import(&mut self) {
        if !self.umbra_free_for_player() {
            return;
        }
        let path = local_intel::import_source_path(&self.local_model_import_path);
        if path.as_os_str().is_empty() {
            self.set_umbra_notice(
                None,
                "Enter where you saved Umbra's core first".to_owned(),
                true,
            );
            return;
        }
        self.interrupt_auto_briefing();

        self.umbra_worker = None;
        self.start_umbra_task(UmbraTask::Importing);
        let tx = self.local_intel_tx.clone();
        thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = local_intel::import_model(&path, |progress| {
                let _ = progress_tx.send(LocalIntelEvent::Progress(progress));
            })
            .map(|()| LocalIntelTaskResult::Imported);
            let _ = tx.send(LocalIntelEvent::Finished(result));
        });
    }

    /// Checksums a core that has not been verified since it last changed, e.g. one
    /// installed by an older Nomadeer, once per session.
    fn maybe_check_umbra_core(&mut self) {
        if self.umbra_task.is_none()
            && !self.umbra_check_attempted
            && local_intel::core_status() == CoreStatus::Unchecked
        {
            self.start_umbra_core_check();
        }
    }

    fn start_umbra_core_check(&mut self) {
        if !self.make_umbra_free_for_player() {
            return;
        }
        self.umbra_check_attempted = true;
        self.start_umbra_task(UmbraTask::Checking);
        let tx = self.local_intel_tx.clone();
        thread::spawn(move || {
            let result = local_intel::verify_core().map(|()| LocalIntelTaskResult::Checked);
            let _ = tx.send(LocalIntelEvent::Finished(result));
        });
    }

    fn remove_local_model(&mut self) {
        if !self.make_umbra_free_for_player() {
            return;
        }
        // Drop the loaded core first so its memory is freed with the files.
        self.umbra_worker = None;
        let message = if local_intel::core_status() == CoreStatus::Missing {
            "Old Umbra files cleared"
        } else {
            "Umbra removed"
        };
        match local_intel::remove_model() {
            Ok(()) => {
                self.umbra_needs_repair = false;
                self.umbra_check_attempted = false;
                self.set_umbra_notice(None, message.to_owned(), false);
            }
            Err(error) => {
                self.set_umbra_notice(None, format!("Could not remove Umbra: {error}"), true)
            }
        }
    }

    /// Signs the character in again. `with_online_status` asks to see whether they are
    /// logged in for this sign-in only, so the saved scopes stay as the player set them.
    fn start_authorize_account(&mut self, account_id: String, with_online_status: bool) {
        if self.esi_busy {
            self.status = "Already working on a character update".to_owned();
            return;
        }
        if !self.save_esi_settings_from_form() {
            return;
        }
        if self.data.esi_settings.client_id.is_empty() {
            self.status = "Set your ESI Client ID before connecting a character".to_owned();
            return;
        }

        let mut settings = self.data.esi_settings.clone();
        if with_online_status {
            settings.scopes = esi::with_scope(&settings.scopes, ONLINE_SCOPE);
        }
        self.spawn_authorize_account(account_id, settings);
    }

    fn start_add_character(&mut self) {
        if self.esi_busy {
            self.status = "Already working on a character update".to_owned();
            return;
        }
        if !self.save_esi_settings_from_form() {
            self.show_settings = true;
            return;
        }
        if self.data.esi_settings.client_id.is_empty() {
            self.status = "Set your ESI Client ID before adding a character".to_owned();
            self.show_settings = true;
            return;
        }

        let timestamp = now();
        let account_id = self.new_id("acct");
        self.data.accounts.push(Account {
            id: account_id.clone(),
            character_id: String::new(),
            character_name: "Authorizing...".to_owned(),
            owner_hash: String::new(),
            auth_status: "authorizing".to_owned(),
            last_system_id: String::new(),
            last_system_name: String::new(),
            access_token: None,
            refresh_token: None,
            token_expires_at: None,
            created_at: timestamp,
            updated_at: timestamp,
            recon: false,
        });
        recon::mark_added_character(&mut self.data, &account_id);
        self.save();
        self.spawn_authorize_account(account_id, self.data.esi_settings.clone());
    }

    fn spawn_authorize_account(&mut self, account_id: String, settings: EsiSettings) {
        self.esi_busy = true;
        self.background_esi_task = false;
        self.status = "Opening browser to connect character...".to_owned();
        let tx = self.esi_tx.clone();
        thread::spawn(move || {
            let result =
                esi::authorize_account(settings, account_id).map(EsiTaskResult::Authorized);
            let _ = tx.send(result);
        });
    }

    fn start_sync_location(&mut self, account_id: String) {
        if self.esi_busy {
            self.status = "Already working on a character update".to_owned();
            return;
        }
        let Some(account) = self
            .data
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .cloned()
        else {
            self.status = "Character not found".to_owned();
            return;
        };

        self.spawn_sync_location(account, true);
    }

    fn spawn_sync_location(&mut self, account: Account, show_status: bool) {
        self.esi_busy = true;
        self.background_esi_task = !show_status;
        if show_status {
            self.status = format!("Updating {} location...", account.character_name);
        }
        let tx = self.esi_tx.clone();
        let settings = self.data.esi_settings.clone();
        thread::spawn(move || {
            let result = esi::sync_location(settings, account).map(EsiTaskResult::Location);
            let _ = tx.send(result);
        });
    }

    /// Checks whether the next character is logged in, when ESI has a newer answer.
    fn maybe_poll_online(&mut self) {
        if self.esi_busy || self.data.esi_settings.client_id.is_empty() {
            return;
        }
        let Some(account) = self
            .online_polls
            .next_due(
                self.data
                    .accounts
                    .iter()
                    .filter(|account| recon::online_status_available(account))
                    .map(|account| account.id.as_str()),
                Instant::now(),
            )
            .and_then(|account_id| {
                self.data
                    .accounts
                    .iter()
                    .find(|account| account.id == account_id)
            })
            .cloned()
        else {
            return;
        };

        self.esi_busy = true;
        self.background_esi_task = true;
        self.online_poll_in_flight = Some(account.id.clone());
        let tx = self.esi_tx.clone();
        let settings = self.data.esi_settings.clone();
        thread::spawn(move || {
            let result = esi::sync_online(settings, account).map(EsiTaskResult::Online);
            let _ = tx.send(result);
        });
    }

    fn maybe_auto_sync(&mut self) {
        if self.esi_busy || Instant::now() < self.next_auto_sync {
            return;
        }

        let accounts: Vec<Account> = self
            .data
            .accounts
            .iter()
            .filter(|account| {
                account.auth_status == "authorized"
                    && self
                        .online_polls
                        .wants_location_poll(&account.id, recon::online_status_available(account))
            })
            .cloned()
            .collect();
        if accounts.is_empty() || self.data.esi_settings.client_id.is_empty() {
            self.next_auto_sync = Instant::now() + AUTO_SYNC_INTERVAL;
            return;
        }

        let index = self.auto_sync_index % accounts.len();
        self.auto_sync_index = self.auto_sync_index.wrapping_add(1);
        self.next_auto_sync = Instant::now() + AUTO_SYNC_INTERVAL;
        self.spawn_sync_location(accounts[index].clone(), false);
    }

    /// Puts the selected system at the front of the killboard queue.
    fn start_zkill_refresh(&mut self) {
        let Some(system) = self.selected_system() else {
            self.status = "Select a system first".to_owned();
            return;
        };
        if system.solar_system_id == 0 {
            self.status = "Selected system has no EVE system ID yet".to_owned();
            return;
        }

        let solar_system_id = system.solar_system_id;
        let system_name = system.name.clone();
        self.intel_queue
            .push(solar_system_id, IntelRequest::Player, Instant::now());
        if self.intel_queue.in_flight().is_some() {
            self.status = format!("{system_name} is next for killboard intel");
        }
        self.maybe_start_intel();
    }

    /// Queues killboard intel for a system a character just arrived in, unless it
    /// already has a fresh assessment.
    fn queue_arrival_intel(&mut self, system_id: &str) {
        let Some(system) = self.system_by_id(system_id) else {
            return;
        };
        let assessed_at = self
            .system_assessments
            .get(system_id)
            .map(|assessment| assessment.generated_at);
        if auto_intel::wants_intel_on_arrival(system.solar_system_id, assessed_at, now()) {
            let solar_system_id = system.solar_system_id;
            self.intel_queue
                .push(solar_system_id, IntelRequest::Arrival, Instant::now());
        }
    }

    /// Starts the next queued killboard refresh when the last one is done.
    fn maybe_start_intel(&mut self) {
        let systems = &self.data.systems;
        let Some((solar_system_id, request)) =
            self.intel_queue
                .start_next(Instant::now(), |solar_system_id| {
                    systems
                        .iter()
                        .any(|system| system.solar_system_id == solar_system_id)
                })
        else {
            return;
        };
        // Arrivals are gathered quietly; the selected system's panel shows them.
        if request == IntelRequest::Player
            && let Some(system) = systems
                .iter()
                .find(|system| system.solar_system_id == solar_system_id)
        {
            self.status = format!("Fetching killboard intel for {}...", system.name);
        }
        let tx = self.zkill_tx.clone();
        let store = self.store.clone();
        thread::spawn(move || {
            let _ = tx.send(Self::run_zkill_refresh(&store, solar_system_id));
        });
    }

    /// Runs on a worker thread: merges cached killmails with fresh zKill data, stores
    /// the result and reads the system's traffic history for the assessment.
    fn run_zkill_refresh(store: &Store, solar_system_id: u64) -> Result<ZkillTaskResult, String> {
        let current_time = now();
        let mut save_error = None;
        let cached = store.cached_killmails(
            solar_system_id,
            current_time.saturating_sub(KILLMAIL_RETENTION_SECS),
        );
        // Without the cached kills, their coverage would let paging stop short.
        let (cached_killmails, coverage) = match cached {
            Ok(kills) => {
                let coverage = store
                    .killboard_coverage(solar_system_id)
                    .unwrap_or_else(|error| {
                        save_error = Some(error);
                        None
                    });
                (kills, coverage)
            }
            Err(error) => {
                save_error = Some(error);
                (Vec::new(), None)
            }
        };
        let mut report =
            zkill::fetch_system_report(solar_system_id, cached_killmails, coverage, current_time)?;
        if let Err(error) = store.store_killmails(
            &report.fetched_killmails,
            Some(&report.coverage),
            current_time,
        ) {
            save_error.get_or_insert(error);
        }
        // The fetched killmails now live in the database; the report keeps its summary.
        report.fetched_killmails = Vec::new();
        let traffic = store
            .system_traffic(
                solar_system_id,
                current_time.saturating_sub(TRAFFIC_RETENTION_SECS),
            )
            .unwrap_or_else(|error| {
                save_error.get_or_insert(error);
                SystemTrafficHistory::default()
            });
        let killmails = store
            .cached_killmails(
                solar_system_id,
                current_time.saturating_sub(KILLMAIL_RETENTION_SECS),
            )
            .ok();
        Ok(ZkillTaskResult::Report {
            report,
            traffic,
            killmails,
            save_error,
        })
    }

    fn maybe_poll_traffic(&mut self) {
        let current = Instant::now();
        for state in &mut self.traffic_feeds {
            if state.busy || current < state.next_poll {
                continue;
            }
            state.busy = true;
            let feed = state.feed;
            let store = self.store.clone();
            let tx = self.traffic_tx.clone();
            thread::spawn(move || {
                let result = esi::fetch_traffic_feed(feed).and_then(|capture| {
                    store.record_traffic_hour(&capture.traffic, now())?;
                    Ok(capture.expires_at)
                });
                let _ = tx.send(TrafficTaskResult { feed, result });
            });
        }
    }

    fn poll_traffic_tasks(&mut self) {
        while let Ok(task) = self.traffic_rx.try_recv() {
            let Some(state) = self
                .traffic_feeds
                .iter_mut()
                .find(|state| state.feed == task.feed)
            else {
                continue;
            };
            state.busy = false;
            match task.result {
                Ok(expires_at) => {
                    state.failures = 0;
                    state.next_poll =
                        Instant::now() + esi::traffic_next_poll_delay(expires_at, now());
                }
                Err(_error) => {
                    // Traffic history is background intel; a missed hour is just a gap.
                    state.failures = state.failures.saturating_add(1);
                    state.next_poll = Instant::now() + esi::traffic_retry_delay(state.failures);
                }
            }
        }
    }

    fn poll_zkill_tasks(&mut self) {
        loop {
            match self.zkill_rx.try_recv() {
                Ok(Ok(ZkillTaskResult::Report {
                    report,
                    traffic,
                    killmails,
                    save_error,
                })) => {
                    let background = self
                        .intel_queue
                        .finish(Instant::now())
                        .is_some_and(|(_, request)| request == IntelRequest::Arrival);
                    self.intel_errors.remove(&report.solar_system_id);
                    let kill_count = report.history.kills_7d + report.history.npc_kills_7d;
                    if let Some(killmails) = killmails {
                        self.keep_latest_kills(
                            report.solar_system_id,
                            live_intel::SystemKills {
                                killmails,
                                coverage: Some(report.coverage),
                            },
                        );
                    }
                    let assessment = self
                        .data
                        .systems
                        .iter()
                        .find(|system| system.solar_system_id == report.solar_system_id)
                        .cloned()
                        .map(|system| self.assess(&system, &report, &traffic, now()));
                    let mut save_error = save_error;
                    if let Some(mut assessment) = assessment {
                        if let Some(previous) = self.system_assessments.get(&assessment.system_id) {
                            local_intel::keep_briefing(&mut assessment, previous);
                        }
                        self.system_assessments
                            .insert(assessment.system_id.clone(), assessment.clone());
                        self.queue_auto_briefing(&assessment);
                        self.upsert_system_assessment(assessment);
                        if let Err(error) = self.store.save(&self.data) {
                            save_error.get_or_insert(error);
                        }
                    } else {
                        // The system left the map meanwhile; arriving there again
                        // should gather its intel afresh.
                        self.intel_queue.forget_refresh(report.solar_system_id);
                    }
                    let status = match (save_error, &report.interrupted) {
                        (Some(error), _) => {
                            Some(format!("Intel loaded, but it could not be saved: {error}"))
                        }
                        // Intel gathered on arrival speaks up only when it could not be kept.
                        _ if background => None,
                        (None, Some(error)) => Some(format!(
                            "Loaded {kill_count} kills before zKillboard stopped answering: {error}"
                        )),
                        (None, None) if report.history.partial_since.is_some() => Some(format!(
                            "Loaded the latest {kill_count} kills; older kills from this week will be read on later refreshes"
                        )),
                        (None, None) => {
                            Some(format!("Loaded {kill_count} kills from the last 7 days"))
                        }
                    };
                    if let Some(status) = status {
                        self.status = status;
                    }
                    self.refreshed_traffic
                        .insert(report.solar_system_id, traffic);
                    self.zkill_reports.insert(report.solar_system_id, report);
                }
                Ok(Err(error)) => {
                    let finished = self.intel_queue.finish(Instant::now());
                    if let Some((solar_system_id, request)) = finished {
                        if request == IntelRequest::Player {
                            self.status = format!("Killboard intel failed: {error}");
                        }
                        self.intel_errors.insert(solar_system_id, error);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.intel_queue.finish(Instant::now());
                    self.status = "Killboard update stopped unexpectedly".to_owned();
                    break;
                }
            }
        }
    }

    /// Player and NPC kills over the week, marked "+" when the week is not fully read.
    fn killboard_kill_count(report: &ZkillReport) -> String {
        let count = report.history.kills_7d + report.history.npc_kills_7d;
        if report.history.partial_since.is_some() {
            format!("{count}+")
        } else {
            count.to_string()
        }
    }

    fn maybe_request_portraits(&mut self) {
        for account in &self.data.accounts {
            if account.character_id.is_empty()
                || self.portrait_textures.contains_key(&account.id)
                || self.portrait_requested.contains(&account.id)
            {
                continue;
            }

            self.portrait_requested.insert(account.id.clone());
            let account_id = account.id.clone();
            let character_id = account.character_id.clone();
            let tx = self.portrait_tx.clone();
            thread::spawn(move || {
                let result = portrait::fetch_character_portrait(&character_id)
                    .map(|image| PortraitTaskResult { account_id, image });
                let _ = tx.send(result);
            });
        }
    }

    fn poll_portrait_tasks(&mut self, ctx: &egui::Context) {
        loop {
            match self.portrait_rx.try_recv() {
                Ok(Ok(result)) => {
                    let texture = ctx.load_texture(
                        format!("portrait_{}", result.account_id),
                        result.image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.portrait_textures.insert(result.account_id, texture);
                }
                Ok(Err(_error)) => {
                    // Keep portrait failures quiet; names/location still matter more than cosmetics.
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn start_local_briefing(&mut self, system_id: &str) {
        // Already briefing this system on her own: the briefing becomes the player's.
        if let Some(UmbraTask::Briefing {
            system_id: briefing,
            auto,
            ..
        }) = &mut self.umbra_task
            && *auto
            && briefing == system_id
            && self
                .system_assessments
                .get(system_id)
                .is_some_and(|assessment| !assessment.model_generated)
        {
            *auto = false;
            if let Some(task) = &self.umbra_task {
                self.status = task.label().to_owned();
            }
            self.umbra_notice = None;
            return;
        }
        if !self.make_umbra_free_for_player() {
            self.status = "Umbra is already working".to_owned();
            return;
        }
        match local_intel::core_status() {
            _ if self.umbra_needs_repair => {
                self.show_local_intel_settings = true;
                self.set_umbra_notice(
                    Some(system_id.to_owned()),
                    "Umbra needs repairing before she can brief you".to_owned(),
                    true,
                );
                return;
            }
            CoreStatus::Missing => {
                self.show_local_intel_settings = true;
                self.set_umbra_notice(
                    Some(system_id.to_owned()),
                    "Umbra needs to be installed before she can brief you".to_owned(),
                    true,
                );
                return;
            }
            CoreStatus::Unchecked => {
                // Her core changed since it was last checked; check it, then ask again.
                self.start_umbra_core_check();
                return;
            }
            CoreStatus::Ready => {}
        }
        // The chain may have changed since the assessment, e.g. a hole marked EOL.
        self.refresh_chain_facts(system_id);
        let Some(assessment) = self.system_assessments.get(system_id).cloned() else {
            self.set_umbra_notice(
                Some(system_id.to_owned()),
                "Refresh zKill so Umbra has evidence to brief from".to_owned(),
                true,
            );
            return;
        };
        self.request_briefing(assessment, false);
    }

    /// Hands the assessment to Umbra's worker, waking her if she is not loaded yet.
    fn request_briefing(&mut self, assessment: SystemAssessment, auto: bool) {
        let system_id = assessment.system_id.clone();
        let tx = self.local_intel_tx.clone();
        let worker = self.umbra_worker.get_or_insert_with(|| {
            BriefingWorker::spawn(move |event| {
                let _ = tx.send(LocalIntelEvent::Briefing(event));
            })
        });
        if let Err(error) = worker.request(assessment) {
            self.umbra_worker = None;
            if auto {
                self.note_umbra(Some(system_id), error, true);
            } else {
                self.set_umbra_notice(Some(system_id), error, true);
            }
            return;
        }
        self.start_umbra_task(UmbraTask::Briefing {
            system_id,
            waking: false,
            auto,
        });
    }

    /// Asks Umbra to brief a fresh assessment on her own, when she is installed and
    /// the player wants her to. Without her core nothing is asked.
    fn queue_auto_briefing(&mut self, assessment: &SystemAssessment) {
        if self.data.auto_intel.umbra_briefs_automatically
            && !assessment.model_generated
            && !self.umbra_needs_repair
            && local_intel::core_status() != CoreStatus::Missing
        {
            self.auto_briefings.request(assessment.clone());
        }
    }

    /// Starts the next briefing Umbra owes once she is free: the selected system's
    /// first, then the newest.
    fn maybe_start_auto_briefing(&mut self) {
        if self.umbra_task.is_some() || self.auto_briefings.is_empty() {
            return;
        }
        if !self.data.auto_intel.umbra_briefs_automatically {
            self.auto_briefings.clear();
            return;
        }
        match local_intel::core_status() {
            _ if self.umbra_needs_repair => {
                self.auto_briefings.clear();
                return;
            }
            CoreStatus::Ready => {}
            // Her core is checked once per session before she briefs.
            CoreStatus::Unchecked if !self.umbra_check_attempted => return,
            CoreStatus::Unchecked | CoreStatus::Missing => {
                self.auto_briefings.clear();
                return;
            }
        }
        if let Some(assessment) = self
            .auto_briefings
            .next(&self.system_assessments, &self.selected_system_id)
        {
            self.request_briefing(assessment, true);
        }
    }

    fn poll_local_intel_tasks(&mut self) {
        loop {
            match self.local_intel_rx.try_recv() {
                Ok(LocalIntelEvent::Progress(progress)) => {
                    self.local_intel_progress = Some(progress);
                }
                Ok(LocalIntelEvent::Finished(result)) => {
                    let task = self.umbra_task.take();
                    self.local_intel_progress = None;
                    match result {
                        Ok(done) => {
                            self.umbra_needs_repair = false;
                            let message = match done {
                                LocalIntelTaskResult::Installed
                                | LocalIntelTaskResult::Imported => "Umbra is online",
                                LocalIntelTaskResult::Checked => "Umbra's core checks out",
                            };
                            self.set_umbra_notice(None, message.to_owned(), false);
                        }
                        Err(error) => {
                            if task == Some(UmbraTask::Checking) {
                                self.umbra_needs_repair = true;
                            }
                            let message = match task {
                                Some(UmbraTask::Checking) => error,
                                _ => format!("Umbra could not be installed: {error}"),
                            };
                            self.set_umbra_notice(None, message, true);
                        }
                    }
                }
                Ok(LocalIntelEvent::Briefing(event)) => self.apply_briefing_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.umbra_task = None;
                    self.local_intel_progress = None;
                    break;
                }
            }
        }
    }

    fn apply_briefing_event(&mut self, event: BriefingEvent) {
        match event {
            BriefingEvent::Waking | BriefingEvent::Writing => {
                let waking_up = matches!(event, BriefingEvent::Waking);
                if let Some(UmbraTask::Briefing { waking, .. }) = &mut self.umbra_task {
                    *waking = waking_up;
                }
                if let Some(task) = &self.umbra_task
                    && !task.is_auto()
                {
                    self.status = task.label().to_owned();
                }
            }
            BriefingEvent::Written { source, briefing } => {
                // One she was pulled off may finish anyway; it is kept quietly.
                let auto = self
                    .end_briefing_task(&source.system_id)
                    .is_none_or(|auto| auto);
                let briefed = self
                    .system_assessments
                    .get(&source.system_id)
                    .and_then(|current| local_intel::attach_briefing(current, &source, briefing));
                let Some(briefed) = briefed else {
                    // The newer intel has its own briefing coming.
                    if auto {
                        return;
                    }
                    if self.auto_briefings.is_pending(&source.system_id) {
                        self.set_umbra_notice(
                            Some(source.system_id),
                            "The intel changed while Umbra was writing, so she is briefing the newer intel instead.".to_owned(),
                            false,
                        );
                    } else {
                        self.set_umbra_notice(
                            Some(source.system_id),
                            "The intel changed while Umbra was writing, so she set her briefing aside. Ask her again for the latest picture.".to_owned(),
                            true,
                        );
                    }
                    return;
                };
                self.system_assessments
                    .insert(briefed.system_id.clone(), briefed.clone());
                self.record_umbra_briefing(&briefed, auto);
                self.upsert_system_assessment(briefed);
                match self.store.save(&self.data) {
                    Ok(()) if auto => {}
                    Ok(()) => self.status = "Umbra's briefing is ready".to_owned(),
                    Err(error) => self.set_umbra_notice(
                        Some(source.system_id),
                        format!("Umbra's briefing is ready, but it could not be saved: {error}"),
                        true,
                    ),
                }
            }
            BriefingEvent::Failed {
                source,
                error,
                needs_repair,
            } => {
                // A briefing she was pulled off has nothing more to say.
                let Some(auto) = self.end_briefing_task(&source.system_id) else {
                    return;
                };
                if needs_repair {
                    self.umbra_needs_repair = true;
                    self.umbra_worker = None;
                }
                if auto {
                    self.note_umbra(Some(source.system_id), error, true);
                } else {
                    self.set_umbra_notice(Some(source.system_id), error, true);
                }
            }
            // Only a briefing she was pulled off stops, and she has already moved on.
            BriefingEvent::Stopped => {}
        }
    }

    /// Ends her briefing task when the event reports on it, returning whether she
    /// was briefing on her own. None when it is about a briefing she was pulled off.
    fn end_briefing_task(&mut self, system_id: &str) -> Option<bool> {
        if self
            .umbra_task
            .as_ref()
            .and_then(UmbraTask::briefing_system)
            != Some(system_id)
        {
            return None;
        }
        self.umbra_task.take().map(|task| task.is_auto())
    }

    fn poll_esi_tasks(&mut self) {
        loop {
            match self.esi_rx.try_recv() {
                Ok(Ok(EsiTaskResult::Authorized(auth))) => {
                    self.esi_busy = false;
                    self.apply_authorization(auth);
                }
                Ok(Ok(EsiTaskResult::Location(location))) => {
                    self.esi_busy = false;
                    self.apply_location_sync(location);
                }
                Ok(Ok(EsiTaskResult::Online(online))) => {
                    self.esi_busy = false;
                    self.online_poll_in_flight = None;
                    self.apply_online_sync(online);
                }
                Ok(Err(error)) => {
                    self.esi_busy = false;
                    if let Some(account_id) = self.online_poll_in_flight.take() {
                        self.online_polls
                            .record_failure(&account_id, Instant::now());
                    }
                    // A failed background sync is retried in a few seconds anyway.
                    if !self.background_esi_task {
                        self.status = format!("Character update failed: {error}");
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.esi_busy = false;
                    self.online_poll_in_flight = None;
                    self.status = "Character update stopped unexpectedly".to_owned();
                    break;
                }
            }
        }
    }

    fn apply_authorization(&mut self, auth: AccountAuthorization) {
        if let Some(account) = self
            .data
            .accounts
            .iter_mut()
            .find(|account| account.id == auth.account_id)
        {
            account.character_id = auth.character_id;
            account.character_name = auth.character_name;
            account.owner_hash = auth.owner_hash;
            account.auth_status = "authorized".to_owned();
            account.access_token = Some(auth.access_token);
            account.refresh_token = Some(auth.refresh_token);
            account.token_expires_at = Some(auth.token_expires_at);
            account.updated_at = now();
            // The new sign-in may read online status the old one could not.
            self.online_polls.forget(&auth.account_id);
            self.save();
            self.status = "Character connected".to_owned();
        } else {
            self.status = "Character connected, but it is no longer in the list".to_owned();
        }
    }

    fn apply_online_sync(&mut self, sync: OnlineSync) {
        let Some(account) = self
            .data
            .accounts
            .iter_mut()
            .find(|account| account.id == sync.account_id)
        else {
            return;
        };
        let tokens_changed = account.access_token.as_deref() != Some(sync.access_token.as_str())
            || account.refresh_token.as_deref() != Some(sync.refresh_token.as_str())
            || account.token_expires_at != Some(sync.token_expires_at);
        if tokens_changed {
            account.access_token = Some(sync.access_token);
            account.refresh_token = Some(sync.refresh_token);
            account.token_expires_at = Some(sync.token_expires_at);
            account.updated_at = now();
        }

        let was_online = self.online_polls.record(
            &sync.account_id,
            sync.online,
            sync.cache_lifetime,
            Instant::now(),
        );
        // The next location after logging in is judged like the first of the session:
        // it is where the pilot is flying now, which may already be past where they
        // logged off, as this status can trail the login by a minute or two.
        if auto_intel::logged_in(was_online, sync.online) {
            self.located_this_session.remove(&sync.account_id);
        }
        if tokens_changed && let Err(error) = self.store.save(&self.data) {
            self.status = format!("Save failed: {error}");
        }
    }

    fn apply_location_sync(&mut self, location: LocationSync) {
        let mapped_systems = self.data.systems.len();
        let (system_id, system_changed) = self.ensure_esi_system(&location.system);
        let node_created = self.data.systems.len() > mapped_systems;

        let Some(account_index) = self
            .data
            .accounts
            .iter()
            .position(|account| account.id == location.account_id)
        else {
            self.status = "Location updated, but the character is no longer in the list".to_owned();
            return;
        };

        let previous_system_id = self.data.accounts[account_index].last_system_id.clone();
        let character_name = self.data.accounts[account_index].character_name.clone();
        let is_recon = self.data.accounts[account_index].recon;
        let presence = self.online_polls.presence(
            &location.account_id,
            recon::online_status_available(&self.data.accounts[account_index]),
        );

        let account = &mut self.data.accounts[account_index];
        let account_changed = account.access_token.as_deref()
            != Some(location.access_token.as_str())
            || account.refresh_token.as_deref() != Some(location.refresh_token.as_str())
            || account.token_expires_at != Some(location.token_expires_at)
            || account.last_system_id != system_id
            || account.last_system_name != location.system.name
            || account.auth_status != "authorized";
        if account_changed {
            account.access_token = Some(location.access_token);
            account.refresh_token = Some(location.refresh_token);
            account.token_expires_at = Some(location.token_expires_at);
            account.last_system_id = system_id.clone();
            account.last_system_name = location.system.name.clone();
            account.auth_status = "authorized".to_owned();
            account.updated_at = now();
        }

        let moved = previous_system_id.is_empty() || previous_system_id != system_id;
        if moved {
            let visit_id = self.new_id("visit");
            self.data.visits.push(VisitLog {
                id: visit_id,
                system_id: system_id.clone(),
                pilot: character_name.clone(),
                activity: if previous_system_id.is_empty() {
                    "Location found".to_owned()
                } else {
                    "Moved system".to_owned()
                },
                notes: String::new(),
                created_at: now(),
            });
        }

        if !previous_system_id.is_empty() && previous_system_id != system_id {
            self.ensure_connection(&previous_system_id, &system_id, &character_name);
        }

        if self.selected_system_id.is_empty() {
            self.selected_system_id = system_id.clone();
        }
        let first_fix = self
            .located_this_session
            .insert(location.account_id.clone());
        if self.data.auto_intel.gather_on_arrival
            && auto_intel::is_arrival(
                is_recon,
                presence,
                &LocationFix {
                    previous_system_id: &previous_system_id,
                    system_id: &system_id,
                    first_fix_this_session: first_fix,
                    node_created,
                    on_screen: system_id == self.selected_system_id,
                },
            )
        {
            self.queue_arrival_intel(&system_id);
        }
        if moved || !self.background_esi_task {
            self.status = format!("{character_name} is in {}", location.system.name);
        }
        // Auto-sync runs every few seconds, so only write when something changed, and
        // only a failure is worth replacing the status for.
        if (system_changed || account_changed || moved)
            && let Err(error) = self.store.save(&self.data)
        {
            self.status = format!("Save failed: {error}");
        }
    }

    fn upsert_system_assessment(&mut self, assessment: SystemAssessment) {
        if let Some(existing) = self
            .data
            .system_assessments
            .iter_mut()
            .find(|existing| existing.system_id == assessment.system_id)
        {
            *existing = assessment;
        } else {
            self.data.system_assessments.push(assessment);
        }
    }

    /// Returns the map node id for the system and whether the node changed.
    fn ensure_esi_system(&mut self, intel: &SystemIntel) -> (String, bool) {
        let esi_id = format!("esi_sys_{}", intel.solar_system_id);
        if let Some(existing) = self
            .data
            .systems
            .iter_mut()
            .find(|system| system.id == esi_id)
        {
            let changed = Self::apply_system_intel(existing, intel);
            return (esi_id, changed);
        }
        if let Some(existing) = self
            .data
            .systems
            .iter_mut()
            .find(|system| system.name == intel.name)
        {
            let changed = Self::apply_system_intel(existing, intel);
            return (existing.id.clone(), changed);
        }

        let mut system = SystemNode {
            id: esi_id.clone(),
            solar_system_id: intel.solar_system_id,
            name: intel.name.clone(),
            class: intel.security_class.clone(),
            security_status: intel.security_status,
            constellation_id: intel.constellation_id,
            constellation_name: intel.constellation_name.clone(),
            region_id: intel.region_id,
            region_name: intel.region_name.clone(),
            planet_count: intel.planet_count,
            stargate_count: intel.stargate_count,
            station_count: intel.station_count,
            jumps_1h: 0,
            ship_kills_1h: 0,
            pod_kills_1h: 0,
            npc_kills_1h: 0,
            intel_updated_at: 0,
            effect: String::new(),
            status: "scouted".to_owned(),
            owner: String::new(),
            notes: format!("Solar system ID {}", intel.solar_system_id),
            created_at: now(),
            map_position: None,
        };
        Self::apply_system_intel(&mut system, intel);
        self.data.systems.push(system);
        (esi_id, true)
    }

    /// Copies ESI intel onto a map node and reports whether anything changed. Activity
    /// that ESI could not provide keeps its previous values, and `intel_updated_at` is
    /// the ESI time of the oldest activity reading on the node.
    fn apply_system_intel(system: &mut SystemNode, intel: &SystemIntel) -> bool {
        let before = system.clone();
        system.solar_system_id = intel.solar_system_id;
        system.name = intel.name.clone();
        system.class = intel.security_class.clone();
        system.security_status = intel.security_status;
        system.constellation_id = intel.constellation_id;
        system.constellation_name = intel.constellation_name.clone();
        system.region_id = intel.region_id;
        system.region_name = intel.region_name.clone();
        system.planet_count = intel.planet_count;
        system.stargate_count = intel.stargate_count;
        system.station_count = intel.station_count;
        let previous_update = system.intel_updated_at;
        let jumps_time = intel.jumps.map_or(previous_update, |jumps| {
            system.jumps_1h = jumps.value;
            jumps.data_time
        });
        let kills_time = intel.kills.map_or(previous_update, |kills| {
            system.ship_kills_1h = kills.value.ship_kills;
            system.pod_kills_1h = kills.value.pod_kills;
            system.npc_kills_1h = kills.value.npc_kills;
            kills.data_time
        });
        system.intel_updated_at = jumps_time.min(kills_time);
        *system != before
    }

    fn ensure_connection(&mut self, from_system: &str, to_system: &str, character_name: &str) {
        let exists = self.data.connections.iter().any(|connection| {
            (connection.from_system == from_system && connection.to_system == to_system)
                || (connection.from_system == to_system && connection.to_system == from_system)
        });
        if exists {
            return;
        }

        let connection_id = self.new_id("conn");
        self.data.connections.push(Connection {
            id: connection_id,
            from_system: from_system.to_owned(),
            to_system: to_system.to_owned(),
            signature_from: String::new(),
            signature_to: String::new(),
            mass_status: "unknown".to_owned(),
            life_status: "unknown".to_owned(),
            ship_size: String::new(),
            notes: format!("Inferred from {character_name} movement"),
            created_at: now(),
            ..Connection::default()
        });
    }

    fn create_manual_connection(&mut self, from_system: &str, to_system: &str) {
        if from_system == to_system {
            self.status = "Drag to a different system to make a connection".to_owned();
            return;
        }

        let exists = self.data.connections.iter().any(|connection| {
            (connection.from_system == from_system && connection.to_system == to_system)
                || (connection.from_system == to_system && connection.to_system == from_system)
        });
        if exists {
            self.status = "That connection already exists".to_owned();
            return;
        }

        let from_name = self.system_name(from_system);
        let to_name = self.system_name(to_system);
        let connection_id = self.new_id("conn");
        self.data.connections.push(Connection {
            id: connection_id,
            from_system: from_system.to_owned(),
            to_system: to_system.to_owned(),
            signature_from: String::new(),
            signature_to: String::new(),
            mass_status: "unknown".to_owned(),
            life_status: "unknown".to_owned(),
            ship_size: String::new(),
            notes: "Linked on chain map".to_owned(),
            created_at: now(),
            ..Connection::default()
        });
        self.save();
        self.status = format!("Linked {from_name} to {to_name}");
    }

    fn save_chain_snapshot(&mut self) {
        let name = clean(&self.chain_snapshot_name);
        if name.is_empty() {
            self.status = "Name this chain before saving it".to_owned();
            return;
        }

        let snapshot = ChainSnapshot {
            name: name.clone(),
            created_at: now(),
            selected_system_id: self.selected_system_id.clone(),
            systems: self.data.systems.clone(),
            connections: self.data.connections.clone(),
            signatures: self.data.signatures.clone(),
            visits: self.data.visits.clone(),
            notes: self.data.notes.clone(),
        };

        match chain_archive::save_snapshot(&paths::chain_archive_dir(), &snapshot) {
            Ok(_) => {
                self.chain_snapshot_name = format!("Chain {}", now());
                self.refresh_chain_snapshots();
                self.status = format!("Saved chain {name}");
            }
            Err(error) => self.status = format!("Could not save chain: {error}"),
        }
    }

    fn load_chain_snapshot(&mut self, file_name: &str) {
        match chain_archive::load_snapshot(&paths::chain_archive_dir(), file_name) {
            Ok(snapshot) => {
                let name = snapshot.name.clone();
                self.data.systems = snapshot.systems;
                self.data.connections = snapshot.connections;
                self.data.signatures = snapshot.signatures;
                self.data.visits = snapshot.visits;
                self.data.notes = snapshot.notes;
                self.selected_system_id = snapshot.selected_system_id;
                self.forget_chain_view();
                self.clear_account_locations_outside_chain();
                self.retain_assessments_in_chain();
                self.reconcile_selected_system();
                self.save();
                self.status = format!("Loaded chain {name}");
            }
            Err(error) => self.status = format!("Could not load chain: {error}"),
        }
    }

    fn delete_chain_snapshot(&mut self, file_name: &str) {
        match chain_archive::delete_snapshot(&paths::chain_archive_dir(), file_name) {
            Ok(()) => {
                self.refresh_chain_snapshots();
                self.status = "Deleted saved chain".to_owned();
            }
            Err(error) => self.status = format!("Could not delete saved chain: {error}"),
        }
    }

    fn clear_chain(&mut self) {
        self.data.systems.clear();
        self.data.connections.clear();
        self.data.signatures.clear();
        self.data.visits.clear();
        self.data.notes.clear();
        self.selected_system_id.clear();
        self.forget_chain_view();
        for account in &mut self.data.accounts {
            account.last_system_id.clear();
            account.last_system_name.clear();
        }
        self.retain_assessments_in_chain();
        self.save();
        self.status = "Cleared current chain".to_owned();
    }

    /// A different chain is fitted into view afresh, and nothing held on the old one
    /// carries over.
    fn forget_chain_view(&mut self) {
        self.chain_camera = None;
        self.chain_drag = None;
        self.chain_confirm_reset_layout = false;
        self.chain_context_target = None;
    }

    /// Assessments belong to map nodes, so drop any whose node is no longer mapped.
    /// Killmails and traffic history are keyed by solar system and kept.
    fn retain_assessments_in_chain(&mut self) {
        let system_ids = self
            .data
            .systems
            .iter()
            .map(|system| system.id.clone())
            .collect::<HashSet<_>>();
        self.data
            .system_assessments
            .retain(|assessment| system_ids.contains(&assessment.system_id));
        self.system_assessments
            .retain(|system_id, _| system_ids.contains(system_id));
    }

    fn clear_account_locations_outside_chain(&mut self) {
        let system_ids = self
            .data
            .systems
            .iter()
            .map(|system| system.id.clone())
            .collect::<HashSet<_>>();
        for account in &mut self.data.accounts {
            if !account.last_system_id.is_empty() && !system_ids.contains(&account.last_system_id) {
                account.last_system_id.clear();
                account.last_system_name.clear();
            }
        }
    }

    fn delete_connection(&mut self, connection_id: &str) {
        let before = self.data.connections.len();
        self.data
            .connections
            .retain(|connection| connection.id != connection_id);
        if self.data.connections.len() != before {
            self.save();
            self.status = "Deleted connection".to_owned();
        }
    }

    fn delete_system(&mut self, system_id: &str) {
        let Some((system_name, solar_system_id)) = self
            .data
            .systems
            .iter()
            .find(|system| system.id == system_id)
            .map(|system| (system.name.clone(), system.solar_system_id))
        else {
            self.status = "System not found".to_owned();
            return;
        };

        self.data.systems.retain(|system| system.id != system_id);
        self.data.connections.retain(|connection| {
            connection.from_system != system_id && connection.to_system != system_id
        });
        self.data
            .signatures
            .retain(|signature| signature.system_id != system_id);
        self.data
            .visits
            .retain(|visit| visit.system_id != system_id);
        self.data.notes.retain(|note| note.system_id != system_id);
        self.data
            .system_assessments
            .retain(|assessment| assessment.system_id != system_id);
        self.system_assessments.remove(system_id);
        self.zkill_reports.remove(&solar_system_id);
        self.forget_live_intel(system_id);
        for account in &mut self.data.accounts {
            if account.last_system_id == system_id {
                account.last_system_id.clear();
                account.last_system_name.clear();
            }
        }

        if self.selected_system_id == system_id {
            self.selected_system_id.clear();
            self.reconcile_selected_system();
        }

        self.save();
        self.status = format!("Deleted {system_name}");
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("dashboard_sidebar")
            .resizable(true)
            .default_width(320.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("dashboard_sidebar_scroll")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add_space(2.0);
                        self.selected_system_sidebar(ui);
                        ui.separator();
                        self.characters_sidebar(ui);
                    });
            });
    }

    fn central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.heading("Dashboard");
                ui.label(format!(
                    "{} systems · {} connections · {} visits",
                    self.data.systems.len(),
                    self.data.connections.len(),
                    self.data.visits.len()
                ));
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("Chain map");
                ui.label(chain_map::CHAIN_MAP_HINT);
            });
            let area = ui.available_rect_before_wrap();
            let split = self.map_divider(ui, area);
            self.chain_map(ui, split.map);
            self.dashboard_panes(ui, split.below);
        });
    }

    /// The divider under the chain map. Dragging moves it, a double-click puts it back;
    /// the new height is saved when the player lets go. Returns where everything sits.
    fn map_divider(&mut self, ui: &mut egui::Ui, area: egui::Rect) -> layout::MapSplit {
        let split = layout::map_split(area, self.data.dashboard_layout.map_fraction);
        let response = Self::divider_response(ui, "map_divider", split.divider);
        let mut moved = false;
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let fraction = layout::map_fraction_at(pointer.y, area);
            if fraction != self.data.dashboard_layout.map_fraction {
                self.data.dashboard_layout.map_fraction = fraction;
                moved = true;
            }
        }
        if response.drag_stopped() {
            self.save_quietly();
        }
        if response.double_clicked() {
            self.data.dashboard_layout.map_fraction = DashboardLayout::DEFAULT_MAP_FRACTION;
            self.save_quietly();
            moved = true;
        }

        let split = if moved {
            layout::map_split(area, self.data.dashboard_layout.map_fraction)
        } else {
            split
        };
        Self::paint_divider(ui, split.divider, response.hovered() || response.dragged());
        split
    }

    /// The tabs on the left and Umbra on the right, under the chain map and down to
    /// the bottom of the window; stacked when the window is too narrow for both.
    fn dashboard_panes(&mut self, ui: &mut egui::Ui, mut area: egui::Rect) {
        area.set_height(area.height().max(MIN_DASHBOARD_PANES_HEIGHT));
        let mut panes = layout::dashboard_panes(area, self.data.dashboard_layout.tabs_fraction);
        if let Some(divider) = panes.divider {
            let response = Self::divider_response(ui, "dashboard_divider", divider);
            if self.drag_dashboard_divider(&response, area) {
                panes = layout::dashboard_panes(area, self.data.dashboard_layout.tabs_fraction);
            }
            if let Some(divider) = panes.divider {
                Self::paint_divider(ui, divider, response.hovered() || response.dragged());
            }
        }

        let mut tabs_ui = Self::pane_ui(ui, "dashboard_tabs_pane", panes.tabs);
        self.dashboard_tabs(&mut tabs_ui);
        tabs_ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("dashboard_tab_content")
            .auto_shrink([false, false])
            .show(&mut tabs_ui, |ui| match self.active_tab {
                DashboardTab::Movement => self.recent_movements(ui),
                DashboardTab::Connections => self.connections_tab(ui),
                DashboardTab::Systems => self.systems_overview(ui),
                DashboardTab::Intel => self.selected_system_intel(ui),
                DashboardTab::Chain => self.chain_management(ui),
            });

        let mut umbra_ui = Self::pane_ui(ui, "umbra_pane", panes.umbra);
        self.umbra_panel(&mut umbra_ui);

        ui.advance_cursor_after_rect(area);
    }

    /// A child UI confined to one pane, so nothing it draws spills into the other.
    fn pane_ui(ui: &mut egui::Ui, id: &str, rect: egui::Rect) -> egui::Ui {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(id)
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        child
    }

    /// Follows a drag on the divider between the tabs and Umbra; a double-click puts it
    /// back, and the new split is saved when the player lets go. Returns whether it moved.
    fn drag_dashboard_divider(&mut self, response: &egui::Response, area: egui::Rect) -> bool {
        let mut moved = false;
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let fraction = layout::fraction_at(pointer.x, area);
            if fraction != self.data.dashboard_layout.tabs_fraction {
                self.data.dashboard_layout.tabs_fraction = fraction;
                moved = true;
            }
        }
        if response.drag_stopped() {
            self.save_quietly();
        }
        if response.double_clicked() {
            self.data.dashboard_layout.tabs_fraction = DashboardLayout::DEFAULT_TABS_FRACTION;
            self.save_quietly();
            moved = true;
        }
        moved
    }

    /// A divider the player drags to resize the panes either side of it, showing the
    /// resize cursor while hovered or held, even once the pointer slips off it.
    fn divider_response(ui: &egui::Ui, id: &str, divider: egui::Rect) -> egui::Response {
        let cursor = if divider.height() >= divider.width() {
            egui::CursorIcon::ResizeHorizontal
        } else {
            egui::CursorIcon::ResizeVertical
        };
        let response = ui
            .interact(divider, ui.id().with(id), Sense::click_and_drag())
            .on_hover_cursor(cursor);
        if response.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }
        response
    }

    /// A divider's line with its grip in the middle, lit while hovered or held.
    fn paint_divider(ui: &egui::Ui, divider: egui::Rect, active: bool) {
        let color = if active {
            theme::accent()
        } else {
            theme::panel_stroke()
        };
        let painter = ui.painter();
        let center = divider.center();
        let (line, grip_size) = if divider.height() >= divider.width() {
            (
                [
                    egui::pos2(center.x, divider.top() + 4.0),
                    egui::pos2(center.x, divider.bottom() - 4.0),
                ],
                Vec2::new(8.0, 26.0),
            )
        } else {
            (
                [
                    egui::pos2(divider.left() + 4.0, center.y),
                    egui::pos2(divider.right() - 4.0, center.y),
                ],
                Vec2::new(26.0, 8.0),
            )
        };
        painter.line_segment(line, Stroke::new(if active { 2.0_f32 } else { 1.0 }, color));
        let grip = egui::Rect::from_center_size(center, grip_size);
        painter.rect_filled(grip, 4.0, theme::panel_background());
        painter.rect_stroke(
            grip,
            4.0,
            Stroke::new(1.0_f32, color),
            egui::StrokeKind::Inside,
        );
        for dot in layout::grip_dots(divider) {
            painter.circle_filled(dot, 1.5, color);
        }
    }

    fn selected_system_sidebar(&mut self, ui: &mut egui::Ui) {
        self.reconcile_selected_system();
        ui.horizontal(|ui| {
            ui.heading("Selected System");
        });

        let Some(system) = self.selected_system() else {
            record_frame(ui, |ui| {
                ui.label("No system selected.");
                ui.label("Sync a character location or select a system on the chain map.");
            });
            return;
        };

        let system_node = system.clone();
        let system_id = system.id.clone();
        let system_name = system.name.clone();
        let system_class = Self::inferred_system_class(system);
        let raw_system_class = system.class.clone();
        let system_status = system.status.clone();
        let system_notes = system.notes.clone();
        let security_status = system.security_status;
        let constellation_name = system.constellation_name.clone();
        let region_name = system.region_name.clone();
        let planet_count = system.planet_count;
        let stargate_count = system.stargate_count;
        let station_count = system.station_count;
        let traffic_line = Self::recent_traffic(
            system,
            self.refreshed_traffic.get(&system.solar_system_id),
            now(),
        )
        .map(|traffic| Self::recent_traffic_label(&traffic));
        let umbra_words = self
            .system_assessments
            .get(&system_id)
            .and_then(umbra::sidebar_words)
            .map(str::to_owned);
        let assessment_summary = self.system_assessments.get(&system_id).map(|assessment| {
            let (age, stale) = Self::assessment_age(assessment.generated_at, now());
            (
                format!(
                    "Threat {} · Activity {} · {}",
                    local_intel::score_label(assessment.threat_score),
                    local_intel::score_label(assessment.activity_score),
                    age.to_lowercase()
                ),
                stale,
            )
        });
        let pilots: Vec<String> = self
            .data
            .accounts
            .iter()
            .filter(|account| account.last_system_id == system_id)
            .map(|account| account.character_name.clone())
            .collect();
        let connection_count = self
            .data
            .connections
            .iter()
            .filter(|connection| {
                connection.from_system == system_id || connection.to_system == system_id
            })
            .count();
        let visit_count = self
            .data
            .visits
            .iter()
            .filter(|visit| visit.system_id == system_id)
            .count();

        record_frame(ui, |ui| {
            ui.heading(egui::RichText::new(system_name).color(theme::system_text()));
            ui.horizontal_wrapped(|ui| {
                pill(ui, &system_class);
                if raw_system_class != system_class
                    && !raw_system_class.is_empty()
                    && raw_system_class != "unknown"
                {
                    ui.label(egui::RichText::new(raw_system_class).color(theme::muted_text()));
                }
                pill(ui, &system_status);
                ui.label(format!("sec {:.1}", security_status));
            });
            if !region_name.is_empty() || !constellation_name.is_empty() {
                ui.label(format!(
                    "{} / {}",
                    if region_name.is_empty() {
                        "Unknown region"
                    } else {
                        &region_name
                    },
                    if constellation_name.is_empty() {
                        "Unknown constellation"
                    } else {
                        &constellation_name
                    }
                ));
            }
            ui.label(format!(
                "{} planets · {} gates · {} stations",
                planet_count, stargate_count, station_count
            ));
            if let Some(traffic_line) = &traffic_line {
                ui.label(traffic_line);
            }
            if let Some((summary, stale)) = &assessment_summary {
                ui.label(egui::RichText::new(summary).color(if *stale {
                    theme::muted_text()
                } else {
                    theme::accent()
                }));
            }
            if let Some(words) = &umbra_words {
                umbra::umbra_quote(ui, words);
            }
            self.intel_activity(ui, &system_node, true);
            ui.label(format!(
                "{} connections · {} visits",
                connection_count, visit_count
            ));
            if pilots.is_empty() {
                ui.label("No characters currently here.");
            } else {
                ui.label("Characters here:");
                for pilot in pilots {
                    ui.label(egui::RichText::new(pilot).color(theme::character_text()));
                }
            }
            if !system_notes.is_empty() {
                ui.separator();
                ui.label(system_notes);
            }
            ui.separator();
            if ui.button("Delete system from chain").clicked() {
                self.delete_system(&system_id);
            }
        });
        record_frame(ui, |ui| self.signatures_section(ui, &system_id));
        record_frame(ui, |ui| self.live_intel_section(ui, &system_id));
    }

    /// What is under way for a system's intel: its killboard refresh and, with
    /// `with_umbra`, Umbra's briefing.
    fn intel_activity(&self, ui: &mut egui::Ui, system: &SystemNode, with_umbra: bool) {
        let solar_system_id = system.solar_system_id;
        if solar_system_id != 0 {
            if self.intel_queue.in_flight_system() == Some(solar_system_id) {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Gathering killboard intel...");
                });
            } else if let Some(ahead) = self.intel_queue.place(solar_system_id) {
                let text = match ahead {
                    0 => "Killboard intel is next in line".to_owned(),
                    ahead => format!("Killboard intel queued · {ahead} ahead"),
                };
                ui.label(egui::RichText::new(text).color(theme::muted_text()));
            } else if let Some(error) = self.intel_errors.get(&solar_system_id) {
                ui.label(
                    egui::RichText::new(format!("Intel could not be gathered: {error}"))
                        .color(WARNING_TEXT),
                );
            }
        }
        if !with_umbra {
            return;
        }
        match &self.umbra_task {
            Some(task) if task.briefing_system() == Some(system.id.as_str()) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(task.label());
                });
            }
            _ if self.auto_briefings.is_pending(&system.id) => {
                ui.label(
                    egui::RichText::new("Umbra will brief this system as soon as she is free.")
                        .color(theme::muted_text()),
                );
            }
            _ => {}
        }
    }

    fn characters_sidebar(&mut self, ui: &mut egui::Ui) {
        let mut authorize_id = None;
        let mut sign_in_for_online_id = None;
        let mut sync_id = None;
        let mut delete_id = None;
        let mut recon_change = None;

        ui.horizontal(|ui| {
            ui.heading("Characters");
            if ui
                .add_enabled(!self.esi_busy, egui::Button::new("Add"))
                .clicked()
            {
                self.start_add_character();
            }
        });

        if self.data.accounts.is_empty() {
            record_frame(ui, |ui| {
                ui.label("No characters yet.");
                ui.label("Add a character to track current location.");
            });
        }

        for account in &self.data.accounts {
            record_frame(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(texture) = self.portrait_textures.get(&account.id) {
                        ui.add(
                            egui::Image::new(texture)
                                .fit_to_exact_size(Vec2::splat(56.0))
                                .corner_radius(6.0),
                        );
                    } else {
                        ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
                    }

                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.heading(
                                egui::RichText::new(&account.character_name)
                                    .color(theme::character_text()),
                            );
                            pill(ui, &account.auth_status);
                        });
                        ui.horizontal_wrapped(|ui| {
                            let mut is_recon = account.recon;
                            if ui
                                .add_enabled(
                                    recon::can_mark_recon(&self.data.accounts, &account.id),
                                    egui::Checkbox::new(&mut is_recon, "Recon"),
                                )
                                .on_hover_text(RECON_HINT)
                                .on_disabled_hover_text(format!(
                                    "Up to {MAX_RECON_CHARACTERS} recon pilots at a time. Stand one down to make room."
                                ))
                                .changed()
                            {
                                recon_change = Some((account.id.clone(), is_recon));
                            }
                            if self.online_indicator(ui, account) {
                                sign_in_for_online_id = Some(account.id.clone());
                            }
                        });
                        if !account.last_system_name.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new("Current:").color(theme::muted_text()),
                                );
                                ui.label(
                                    egui::RichText::new(&account.last_system_name)
                                        .color(theme::system_text()),
                                );
                            });
                        }
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!self.esi_busy, egui::Button::new("Reconnect"))
                                .clicked()
                            {
                                authorize_id = Some(account.id.clone());
                            }
                            if ui
                                .add_enabled(
                                    !self.esi_busy && account.auth_status == "authorized",
                                    egui::Button::new("Update location"),
                                )
                                .clicked()
                            {
                                sync_id = Some(account.id.clone());
                            }
                            if ui.small_button("Remove").clicked() {
                                delete_id = Some(account.id.clone());
                            }
                        });
                    });
                });
            });
        }

        if let Some((id, is_recon)) = recon_change
            && recon::set_recon(&mut self.data.accounts, &id, is_recon)
        {
            self.save();
        }
        if let Some(id) = delete_id {
            self.data.accounts.retain(|account| account.id != id);
            self.online_polls.forget(&id);
            self.reconcile_selected_system();
            self.save();
        }
        if let Some(id) = sign_in_for_online_id {
            self.start_authorize_account(id, true);
        }
        if let Some(id) = authorize_id {
            self.start_authorize_account(id, false);
        }
        if let Some(id) = sync_id {
            self.start_sync_location(id);
        }
    }

    /// Shows whether the character is logged in, or how to let Nomadeer tell. Returns
    /// whether the player asked to sign in again for it.
    fn online_indicator(&self, ui: &mut egui::Ui, account: &Account) -> bool {
        match recon::online_scope(account) {
            Some(true) => {
                let (text, color) = match self.online_polls.online(&account.id) {
                    Some(true) => ("Online", theme::online()),
                    Some(false) => ("Offline", theme::muted_text()),
                    None => return false,
                };
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, color);
                ui.label(egui::RichText::new(text).color(color));
                false
            }
            Some(false) if account.auth_status == "authorized" => ui
                .add_enabled(
                    !self.esi_busy,
                    egui::Button::new("Show online status").small(),
                )
                .on_hover_text("Sign in again so Nomadeer can see when this pilot is logged in.")
                .clicked(),
            _ => false,
        }
    }

    fn dashboard_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(self.active_tab == DashboardTab::Movement, "Movement")
                .clicked()
            {
                self.active_tab = DashboardTab::Movement;
            }
            if ui
                .selectable_label(self.active_tab == DashboardTab::Connections, "Connections")
                .clicked()
            {
                self.active_tab = DashboardTab::Connections;
            }
            if ui
                .selectable_label(self.active_tab == DashboardTab::Systems, "Systems")
                .clicked()
            {
                self.active_tab = DashboardTab::Systems;
            }
            if ui
                .selectable_label(self.active_tab == DashboardTab::Intel, "Intel")
                .clicked()
            {
                self.active_tab = DashboardTab::Intel;
            }
            if ui
                .selectable_label(self.active_tab == DashboardTab::Chain, "Chain")
                .clicked()
            {
                self.active_tab = DashboardTab::Chain;
            }
        });
    }

    fn selected_system_intel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Intel");
            let mut gather = self.data.auto_intel.gather_on_arrival;
            if ui
                .checkbox(&mut gather, "Gather intel on arrival")
                .on_hover_text(
                    "Refresh killboard intel when a recon pilot enters a system without fresh intel.",
                )
                .changed()
            {
                self.data.auto_intel.gather_on_arrival = gather;
                if !gather {
                    self.intel_queue.drop_arrivals();
                }
                self.save();
            }
        });
        let Some(system) = self.selected_system().cloned() else {
            ui.label("Select a system on the chain map first.");
            return;
        };

        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(&system.name).color(theme::system_text()));
            pill(ui, &Self::inferred_system_class(&system));
            if system.solar_system_id != 0 {
                if ui.button("Open zKillboard").clicked() {
                    self.open_external_url(&format!(
                        "https://zkillboard.com/system/{}/",
                        system.solar_system_id
                    ));
                }
                if ui.button("Open Anoik.is").clicked() {
                    self.open_external_url(&format!("https://anoik.is/systems/{}", system.name));
                }
            }
            if ui
                .add_enabled(
                    self.can_refresh_intel(&system),
                    egui::Button::new("Refresh zKill"),
                )
                .clicked()
            {
                self.start_zkill_refresh();
            }
        });
        self.intel_activity(ui, &system, false);

        // Saved assessments outlive the session; the kill list below does not.
        let assessment = self.system_assessments.get(&system.id).cloned();
        match &assessment {
            Some(assessment) => self.assessment_panel(ui, &system, assessment),
            None => {
                ui.label("No assessment for this system yet. Refresh zKill to score its threat and activity.");
                self.umbra_feedback(ui, Some(&system.id));
            }
        }

        let Some(report) = self.zkill_reports.get(&system.solar_system_id) else {
            ui.label(if assessment.is_some() {
                "Recent kills have not been loaded this session. Refresh zKill to see them."
            } else {
                "No zKillboard data loaded yet. Refresh to pull recent kills for this system."
            });
            return;
        };
        let recent_kills = report.recent_kills.clone();
        let total_value = report.total_value;
        let solo_kills = report.solo_kills;
        let npc_kills = report.npc_kills;
        let history_kills = Self::killboard_kill_count(report);

        ui.label(format!(
            "Latest {} shown · {history_kills} kills in 7d · {} shown value · {} solo · {} NPC",
            recent_kills.len(),
            Self::format_isk(total_value),
            solo_kills,
            npc_kills
        ));

        if recent_kills.is_empty() {
            ui.label("No recent kills returned by zKillboard.");
            return;
        }

        for kill in &recent_kills {
            record_frame(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let (relative_time, time_color) = Self::relative_kill_time(&kill.killmail_time);
                    ui.label(egui::RichText::new(relative_time).color(time_color));
                    if ui.button("View details").clicked() {
                        self.selected_zkill_kill = Some(kill.clone());
                    }
                    if ui.button("Open in browser").clicked() {
                        self.open_external_url(&format!(
                            "https://zkillboard.com/kill/{}/",
                            kill.killmail_id
                        ));
                    }
                    if kill.solo {
                        pill(ui, "solo");
                    }
                    if kill.npc {
                        pill(ui, "NPC");
                    }
                });
                let victim_name = kill
                    .victim_character_name
                    .as_deref()
                    .unwrap_or("Unknown victim");
                let ship_name = kill
                    .victim_ship_type_name
                    .clone()
                    .unwrap_or_else(|| "Unknown ship".to_owned());
                ui.label(format!(
                    "{victim_name} lost {ship_name} · {} attackers · {}",
                    kill.attacker_count,
                    Self::format_isk(kill.total_value)
                ));
                let corporation = kill.victim_corporation_name.as_deref();
                let alliance = kill.victim_alliance_name.as_deref();
                match (corporation, alliance) {
                    (Some(corporation), Some(alliance)) => {
                        ui.label(format!("{corporation} · {alliance}"));
                    }
                    (Some(corporation), None) => {
                        ui.label(corporation);
                    }
                    (None, Some(alliance)) => {
                        ui.label(alliance);
                    }
                    (None, None) => {}
                }
            });
        }
    }

    fn assessment_panel(
        &mut self,
        ui: &mut egui::Ui,
        system: &SystemNode,
        assessment: &SystemAssessment,
    ) {
        let (age, stale) = Self::assessment_age(assessment.generated_at, now());
        // An old assessment still shows, but muted, so it never reads as a live warning.
        let score_color = if stale {
            theme::muted_text()
        } else {
            theme::accent()
        };
        record_frame(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("System assessment");
                for text in [
                    format!(
                        "Threat {} · {}",
                        assessment.threat_score,
                        local_intel::score_label(assessment.threat_score)
                    ),
                    format!(
                        "Activity {} · {}",
                        assessment.activity_score,
                        local_intel::score_label(assessment.activity_score)
                    ),
                    local_intel::confidence_label(&assessment.confidence),
                ] {
                    ui.label(egui::RichText::new(text).color(score_color));
                }
                ui.label(egui::RichText::new(age).color(if stale {
                    WARNING_TEXT
                } else {
                    theme::muted_text()
                }));
            });
            if stale {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(
                            "Over an hour old: the last-hour figures no longer describe the system now.",
                        )
                        .color(WARNING_TEXT),
                    );
                    if ui
                        .add_enabled(
                            self.can_refresh_intel(system),
                            egui::Button::new("Refresh now"),
                        )
                        .clicked()
                    {
                        self.start_zkill_refresh();
                    }
                });
            }
            self.assessment_briefing(ui, system, assessment);
        });
    }

    /// "Assessed 3h ago", and whether that is too old to describe the system now.
    fn assessment_age(generated_at: u64, current_time: u64) -> (String, bool) {
        let age = current_time.saturating_sub(generated_at);
        (
            format!("Assessed {}", Self::format_duration_ago(age)),
            auto_intel::assessment_is_stale(generated_at, current_time),
        )
    }

    /// A system can be refreshed unless it has no EVE system or is being fetched now.
    fn can_refresh_intel(&self, system: &SystemNode) -> bool {
        system.solar_system_id != 0
            && self.intel_queue.in_flight_system() != Some(system.solar_system_id)
    }

    fn zkill_details_window(&mut self, ctx: &egui::Context) {
        let Some(kill) = self.selected_zkill_kill.clone() else {
            return;
        };

        let mut open = true;
        egui::Window::new(format!("Kill {}", kill.killmail_id))
            .open(&mut open)
            .resizable(true)
            .default_width(620.0)
            .default_height(680.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let (relative_time, time_color) =
                            Self::relative_kill_time(&kill.killmail_time);
                        ui.label(egui::RichText::new(relative_time).color(time_color));
                        ui.label(
                            egui::RichText::new(&kill.killmail_time).color(theme::muted_text()),
                        );
                        if kill.solo {
                            pill(ui, "solo");
                        }
                        if kill.npc {
                            pill(ui, "NPC");
                        }
                        if kill.awox {
                            pill(ui, "AWOX");
                        }
                    });

                    if ui.button("Open in browser").clicked() {
                        self.open_external_url(&format!(
                            "https://zkillboard.com/kill/{}/",
                            kill.killmail_id
                        ));
                    }

                    ui.separator();
                    ui.heading("Loss");
                    let victim_name = kill
                        .victim_character_name
                        .as_deref()
                        .unwrap_or("Unknown victim");
                    let ship_name = kill
                        .victim_ship_type_name
                        .as_deref()
                        .unwrap_or("Unknown ship");
                    ui.label(
                        egui::RichText::new(format!("{victim_name} · {ship_name}"))
                            .color(theme::system_text())
                            .strong(),
                    );
                    Self::zkill_affiliation(
                        ui,
                        kill.victim_corporation_name.as_deref(),
                        kill.victim_alliance_name.as_deref(),
                    );
                    ui.label(format!(
                        "{} damage taken · {} points",
                        kill.victim_damage_taken, kill.points
                    ));

                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        pill(ui, &format!("{} total", Self::format_isk(kill.total_value)));
                        pill(
                            ui,
                            &format!("{} fitted", Self::format_isk(kill.fitted_value)),
                        );
                        pill(
                            ui,
                            &format!("{} destroyed", Self::format_isk(kill.destroyed_value)),
                        );
                        pill(
                            ui,
                            &format!("{} dropped", Self::format_isk(kill.dropped_value)),
                        );
                    });

                    ui.separator();
                    egui::CollapsingHeader::new(format!("Attackers ({})", kill.attackers.len()))
                        .default_open(true)
                        .show(ui, |ui| {
                            let mut attackers = kill.attackers.clone();
                            attackers.sort_by_key(|attacker| {
                                (
                                    std::cmp::Reverse(attacker.final_blow),
                                    std::cmp::Reverse(attacker.damage_done),
                                )
                            });
                            for attacker in attackers {
                                record_frame(ui, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        let attacker_name = attacker
                                            .character_name
                                            .as_deref()
                                            .or(attacker.faction_name.as_deref())
                                            .unwrap_or("NPC / unknown attacker");
                                        ui.label(
                                            egui::RichText::new(attacker_name)
                                                .color(theme::character_text())
                                                .strong(),
                                        );
                                        if attacker.final_blow {
                                            pill(ui, "final blow");
                                        }
                                        ui.label(format!("sec {:.1}", attacker.security_status));
                                    });
                                    Self::zkill_affiliation(
                                        ui,
                                        attacker.corporation_name.as_deref(),
                                        attacker.alliance_name.as_deref(),
                                    );
                                    let ship = attacker
                                        .ship_type_name
                                        .as_deref()
                                        .unwrap_or("Unknown ship");
                                    let weapon = attacker
                                        .weapon_type_name
                                        .as_deref()
                                        .unwrap_or("Unknown weapon");
                                    ui.label(format!(
                                        "{ship} · {weapon} · {} damage",
                                        attacker.damage_done
                                    ));
                                });
                            }
                        });

                    ui.separator();
                    egui::CollapsingHeader::new(format!(
                        "Fitting and cargo ({})",
                        kill.items.len()
                    ))
                    .default_open(false)
                    .show(ui, |ui| {
                        if kill.items.is_empty() {
                            ui.label("No item details were included in this killmail.");
                        }
                        for item in &kill.items {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(&item.type_name);
                                if item.quantity_destroyed > 0 {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} destroyed",
                                            item.quantity_destroyed
                                        ))
                                        .color(Color32::from_rgb(255, 91, 112)),
                                    );
                                }
                                if item.quantity_dropped > 0 {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} dropped",
                                            item.quantity_dropped
                                        ))
                                        .color(Color32::from_rgb(126, 231, 135)),
                                    );
                                }
                            });
                        }
                    });
                });
            });

        if !open {
            self.selected_zkill_kill = None;
        }
    }

    fn zkill_affiliation(ui: &mut egui::Ui, corporation: Option<&str>, alliance: Option<&str>) {
        match (corporation, alliance) {
            (Some(corporation), Some(alliance)) => {
                ui.label(format!("{corporation} · {alliance}"));
            }
            (Some(corporation), None) => {
                ui.label(corporation);
            }
            (None, Some(alliance)) => {
                ui.label(alliance);
            }
            (None, None) => {}
        }
    }

    fn relative_kill_time(killmail_time: &str) -> (String, Color32) {
        let Some(kill_time) = Self::parse_zkill_time(killmail_time) else {
            return ("Unknown time".to_owned(), theme::muted_text());
        };
        let current_time = now();
        let age_seconds = current_time.saturating_sub(kill_time);
        let label = Self::format_duration_ago(age_seconds);

        let color = if age_seconds <= 4 * 60 * 60 {
            Color32::from_rgb(255, 91, 112)
        } else if age_seconds <= 8 * 60 * 60 {
            Color32::from_rgb(255, 194, 87)
        } else if age_seconds <= 24 * 60 * 60 {
            Color32::from_rgb(126, 231, 135)
        } else {
            theme::muted_text()
        };

        (label, color)
    }

    fn format_duration_ago(seconds: u64) -> String {
        if seconds < 60 {
            "just now".to_owned()
        } else if seconds < 60 * 60 {
            format!("{}m ago", seconds / 60)
        } else if seconds < 24 * 60 * 60 {
            let hours = seconds / (60 * 60);
            let minutes = (seconds % (60 * 60)) / 60;
            if minutes == 0 {
                format!("{hours}h ago")
            } else {
                format!("{hours}h {minutes}m ago")
            }
        } else {
            format!("{}d ago", seconds / (24 * 60 * 60))
        }
    }

    fn parse_zkill_time(value: &str) -> Option<u64> {
        if value.len() < 20 {
            return None;
        }

        let year = value.get(0..4)?.parse::<i64>().ok()?;
        let month = value.get(5..7)?.parse::<i64>().ok()?;
        let day = value.get(8..10)?.parse::<i64>().ok()?;
        let hour = value.get(11..13)?.parse::<i64>().ok()?;
        let minute = value.get(14..16)?.parse::<i64>().ok()?;
        let second = value.get(17..19)?.parse::<i64>().ok()?;

        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || !(0..=23).contains(&hour)
            || !(0..=59).contains(&minute)
            || !(0..=60).contains(&second)
        {
            return None;
        }

        let days = Self::days_from_civil(year, month, day);
        let timestamp = days * 86_400 + hour * 3_600 + minute * 60 + second;
        u64::try_from(timestamp).ok()
    }

    fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
        let year = year - i64::from(month <= 2);
        let era = if year >= 0 { year } else { year - 399 } / 400;
        let year_of_era = year - era * 400;
        let month_prime = month + if month > 2 { -3 } else { 9 };
        let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    fn format_isk(value: f64) -> String {
        if value >= 1_000_000_000.0 {
            format!("{:.1}b ISK", value / 1_000_000_000.0)
        } else if value >= 1_000_000.0 {
            format!("{:.1}m ISK", value / 1_000_000.0)
        } else if value >= 1_000.0 {
            format!("{:.1}k ISK", value / 1_000.0)
        } else {
            format!("{value:.0} ISK")
        }
    }

    fn systems_overview(&mut self, ui: &mut egui::Ui) {
        ui.heading("Systems");
        if self.data.systems.is_empty() {
            ui.label("No systems yet.");
            return;
        }

        let mut delete_id = None;
        let systems = self.data.systems.clone();
        for system in systems {
            let is_selected = system.id == self.selected_system_id;
            let jumps = Self::recent_traffic(
                &system,
                self.refreshed_traffic.get(&system.solar_system_id),
                now(),
            )
            .and_then(|traffic| {
                traffic.jumps.map(|jumps| {
                    format!(
                        " · {jumps} jumps in the hour to {} EVE time",
                        local_intel::eve_clock(traffic.data_time)
                    )
                })
            })
            .unwrap_or_default();
            record_frame(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(is_selected, &system.name).clicked() {
                        self.selected_system_id = system.id.clone();
                    }
                    pill(ui, &Self::inferred_system_class(&system));
                    pill(ui, &system.status);
                    if !system.region_name.is_empty() {
                        ui.label(&system.region_name);
                    }
                });
                ui.label(format!(
                    "sec {:.1} · {} planets · {} gates{jumps}",
                    system.security_status, system.planet_count, system.stargate_count,
                ));
                if !system.notes.is_empty() {
                    ui.label(&system.notes);
                }
                if ui.small_button("Delete system").clicked() {
                    delete_id = Some(system.id.clone());
                }
            });
        }

        if let Some(id) = delete_id {
            self.delete_system(&id);
        }
    }

    fn chain_management(&mut self, ui: &mut egui::Ui) {
        ui.heading("Chain");
        ui.label(format!(
            "Current chain: {} systems · {} connections · {} visits",
            self.data.systems.len(),
            self.data.connections.len(),
            self.data.visits.len()
        ));
        ui.label(format!(
            "Location polling: every {} seconds per active check",
            AUTO_SYNC_INTERVAL.as_secs()
        ));

        record_frame(ui, |ui| {
            ui.heading("Save current chain");
            ui.horizontal_wrapped(|ui| {
                ui.label("Name");
                ui.add(
                    egui::TextEdit::singleline(&mut self.chain_snapshot_name)
                        .desired_width(ui.available_width().min(220.0)),
                );
                if ui.button("Save chain").clicked() {
                    self.save_chain_snapshot();
                }
            });
            ui.label("Saved chains keep systems, connections, notes, signatures, and movement history. Characters stay separate.");
        });

        record_frame(ui, |ui| {
            ui.heading("Current chain controls");
            ui.horizontal_wrapped(|ui| {
                if ui.button("Refresh saved list").clicked() {
                    self.refresh_chain_snapshots();
                }
                if ui.button("Clear current chain").clicked() {
                    self.clear_chain();
                }
            });
            ui.label("Use clear when a wormhole chain is done or rolled out. Save first if you may want it later.");
        });

        ui.heading("Saved chains");
        if self.chain_snapshots.is_empty() {
            ui.label("No saved chains yet.");
            return;
        }

        let snapshots = self.chain_snapshots.clone();
        let mut load_file = None;
        let mut delete_file = None;
        for snapshot in snapshots {
            record_frame(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(&snapshot.name).color(theme::system_text()));
                    pill(ui, &format!("{} systems", snapshot.system_count));
                    pill(ui, &format!("{} connections", snapshot.connection_count));
                });
                ui.label(
                    egui::RichText::new(format!("Saved {}", snapshot.created_at))
                        .color(theme::muted_text()),
                );
                ui.horizontal(|ui| {
                    if ui.button("Load chain").clicked() {
                        load_file = Some(snapshot.file_name.clone());
                    }
                    if ui.small_button("Delete saved chain").clicked() {
                        delete_file = Some(snapshot.file_name.clone());
                    }
                });
            });
        }

        if let Some(file_name) = load_file {
            self.load_chain_snapshot(&file_name);
        }
        if let Some(file_name) = delete_file {
            self.delete_chain_snapshot(&file_name);
        }
    }

    fn recent_movements(&self, ui: &mut egui::Ui) {
        ui.heading("Recent movement");
        let mut visits: Vec<&VisitLog> = self.data.visits.iter().collect();
        visits.sort_by_key(|visit| std::cmp::Reverse(visit.created_at));

        if visits.is_empty() {
            ui.label("No movement history yet.");
            return;
        }

        for visit in visits.into_iter().take(8) {
            record_frame(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(&visit.pilot).color(theme::character_text()));
                    ui.label(egui::RichText::new("entered").color(theme::muted_text()));
                    ui.label(
                        egui::RichText::new(self.system_name(&visit.system_id))
                            .color(theme::system_text()),
                    );
                });
                if !visit.activity.is_empty() {
                    ui.label(&visit.activity);
                }
            });
        }
    }
}

impl eframe::App for NomadeerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_esi_tasks();
        self.poll_zkill_tasks();
        self.poll_traffic_tasks();
        self.poll_portrait_tasks(ctx);
        self.poll_local_intel_tasks();
        self.poll_live_tasks();
        self.maybe_request_portraits();
        self.maybe_poll_online();
        self.maybe_auto_sync();
        self.maybe_poll_traffic();
        self.maybe_check_umbra_core();
        self.maybe_start_intel();
        self.maybe_expire_live_intel();
        self.maybe_start_auto_briefing();
        self.poll_update_check();
        self.maybe_check_for_updates();
        self.handle_paste(ctx);
        self.top_bar(ctx);
        self.update_notice_bar(ctx);
        self.settings_window(ctx);
        self.update_window(ctx);
        self.local_intel_settings_window(ctx);
        self.side_panel(ctx);
        self.central_panel(ctx);
        self.zkill_details_window(ctx);
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::{NomadeerApp, RecentTraffic, UmbraTask};
    use crate::esi::SystemIntel;
    use crate::model::{KillCounts, SystemNode, SystemTrafficHistory, TrafficReading};

    fn intel() -> SystemIntel {
        SystemIntel {
            solar_system_id: 30_002_187,
            name: "Amamake".to_owned(),
            security_class: "C".to_owned(),
            security_status: 0.4,
            constellation_id: 20_000_320,
            constellation_name: "Hed".to_owned(),
            region_id: 10_000_042,
            region_name: "Metropolis".to_owned(),
            planet_count: 10,
            stargate_count: 5,
            station_count: 3,
            jumps: Some(TrafficReading {
                hour: 7_200,
                data_time: 7_300,
                value: 400,
            }),
            kills: Some(TrafficReading {
                hour: 7_200,
                data_time: 7_500,
                value: KillCounts {
                    ship_kills: 5,
                    pod_kills: 2,
                    npc_kills: 30,
                },
            }),
        }
    }

    fn node() -> SystemNode {
        SystemNode {
            id: "esi_sys_30002187".to_owned(),
            solar_system_id: 0,
            name: String::new(),
            class: String::new(),
            security_status: 0.0,
            constellation_id: 0,
            constellation_name: String::new(),
            region_id: 0,
            region_name: String::new(),
            planet_count: 0,
            stargate_count: 0,
            station_count: 0,
            jumps_1h: 0,
            ship_kills_1h: 0,
            pod_kills_1h: 0,
            npc_kills_1h: 0,
            intel_updated_at: 0,
            effect: String::new(),
            status: "scouted".to_owned(),
            owner: String::new(),
            notes: String::new(),
            created_at: 1,
            map_position: None,
        }
    }

    #[test]
    fn system_intel_uses_esi_data_time_and_reports_changes() {
        let mut system = node();
        assert!(NomadeerApp::apply_system_intel(&mut system, &intel()));
        assert_eq!(system.jumps_1h, 400);
        assert_eq!(system.ship_kills_1h, 5);
        assert_eq!(system.intel_updated_at, 7_300);
        assert!(!NomadeerApp::apply_system_intel(&mut system, &intel()));
    }

    #[test]
    fn assessments_show_their_age_and_go_stale_after_an_hour() {
        assert_eq!(
            NomadeerApp::assessment_age(1_000, 1_030),
            ("Assessed just now".to_owned(), false)
        );
        assert_eq!(
            NomadeerApp::assessment_age(1_000, 1_000 + 45 * 60),
            ("Assessed 45m ago".to_owned(), false)
        );
        assert_eq!(
            NomadeerApp::assessment_age(1_000, 1_000 + 3 * 3_600),
            ("Assessed 3h ago".to_owned(), true)
        );
        // A clock that moved backwards never makes an assessment look fresher than new.
        assert_eq!(
            NomadeerApp::assessment_age(5_000, 1_000),
            ("Assessed just now".to_owned(), false)
        );
    }

    #[test]
    fn failed_activity_feed_keeps_previous_values() {
        let mut system = node();
        NomadeerApp::apply_system_intel(&mut system, &intel());

        let mut newer = intel();
        newer.jumps = None;
        newer.kills = Some(TrafficReading {
            hour: 10_800,
            data_time: 11_000,
            value: KillCounts::default(),
        });
        assert!(NomadeerApp::apply_system_intel(&mut system, &newer));
        assert_eq!(system.jumps_1h, 400);
        assert_eq!(system.ship_kills_1h, 0);
        // The node still holds older jump data, so it is only as fresh as that.
        assert_eq!(system.intel_updated_at, 7_300);
    }

    fn located(solar_system_id: u64, name: &str, security_status: f32, region: &str) -> SystemNode {
        SystemNode {
            solar_system_id,
            name: name.to_owned(),
            security_status,
            region_id: 1,
            region_name: region.to_owned(),
            ..node()
        }
    }

    #[test]
    fn class_pill_follows_the_system_id_not_the_name() {
        let class = |system: &SystemNode| NomadeerApp::inferred_system_class(system);
        assert_eq!(class(&located(30_000_142, "Jita", 0.95, "The Forge")), "HS");
        assert_eq!(
            class(&located(30_002_187, "Amamake", 0.4, "Heimatar")),
            "LS"
        );
        assert_eq!(
            class(&located(30_001_198, "J-GAMP", -0.1, "Fountain")),
            "NS"
        );
        assert_eq!(
            class(&located(31_000_005, "Thera", -0.99, "G-R00031")),
            "Thera"
        );
        assert_eq!(
            class(&located(31_002_238, "J123456", -0.99, "C-R00023")),
            "C3"
        );
        assert_eq!(class(&located(31_002_238, "J123456", -0.99, "")), "WH");
        // J135422 as ESI reports it: region B-R00008, constellation B-C00064.
        assert_eq!(
            class(&located(31_000_792, "J135422", -0.99, "B-R00008")),
            "C2"
        );
        let mut constellation_only = located(31_000_792, "J135422", -0.99, "");
        constellation_only.constellation_name = "B-C00064".to_owned();
        assert_eq!(class(&constellation_only), "C2");
        assert_eq!(
            class(&located(31_000_001, "Sentinel MZ", -0.99, "K-R00033")),
            "C14"
        );
        assert_eq!(
            class(&located(31_000_006, "Azdaja Redoubt", -0.99, "")),
            "C18"
        );
        assert_eq!(
            class(&located(31_000_009, "Unlisted", -0.99, "K-R00033")),
            "Drifter"
        );

        let mut manual = node();
        assert_eq!(class(&manual), "Unknown");
        manual.class = "c5".to_owned();
        assert_eq!(class(&manual), "C5");
        manual.class = "C16".to_owned();
        assert_eq!(class(&manual), "C16");
    }

    #[test]
    fn traffic_is_shown_only_while_it_still_describes_the_system() {
        let now = 20_000 * 3_600 + 1_200;
        let mut system = located(30_002_187, "Amamake", 0.4, "Heimatar");
        system.jumps_1h = 40;
        system.ship_kills_1h = 2;
        system.intel_updated_at = now - 2 * 86_400;
        assert_eq!(NomadeerApp::recent_traffic(&system, None, now), None);

        let refreshed = SystemTrafficHistory {
            latest_jumps: Some(TrafficReading {
                hour: 0,
                data_time: now - 600,
                value: 300,
            }),
            ..SystemTrafficHistory::default()
        };
        let traffic = NomadeerApp::recent_traffic(&system, Some(&refreshed), now).unwrap();
        assert_eq!(
            traffic,
            RecentTraffic {
                data_time: now - 600,
                jumps: Some(300),
                kills: None,
            }
        );
        assert_eq!(
            NomadeerApp::recent_traffic_label(&traffic),
            "Hour to 08:10 EVE time: 300 jumps"
        );

        // W-space has no traffic feed, so its zeroes are never shown as measured.
        let mut hole = located(31_002_238, "J123456", -0.99, "C-R00023");
        hole.intel_updated_at = now - 600;
        assert_eq!(NomadeerApp::recent_traffic(&hole, None, now), None);
    }

    #[test]
    fn a_briefing_elsewhere_is_named_in_other_panels() {
        let task = UmbraTask::Briefing {
            system_id: "sys_a".to_owned(),
            waking: false,
            auto: true,
        };
        assert_eq!(task.briefing_elsewhere(Some("sys_b")), Some("sys_a"));
        assert_eq!(task.briefing_elsewhere(Some("sys_a")), None);
        assert_eq!(task.briefing_elsewhere(None), None);
        assert_eq!(UmbraTask::Checking.briefing_elsewhere(Some("sys_b")), None);
        assert!(task.is_auto());
        assert_eq!(task.briefing_system(), Some("sys_a"));
        assert!(!UmbraTask::Checking.is_auto());
        assert_eq!(UmbraTask::Checking.briefing_system(), None);
    }
}
