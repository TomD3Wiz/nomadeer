use crate::chain_archive::{self, ChainSnapshot, ChainSnapshotMeta};
use crate::config::theme;
use crate::esi::{self, AccountAuthorization, LocationSync, SystemIntel};
use crate::forms::*;
use crate::model::*;
use crate::portrait;
use crate::store::Store;
use crate::util::{clean, now};
use crate::widgets::{pill, record_frame};
use crate::zkill::{self, ZkillReport};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Sense, Stroke, Vec2};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

const AUTO_SYNC_INTERVAL: Duration = Duration::from_secs(5);
const CHAIN_ARCHIVE_DIR: &str = "data/chains";

enum EsiTaskResult {
    Authorized(AccountAuthorization),
    Location(LocationSync),
}

enum ZkillTaskResult {
    Report(ZkillReport),
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
    show_esi_settings: bool,
    id_counter: u64,
    esi_tx: Sender<Result<EsiTaskResult, String>>,
    esi_rx: Receiver<Result<EsiTaskResult, String>>,
    zkill_tx: Sender<Result<ZkillTaskResult, String>>,
    zkill_rx: Receiver<Result<ZkillTaskResult, String>>,
    portrait_tx: Sender<Result<PortraitTaskResult, String>>,
    portrait_rx: Receiver<Result<PortraitTaskResult, String>>,
    esi_busy: bool,
    zkill_busy_system_id: Option<u64>,
    zkill_reports: HashMap<u64, ZkillReport>,
    portrait_textures: HashMap<String, egui::TextureHandle>,
    portrait_requested: HashSet<String>,
    chain_snapshot_name: String,
    chain_snapshots: Vec<ChainSnapshotMeta>,
    chain_context_target: Option<ChainMapTarget>,
    next_auto_sync: Instant,
    auto_sync_index: usize,
}

