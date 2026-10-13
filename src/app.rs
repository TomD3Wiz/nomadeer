use crate::config::theme;
use crate::esi::{self, AccountAuthorization, LocationSync, SystemIntel};
use crate::forms::*;
use crate::model::*;
use crate::store::Store;
use crate::util::{clean, now};
use crate::widgets::{pill, record_frame};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Sense, Stroke, Vec2};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

const AUTO_SYNC_INTERVAL: Duration = Duration::from_secs(30);

enum EsiTaskResult {
    Authorized(AccountAuthorization),
    Location(LocationSync),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DashboardTab {
    Movement,
    Connections,
    Systems,
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
    esi_busy: bool,
    next_auto_sync: Instant,
    auto_sync_index: usize,
}

impl NomadeerApp {
    pub(crate) fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let store = Store::new(PathBuf::from("data/nomadeer.sqlite3"));
        let (esi_tx, esi_rx) = mpsc::channel();
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
            esi_busy: false,
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

        let visit_id = self.new_id("visit");
        self.data.visits.push(VisitLog {
            id: visit_id,
            system_id: system_id.clone(),
            pilot: character_name.clone(),
            activity: "Location updated".to_owned(),
            notes: String::new(),
            created_at: now(),
        });

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

        let mut positions = HashMap::<String, Pos2>::new();
        let columns = ((self.data.systems.len() as f32).sqrt().ceil() as usize).max(1);
        let cell_w = rect.width() / columns as f32;
        let rows = self.data.systems.len().div_ceil(columns).max(1);
        let cell_h = rect.height() / rows as f32;

        for (index, system) in self.data.systems.iter().enumerate() {
            let col = index % columns;
            let row = index / columns;
            let x = rect.left() + cell_w * (col as f32 + 0.5);
            let y = rect.top() + cell_h * (row as f32 + 0.5);
            positions.insert(system.id.clone(), Pos2::new(x, y));
        }

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
                painter.line_segment([*a, *b], Stroke::new(2.0_f32, color));
            }
        }

        for system in &self.data.systems {
            if let Some(pos) = positions.get(&system.id) {
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
                    &system.class,
                    FontId::proportional(12.0),
                    theme::muted_text(),
                );
            }
        }

        if response.clicked()
            && let Some(pointer_pos) = response.interact_pointer_pos()
        {
            for system in &self.data.systems {
                if let Some(pos) = positions.get(&system.id)
                    && pos.distance(pointer_pos) <= 38.0
                {
                    self.selected_system_id = system.id.clone();
                    break;
                }
            }
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

        let system_id = system.id.clone();
        let system_name = system.name.clone();
        let system_class = system.class.clone();
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
                ui.horizontal_wrapped(|ui| {
                    ui.heading(
                        egui::RichText::new(&account.character_name).color(theme::character_text()),
                    );
                    pill(ui, &account.auth_status);
                });
                if !account.last_system_name.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new("Current:").color(theme::muted_text()));
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
        });
    }

    fn systems_overview(&mut self, ui: &mut egui::Ui) {
        ui.heading("Systems");
        if self.data.systems.is_empty() {
            ui.label("No systems yet.");
            return;
        }

        for system in &self.data.systems {
            let is_selected = system.id == self.selected_system_id;
            record_frame(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(is_selected, &system.name).clicked() {
                        self.selected_system_id = system.id.clone();
                    }
                    pill(ui, &system.class);
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
            });
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

    fn inferred_connections(&self, ui: &mut egui::Ui) {
        ui.heading("Inferred connections");
        let mut connections: Vec<&Connection> = self.data.connections.iter().collect();
        connections.sort_by_key(|connection| std::cmp::Reverse(connection.created_at));

        if connections.is_empty() {
            ui.label("No movement-derived connections yet.");
            return;
        }

        for connection in connections.into_iter().take(8) {
            record_frame(ui, |ui| {
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
                if !connection.notes.is_empty() {
                    ui.label(&connection.notes);
                }
            });
        }
    }
}

impl eframe::App for NomadeerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_esi_tasks();
        self.maybe_auto_sync();
        self.top_bar(ctx);
        self.esi_settings_window(ctx);
        self.side_panel(ctx);
        self.central_panel(ctx);
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}