impl NomadeerApp {
    pub(crate) fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let store = Store::new(PathBuf::from("data/nomadeer.sqlite3"));
        let (esi_tx, esi_rx) = mpsc::channel();
        let (zkill_tx, zkill_rx) = mpsc::channel();
        let (portrait_tx, portrait_rx) = mpsc::channel();
        let mut app = Self {
            data: AppData::default(),
            store,
            status: "Ready".to_owned(),
            selected_system_id: String::new(),
            active_tab: DashboardTab::Movement,
            esi_settings_form: EsiSettingsForm::default(),
            show_esi_settings: false,
            id_counter: 0,
            esi_tx,
            esi_rx,
            zkill_tx,
            zkill_rx,
            portrait_tx,
            portrait_rx,
            esi_busy: false,
            zkill_busy_system_id: None,
            zkill_reports: HashMap::new(),
            portrait_textures: HashMap::new(),
            portrait_requested: HashSet::new(),
            chain_snapshot_name: String::new(),
            chain_snapshots: Vec::new(),
            chain_context_target: None,
            next_auto_sync: Instant::now() + Duration::from_secs(5),
            auto_sync_index: 0,
        };
        app.load();
        app
    }

    fn load(&mut self) {
        match self.store.load() {
            Ok(data) => {
                self.data = data;
                self.sync_esi_settings_form();
                self.reconcile_selected_system();
                self.refresh_chain_snapshots();
                self.default_chain_snapshot_name();
                self.status = "Loaded".to_owned();
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

    fn refresh_chain_snapshots(&mut self) {
        match chain_archive::list_snapshots(&PathBuf::from(CHAIN_ARCHIVE_DIR)) {
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

        self.data.esi_settings.client_id = clean(&self.esi_settings_form.client_id);
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

    fn inferred_system_class(system: &SystemNode) -> String {
        if let Some(class) = Self::wormhole_class_from_security_class(&system.class) {
            return class;
        }
        if let Some(class) = Self::wormhole_class_from_security_class(&system.constellation_name) {
            return class;
        }
        if let Some(class) = Self::wormhole_class_from_security_class(&system.region_name) {
            return class;
        }

        if system.security_status >= 0.45 {
            "HS".to_owned()
        } else if system.security_status > 0.0 {
            "LS".to_owned()
        } else if system.security_status <= 0.0 && !system.name.starts_with('J') {
            "NS".to_owned()
        } else {
            "Unknown".to_owned()
        }
    }

    fn wormhole_class_from_security_class(security_class: &str) -> Option<String> {
        let mut parts = security_class.split('-');
        let prefix = parts.next()?;
        let class_code = parts.next()?.chars().next()?;
        if prefix != "C" {
            return None;
        }

        let class = match class_code {
            'A' => "C1",
            'B' => "C2",
            'C' => "C3",
            'D' => "C4",
            'E' => "C5",
            'F' => "C6",
            _ => return None,
        };
        Some(class.to_owned())
    }

    fn system_intel_summary(system: &SystemNode) -> String {
        let region = if system.region_name.is_empty() {
            "Unknown region"
        } else {
            &system.region_name
        };

        format!(
            "{} · {} · {} jumps · kills {} ship / {} pod / {} NPC",
            Self::inferred_system_class(system),
            region,
            system.jumps_1h,
            system.ship_kills_1h,
            system.pod_kills_1h,
            system.npc_kills_1h
        )
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Nomadeer");
                if ui.button("ESI settings").clicked() {
                    self.show_esi_settings = true;
                }
                if ui
                    .add_enabled(!self.esi_busy, egui::Button::new("Add character"))
                    .clicked()
                {
                    self.start_add_character();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(&self.status);
                });
            });
        });
    }

    fn esi_settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_esi_settings;
        let mut close_requested = false;
        egui::Window::new("ESI settings")
            .open(&mut open)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| {
                close_requested = self.esi_settings_contents(ui);
            });
        if close_requested {
            open = false;
        }
        self.show_esi_settings = open;
    }

    fn esi_settings_contents(&mut self, ui: &mut egui::Ui) -> bool {
        let mut close_requested = false;
        ui.label("Register an EVE app with this callback URL:");
        ui.monospace(format!(
            "http://localhost:{}/callback",
            self.esi_settings_form.callback_port
        ));
        ui.add_space(8.0);
        ui.label("Client ID");
        ui.add(
            egui::TextEdit::singleline(&mut self.esi_settings_form.client_id)
                .hint_text("EVE Developers application Client ID"),
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

    fn start_authorize_account(&mut self, account_id: String) {
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

        self.spawn_authorize_account(account_id);
    }

    fn start_add_character(&mut self) {
        if self.esi_busy {
            self.status = "Already working on a character update".to_owned();
            return;
        }
        if !self.save_esi_settings_from_form() {
            self.show_esi_settings = true;
            return;
        }
        if self.data.esi_settings.client_id.is_empty() {
            self.status = "Set your ESI Client ID before adding a character".to_owned();
            self.show_esi_settings = true;
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
        });
        self.save();
        self.spawn_authorize_account(account_id);
    }

    fn spawn_authorize_account(&mut self, account_id: String) {
        self.esi_busy = true;
        self.status = "Opening browser to connect character...".to_owned();
        let tx = self.esi_tx.clone();
        let settings = self.data.esi_settings.clone();
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

    fn maybe_auto_sync(&mut self) {
        if self.esi_busy || Instant::now() < self.next_auto_sync {
            return;
        }

        let accounts: Vec<Account> = self
            .data
            .accounts
            .iter()
            .filter(|account| account.auth_status == "authorized")
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

    fn start_zkill_refresh(&mut self) {
        let Some(system) = self.selected_system() else {
            self.status = "Select a system first".to_owned();
            return;
        };
        if system.solar_system_id == 0 {
            self.status = "Selected system has no EVE system ID yet".to_owned();
            return;
        }
        if self.zkill_busy_system_id.is_some() {
            self.status = "Already fetching killboard intel".to_owned();
            return;
        }

        let solar_system_id = system.solar_system_id;
        let system_name = system.name.clone();
        self.zkill_busy_system_id = Some(solar_system_id);
        self.status = format!("Fetching killboard intel for {system_name}...");
        let tx = self.zkill_tx.clone();
        thread::spawn(move || {
            let result = zkill::fetch_system_report(solar_system_id).map(ZkillTaskResult::Report);
            let _ = tx.send(result);
        });
    }

    fn poll_zkill_tasks(&mut self) {
        loop {
            match self.zkill_rx.try_recv() {
                Ok(Ok(ZkillTaskResult::Report(report))) => {
                    let kill_count = report.recent_kills.len();
                    self.zkill_busy_system_id = None;
                    self.status = format!("Loaded {kill_count} recent kills from zKillboard");
                    self.zkill_reports.insert(report.solar_system_id, report);
                }
                Ok(Err(error)) => {
                    self.zkill_busy_system_id = None;
                    self.status = format!("Killboard intel failed: {error}");
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.zkill_busy_system_id = None;
                    self.status = "Killboard update stopped unexpectedly".to_owned();
                    break;
                }
            }
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
                Ok(Err(error)) => {
                    self.esi_busy = false;
                    self.status = format!("Character update failed: {error}");
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.esi_busy = false;
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
            self.save();
            self.status = "Character connected".to_owned();
        } else {
            self.status = "Character connected, but it is no longer in the list".to_owned();
        }
    }

    fn apply_location_sync(&mut self, location: LocationSync) {
        let system_id = self.ensure_esi_system(&location.system);

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

        self.data.accounts[account_index].access_token = Some(location.access_token);
        self.data.accounts[account_index].refresh_token = Some(location.refresh_token);
        self.data.accounts[account_index].token_expires_at = Some(location.token_expires_at);
        self.data.accounts[account_index].last_system_id = system_id.clone();
        self.data.accounts[account_index].last_system_name = location.system.name.clone();
        self.data.accounts[account_index].auth_status = "authorized".to_owned();
        self.data.accounts[account_index].updated_at = now();

        if previous_system_id.is_empty() || previous_system_id != system_id {
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

        self.save();
        if self.selected_system_id.is_empty() {
            self.selected_system_id = system_id;
        }
        self.status = format!("{character_name} is in {}", location.system.name);
    }

    fn ensure_esi_system(&mut self, intel: &SystemIntel) -> String {
        let esi_id = format!("esi_sys_{}", intel.solar_system_id);
        if let Some(existing) = self
            .data
            .systems
            .iter_mut()
            .find(|system| system.id == esi_id)
        {
            Self::apply_system_intel(existing, intel);
            return esi_id;
        }
        if let Some(existing) = self
            .data
            .systems
            .iter_mut()
            .find(|system| system.name == intel.name)
        {
            Self::apply_system_intel(existing, intel);
            return existing.id.clone();
        }

        self.data.systems.push(SystemNode {
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
            jumps_1h: intel.jumps_1h,
            ship_kills_1h: intel.ship_kills_1h,
            pod_kills_1h: intel.pod_kills_1h,
            npc_kills_1h: intel.npc_kills_1h,
            intel_updated_at: now(),
            effect: String::new(),
            status: "scouted".to_owned(),
            owner: String::new(),
            notes: format!("Solar system ID {}", intel.solar_system_id),
            created_at: now(),
        });
        esi_id
    }

    fn apply_system_intel(system: &mut SystemNode, intel: &SystemIntel) {
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
        system.jumps_1h = intel.jumps_1h;
        system.ship_kills_1h = intel.ship_kills_1h;
        system.pod_kills_1h = intel.pod_kills_1h;
        system.npc_kills_1h = intel.npc_kills_1h;
        system.intel_updated_at = now();
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
        });
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

        match chain_archive::save_snapshot(&PathBuf::from(CHAIN_ARCHIVE_DIR), &snapshot) {
            Ok(_) => {
                self.chain_snapshot_name = format!("Chain {}", now());
                self.refresh_chain_snapshots();
                self.status = format!("Saved chain {name}");
            }
            Err(error) => self.status = format!("Could not save chain: {error}"),
        }
    }

    fn load_chain_snapshot(&mut self, file_name: &str) {
        match chain_archive::load_snapshot(&PathBuf::from(CHAIN_ARCHIVE_DIR), file_name) {
            Ok(snapshot) => {
                let name = snapshot.name.clone();
                self.data.systems = snapshot.systems;
                self.data.connections = snapshot.connections;
                self.data.signatures = snapshot.signatures;
                self.data.visits = snapshot.visits;
                self.data.notes = snapshot.notes;
                self.selected_system_id = snapshot.selected_system_id;
                self.clear_account_locations_outside_chain();
                self.reconcile_selected_system();
                self.save();
                self.status = format!("Loaded chain {name}");
            }
            Err(error) => self.status = format!("Could not load chain: {error}"),
        }
    }

    fn delete_chain_snapshot(&mut self, file_name: &str) {
        match chain_archive::delete_snapshot(&PathBuf::from(CHAIN_ARCHIVE_DIR), file_name) {
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
        for account in &mut self.data.accounts {
            account.last_system_id.clear();
            account.last_system_name.clear();
        }
        self.save();
        self.status = "Cleared current chain".to_owned();
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
        let Some(system_name) = self
            .data
            .systems
            .iter()
            .find(|system| system.id == system_id)
            .map(|system| system.name.clone())
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
                ui.add_space(2.0);
                self.selected_system_sidebar(ui);
                ui.separator();
                self.characters_sidebar(ui);
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
                ui.label("Select a system to inspect it");
            });
            self.chain_map(ui);
            ui.separator();
            self.dashboard_tabs(ui);
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| match self.active_tab {
                DashboardTab::Movement => self.recent_movements(ui),
                DashboardTab::Connections => self.inferred_connections(ui),
                DashboardTab::Systems => self.systems_overview(ui),
                DashboardTab::Intel => self.selected_system_intel(ui),
                DashboardTab::Chain => self.chain_management(ui),
            });
        });
    }

    fn chain_map(&mut self, ui: &mut egui::Ui) {
        let desired = Vec2::new(ui.available_width(), 360.0);
        let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 12.0, theme::panel_background());
        painter.rect_stroke(
            rect,
            12.0,
            Stroke::new(1.0_f32, theme::panel_stroke()),
            egui::StrokeKind::Inside,
        );

        if self.data.systems.is_empty() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Sync a character location to start mapping the chain",
                FontId::proportional(18.0),
                theme::muted_text(),
            );
            return;
        }

        let positions = self.chain_positions(rect);
        let pointer_pos = ui
            .ctx()
            .input(|input| input.pointer.hover_pos())
            .filter(|pos| rect.contains(*pos));
        let hovered_system_id = pointer_pos.and_then(|pos| self.chain_system_at(&positions, pos));
        let hovered_connection_id = if hovered_system_id.is_none() {
            pointer_pos.and_then(|pos| self.chain_connection_at(&positions, pos))
        } else {
            None
        };

        for connection in &self.data.connections {
            if let (Some(a), Some(b)) = (
                positions.get(&connection.from_system),
                positions.get(&connection.to_system),
            ) {
                let color = match connection.mass_status.as_str() {
                    "critical" => Color32::from_rgb(255, 91, 112),
                    "destab" => Color32::from_rgb(255, 194, 87),
                    _ => Color32::from_rgb(93, 242, 255),
                };
                let delta = *b - *a;
                let distance = delta.length();
                let endpoints = if distance > 76.0 {
                    let offset = delta / distance * 38.0;
                    [*a + offset, *b - offset]
                } else {
                    [*a, *b]
                };
                let is_hovered = hovered_connection_id.as_deref() == Some(&connection.id);
                painter.line_segment(
                    endpoints,
                    Stroke::new(
                        if is_hovered { 4.0_f32 } else { 2.0_f32 },
                        if is_hovered { Color32::WHITE } else { color },
                    ),
                );
            }
        }

        for system in &self.data.systems {
            if let Some(pos) = positions.get(&system.id) {
                let is_hovered = hovered_system_id.as_deref() == Some(&system.id);
                let fill = match system.status.as_str() {
                    "home" => Color32::from_rgb(20, 89, 82),
                    "hostile" => Color32::from_rgb(92, 31, 43),
                    "farmable" => Color32::from_rgb(37, 75, 42),
                    "rolled" => Color32::from_rgb(45, 48, 56),
                    _ => Color32::from_rgb(16, 41, 50),
                };
                painter.circle_filled(*pos, 38.0, fill);
                let stroke = if system.id == self.selected_system_id {
                    Stroke::new(4.0_f32, theme::character_text())
                } else if is_hovered {
                    Stroke::new(4.0_f32, Color32::WHITE)
                } else {
                    Stroke::new(2.0_f32, theme::accent())
                };
                painter.circle_stroke(*pos, 38.0, stroke);
                painter.text(
                    *pos + Vec2::new(0.0, -6.0),
                    Align2::CENTER_CENTER,
                    &system.name,
                    FontId::proportional(14.0),
                    Color32::WHITE,
                );
                painter.text(
                    *pos + Vec2::new(0.0, 12.0),
                    Align2::CENTER_CENTER,
                    Self::inferred_system_class(system),
                    FontId::proportional(12.0),
                    theme::muted_text(),
                );
            }
        }

        if let Some(pos) = pointer_pos {
            if let Some(system_id) = &hovered_system_id {
                if let Some(system) = self.system_by_id(system_id) {
                    painter.text(
                        pos + Vec2::new(14.0, 14.0),
                        Align2::LEFT_TOP,
                        format!("{} · right-click for actions", system.name),
                        FontId::proportional(13.0),
                        Color32::WHITE,
                    );
                }
            } else if let Some(connection_id) = &hovered_connection_id
                && let Some(connection) = self
                    .data
                    .connections
                    .iter()
                    .find(|connection| connection.id == *connection_id)
            {
                painter.text(
                    pos + Vec2::new(14.0, 14.0),
                    Align2::LEFT_TOP,
                    format!(
                        "{} ↔ {} · right-click for actions",
                        self.system_name(&connection.from_system),
                        self.system_name(&connection.to_system)
                    ),
                    FontId::proportional(13.0),
                    Color32::WHITE,
                );
            }
        }

        if response.clicked()
            && let Some(pointer_pos) = response.interact_pointer_pos()
            && let Some(system_id) = self.chain_system_at(&positions, pointer_pos)
        {
            self.selected_system_id = system_id;
        }

        if response.secondary_clicked()
            && let Some(pointer_pos) = response.interact_pointer_pos()
        {
            self.chain_context_target = self
                .chain_system_at(&positions, pointer_pos)
                .map(ChainMapTarget::System)
                .or_else(|| {
                    self.chain_connection_at(&positions, pointer_pos)
                        .map(ChainMapTarget::Connection)
                })
                .or(Some(ChainMapTarget::Map));
        }

        let menu_target = self
            .chain_context_target
            .clone()
            .unwrap_or(ChainMapTarget::Map);
        let mut select_system_id = None;
        let mut delete_system_id = None;
        let mut delete_connection_id = None;
        let mut clear_chain = false;
        response.context_menu(|ui| match &menu_target {
            ChainMapTarget::System(system_id) => {
                let system_name = self.system_name(system_id);
                ui.label(egui::RichText::new(&system_name).color(theme::system_text()));
                if ui.button("Select system").clicked() {
                    select_system_id = Some(system_id.clone());
                    ui.close();
                }
                if ui.button("Delete system").clicked() {
                    delete_system_id = Some(system_id.clone());
                    ui.close();
                }
            }
            ChainMapTarget::Connection(connection_id) => {
                if let Some(connection) = self
                    .data
                    .connections
                    .iter()
                    .find(|connection| connection.id == *connection_id)
                {
                    ui.label(format!(
                        "{} ↔ {}",
                        self.system_name(&connection.from_system),
                        self.system_name(&connection.to_system)
                    ));
                    if !connection.notes.is_empty() {
                        ui.label(egui::RichText::new(&connection.notes).color(theme::muted_text()));
                    }
                } else {
                    ui.label("Connection");
                }
                if ui.button("Delete connection").clicked() {
                    delete_connection_id = Some(connection_id.clone());
                    ui.close();
                }
            }
            ChainMapTarget::Map => {
                ui.label("Chain map");
                if ui.button("Clear current chain").clicked() {
                    clear_chain = true;
                    ui.close();
                }
            }
        });

        if let Some(system_id) = select_system_id {
            self.selected_system_id = system_id;
        }
        if let Some(connection_id) = delete_connection_id {
            self.delete_connection(&connection_id);
        }
        if let Some(system_id) = delete_system_id {
            self.delete_system(&system_id);
        }
        if clear_chain {
            self.clear_chain();
        }
    }

    fn chain_system_at(
        &self,
        positions: &HashMap<String, Pos2>,
        pointer_pos: Pos2,
    ) -> Option<String> {
        self.data
            .systems
            .iter()
            .find(|system| {
                positions
                    .get(&system.id)
                    .is_some_and(|pos| pos.distance(pointer_pos) <= 38.0)
            })
            .map(|system| system.id.clone())
    }

    fn chain_connection_at(
        &self,
        positions: &HashMap<String, Pos2>,
        pointer_pos: Pos2,
    ) -> Option<String> {
        self.data
            .connections
            .iter()
            .filter_map(|connection| {
                let a = positions.get(&connection.from_system)?;
                let b = positions.get(&connection.to_system)?;
                let distance = Self::distance_to_segment(pointer_pos, *a, *b);
                (distance <= 8.0).then_some((connection.id.clone(), distance))
            })
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(id, _)| id)
    }

    fn distance_to_segment(point: Pos2, a: Pos2, b: Pos2) -> f32 {
        let ab = b - a;
        let length_squared = ab.length_sq();
        if length_squared <= f32::EPSILON {
            return point.distance(a);
        }

        let ap = point - a;
        let t = (ap.dot(ab) / length_squared).clamp(0.0, 1.0);
        point.distance(a + ab * t)
    }

    fn chain_positions(&self, rect: egui::Rect) -> HashMap<String, Pos2> {
        let mut positions = HashMap::<String, Pos2>::new();
        if self.data.systems.is_empty() {
            return positions;
        }

        let node_margin = 54.0;
        let inner = rect.shrink(node_margin);
        let system_order = self
            .data
            .systems
            .iter()
            .enumerate()
            .map(|(index, system)| (system.id.clone(), index))
            .collect::<HashMap<_, _>>();
        let system_ids = self
            .data
            .systems
            .iter()
            .map(|system| system.id.clone())
            .collect::<HashSet<_>>();
        let mut adjacency = self
            .data
            .systems
            .iter()
            .map(|system| (system.id.clone(), Vec::<String>::new()))
            .collect::<HashMap<_, _>>();

        for connection in &self.data.connections {
            if connection.from_system == connection.to_system
                || !system_ids.contains(&connection.from_system)
                || !system_ids.contains(&connection.to_system)
            {
                continue;
            }

            if let Some(neighbors) = adjacency.get_mut(&connection.from_system) {
                neighbors.push(connection.to_system.clone());
            }
            if let Some(neighbors) = adjacency.get_mut(&connection.to_system) {
                neighbors.push(connection.from_system.clone());
            }
        }

        for neighbors in adjacency.values_mut() {
            neighbors.sort_by_key(|id| system_order.get(id).copied().unwrap_or(usize::MAX));
            neighbors.dedup();
        }

        let mut visited = HashSet::<String>::new();
        let mut components = Vec::<Vec<Vec<String>>>::new();
        while visited.len() < self.data.systems.len() {
            let Some(root) = self
                .data
                .systems
                .iter()
                .filter(|system| !visited.contains(&system.id))
                .max_by_key(|system| {
                    (
                        adjacency.get(&system.id).map_or(0, Vec::len),
                        std::cmp::Reverse(system_order.get(&system.id).copied().unwrap_or(0)),
                    )
                })
                .map(|system| system.id.clone())
            else {
                break;
            };

            let mut levels = Vec::<Vec<String>>::new();
            let mut queue = VecDeque::from([(root.clone(), 0_usize)]);
            visited.insert(root);

            while let Some((system_id, depth)) = queue.pop_front() {
                if levels.len() <= depth {
                    levels.push(Vec::new());
                }
                levels[depth].push(system_id.clone());

                for neighbor in adjacency.get(&system_id).into_iter().flatten() {
                    if visited.insert(neighbor.clone()) {
                        queue.push_back((neighbor.clone(), depth + 1));
                    }
                }
            }

            components.push(levels);
        }

        let max_depth = components.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let component_gap = usize::from(components.len() > 1);
        let total_slots = components
            .iter()
            .map(|levels| levels.iter().map(Vec::len).max().unwrap_or(1).max(1))
            .sum::<usize>()
            + component_gap * components.len().saturating_sub(1);

        let mut slot_cursor = 0_usize;
        for levels in components {
            let component_slots = levels.iter().map(Vec::len).max().unwrap_or(1).max(1);
            for (depth, nodes) in levels.iter().enumerate() {
                let x = if max_depth == 1 {
                    inner.center().x
                } else {
                    inner.left() + inner.width() * depth as f32 / (max_depth - 1) as f32
                };
                let level_offset = (component_slots.saturating_sub(nodes.len())) as f32 / 2.0;

                for (index, system_id) in nodes.iter().enumerate() {
                    let slot = slot_cursor as f32 + level_offset + index as f32;
                    let y = if total_slots <= 1 {
                        inner.center().y
                    } else {
                        inner.top() + inner.height() * (slot + 0.5) / total_slots as f32
                    };
                    positions.insert(system_id.clone(), Pos2::new(x, y));
                }
            }
            slot_cursor += component_slots + component_gap;
        }

        positions
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
        let jumps_1h = system.jumps_1h;
        let ship_kills_1h = system.ship_kills_1h;
        let pod_kills_1h = system.pod_kills_1h;
        let npc_kills_1h = system.npc_kills_1h;
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
            ui.label(format!(
                "Last hour: {} jumps · {} ships · {} pods · {} NPCs",
                jumps_1h, ship_kills_1h, pod_kills_1h, npc_kills_1h
            ));
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
    }

    fn characters_sidebar(&mut self, ui: &mut egui::Ui) {
        let mut authorize_id = None;
        let mut sync_id = None;
        let mut delete_id = None;

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

        if let Some(id) = delete_id {
            self.data.accounts.retain(|account| account.id != id);
            self.reconcile_selected_system();
            self.save();
        }
        if let Some(id) = authorize_id {
            self.start_authorize_account(id);
        }
        if let Some(id) = sync_id {
            self.start_sync_location(id);
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
        ui.heading("Intel");
        let Some(system) = self.selected_system().cloned() else {
            ui.label("Select a system on the chain map first.");
            return;
        };

        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(&system.name).color(theme::system_text()));
            pill(ui, &Self::inferred_system_class(&system));
            if system.solar_system_id != 0 {
                ui.hyperlink_to(
                    "Open zKillboard",
                    format!("https://zkillboard.com/system/{}/", system.solar_system_id),
                );
                ui.hyperlink_to(
                    "Open Anoik.is",
                    format!("https://anoik.is/systems/{}", system.name),
                );
            }
            if ui
                .add_enabled(
                    self.zkill_busy_system_id.is_none() && system.solar_system_id != 0,
                    egui::Button::new("Refresh zKill"),
                )
                .clicked()
            {
                self.start_zkill_refresh();
            }
        });

        let Some(report) = self.zkill_reports.get(&system.solar_system_id) else {
            ui.label(
                "No zKillboard data loaded yet. Refresh to pull recent kills for this system.",
            );
            return;
        };

        ui.label(format!(
            "{} recent kills · {} total · {} solo · {} NPC",
            report.recent_kills.len(),
            Self::format_isk(report.total_value),
            report.solo_kills,
            report.npc_kills
        ));

        if report.recent_kills.is_empty() {
            ui.label("No recent kills returned by zKillboard.");
            return;
        }

        for kill in &report.recent_kills {
            record_frame(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let (relative_time, time_color) = Self::relative_kill_time(&kill.killmail_time);
                    ui.label(egui::RichText::new(relative_time).color(time_color));
                    ui.hyperlink_to(
                        format!("Kill {}", kill.killmail_id),
                        format!("https://zkillboard.com/kill/{}/", kill.killmail_id),
                    );
                    if kill.solo {
                        pill(ui, "solo");
                    }
                    if kill.npc {
                        pill(ui, "NPC");
                    }
                });
                ui.label(format!(
                    "Victim ship type {} · {} attackers · {}",
                    kill.victim_ship_type_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "unknown".to_owned()),
                    kill.attacker_count,
                    Self::format_isk(kill.total_value)
                ));
            });
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
                    "sec {:.1} · {} planets · {} gates · {} jumps last hour",
                    system.security_status,
                    system.planet_count,
                    system.stargate_count,
                    system.jumps_1h
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
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.chain_snapshot_name);
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

    fn inferred_connections(&mut self, ui: &mut egui::Ui) {
        ui.heading("Inferred connections");
        let mut connections = self.data.connections.clone();
        connections.sort_by_key(|connection| std::cmp::Reverse(connection.created_at));

        if connections.is_empty() {
            ui.label("No movement-derived connections yet.");
            return;
        }

        let mut delete_id = None;
        for connection in connections.into_iter().take(12) {
            record_frame(ui, |ui| {
                let from_system = self.system_by_id(&connection.from_system);
                let to_system = self.system_by_id(&connection.to_system);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(self.system_name(&connection.from_system))
                            .color(theme::system_text()),
                    );
                    ui.label(egui::RichText::new("→").color(theme::muted_text()));
                    ui.label(
                        egui::RichText::new(self.system_name(&connection.to_system))
                            .color(theme::system_text()),
                    );
                });
                if let Some(system) = from_system {
                    ui.label(Self::system_intel_summary(system));
                }
                if let Some(system) = to_system {
                    ui.label(Self::system_intel_summary(system));
                }
                if !connection.notes.is_empty() {
                    ui.label(&connection.notes);
                }
                if ui.small_button("Delete connection").clicked() {
                    delete_id = Some(connection.id.clone());
                }
            });
        }

        if let Some(id) = delete_id {
            self.delete_connection(&id);
        }
    }
}

impl eframe::App for NomadeerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_esi_tasks();
        self.poll_zkill_tasks();
        self.poll_portrait_tasks(ctx);
        self.maybe_request_portraits();
        self.maybe_auto_sync();
        self.top_bar(ctx);
        self.esi_settings_window(ctx);
        self.side_panel(ctx);
        self.central_panel(ctx);
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}
