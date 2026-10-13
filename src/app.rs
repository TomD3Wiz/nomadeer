use crate::esi::{self, AccountAuthorization, LocationSync};
use crate::forms::*;
use crate::model::*;
use crate::store::Store;
use crate::util::{clean, empty_dash, now};
use crate::widgets::{combo, pill, record_frame, system_combo, tab_button};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Sense, Stroke, Vec2};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

enum EsiTaskResult {
    Authorized(AccountAuthorization),
    Location(LocationSync),
}

pub(crate) struct NomadeerApp {
    data: AppData,
    store: Store,
    status: String,
    active_tab: PanelTab,
    esi_settings_form: EsiSettingsForm,
    account_form: AccountForm,
    system_form: SystemForm,
    connection_form: ConnectionForm,
    signature_form: SignatureForm,
    visit_form: VisitForm,
    note_form: NoteForm,
    id_counter: u64,
    esi_tx: Sender<Result<EsiTaskResult, String>>,
    esi_rx: Receiver<Result<EsiTaskResult, String>>,
    esi_busy: bool,
}

impl NomadeerApp {
    pub(crate) fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let store = Store::new(PathBuf::from("data/nomadeer.sqlite3"));
        let (esi_tx, esi_rx) = mpsc::channel();
        let mut app = Self {
            data: AppData::default(),
            store,
            status: "Ready".to_owned(),
            active_tab: PanelTab::Systems,
            esi_settings_form: EsiSettingsForm::default(),
            account_form: AccountForm::default(),
            system_form: SystemForm {
                class: "C3".to_owned(),
                status: "scouted".to_owned(),
                ..Default::default()
            },
            connection_form: ConnectionForm {
                mass_status: "fresh".to_owned(),
                life_status: "fresh".to_owned(),
                ..Default::default()
            },
            signature_form: SignatureForm {
                sig_type: "wormhole".to_owned(),
                life: "fresh".to_owned(),
                status: "unscanned".to_owned(),
                ..Default::default()
            },
            visit_form: VisitForm::default(),
            note_form: NoteForm::default(),
            id_counter: 0,
            esi_tx,
            esi_rx,
            esi_busy: false,
        };
        app.load();
        app
    }

    fn load(&mut self) {
        match self.store.load() {
            Ok(data) => {
                self.data = data;
                self.sync_esi_settings_form();
                self.status = format!("Loaded {}", self.store.path().display());
            }
            Err(error) => {
                self.status = format!("Could not load database: {error}");
            }
        }
    }

    fn save(&mut self) {
        match self.store.save(&self.data) {
            Ok(()) => self.status = format!("Saved {}", self.store.path().display()),
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

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Nomadeer");
                ui.label("Private native wormhole chain tracker");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reload").clicked() {
                        self.load();
                    }
                    if ui.button("Save").clicked() {
                        self.save();
                    }
                });
            });
            ui.label(format!(
                "Data: {} · {}",
                self.store.path().display(),
                self.status
            ));
        });
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("forms")
            .resizable(true)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.heading("Add intel");
                ui.separator();
                self.esi_settings_ui(ui);
                ui.separator();
                self.account_form_ui(ui);
                ui.separator();
                self.system_form_ui(ui);
                ui.separator();
                self.connection_form_ui(ui);
                ui.separator();
                self.signature_form_ui(ui);
                ui.separator();
                self.visit_form_ui(ui);
                ui.separator();
                self.note_form_ui(ui);
            });
    }

    fn esi_settings_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("ESI settings")
            .default_open(true)
            .show(ui, |ui| {
                ui.label("Register an EVE app with this callback URL:");
                ui.monospace(format!(
                    "http://localhost:{}/callback",
                    self.esi_settings_form.callback_port
                ));
                ui.text_edit_singleline(&mut self.esi_settings_form.client_id)
                    .on_hover_text("EVE Developers application Client ID");
                ui.text_edit_singleline(&mut self.esi_settings_form.callback_port)
                    .on_hover_text("Local callback port");
                ui.text_edit_multiline(&mut self.esi_settings_form.scopes)
                    .on_hover_text("Space-separated ESI scopes");
                if ui.button("Save ESI settings").clicked() {
                    self.save_esi_settings_from_form();
                }
            });
    }

    fn account_form_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Account")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("Add a character placeholder, then authorize it from the Accounts tab.");
                ui.text_edit_singleline(&mut self.account_form.character_name)
                    .on_hover_text("Character name");
                ui.text_edit_singleline(&mut self.account_form.character_id)
                    .on_hover_text("Optional EVE character ID");

                let can_add = !self.account_form.character_name.trim().is_empty();
                if ui
                    .add_enabled(can_add, egui::Button::new("Add account"))
                    .clicked()
                {
                    let timestamp = now();
                    let account = Account {
                        id: self.new_id("acct"),
                        character_id: clean(&self.account_form.character_id),
                        character_name: clean(&self.account_form.character_name),
                        owner_hash: String::new(),
                        auth_status: "unauthorized".to_owned(),
                        last_system_id: String::new(),
                        last_system_name: String::new(),
                        access_token: None,
                        refresh_token: None,
                        token_expires_at: None,
                        created_at: timestamp,
                        updated_at: timestamp,
                    };
                    self.data.accounts.push(account);
                    self.account_form.character_name.clear();
                    self.account_form.character_id.clear();
                    self.save();
                }
            });
    }

    fn system_form_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("System")
            .default_open(true)
            .show(ui, |ui| {
                ui.text_edit_singleline(&mut self.system_form.name)
                    .on_hover_text("J123456, Thera, Amarr, etc.");
                combo(ui, "Class", &mut self.system_form.class, SYSTEM_CLASSES);
                ui.text_edit_singleline(&mut self.system_form.effect)
                    .on_hover_text("Magnetar, Pulsar, Wolf-Rayet...");
                combo(ui, "Status", &mut self.system_form.status, SYSTEM_STATUSES);
                ui.text_edit_singleline(&mut self.system_form.owner)
                    .on_hover_text("Corp/alliance or occupants");
                ui.text_edit_multiline(&mut self.system_form.notes);

                let can_add = !self.system_form.name.trim().is_empty();
                if ui
                    .add_enabled(can_add, egui::Button::new("Add system"))
                    .clicked()
                {
                    let system = SystemNode {
                        id: self.new_id("sys"),
                        name: clean(&self.system_form.name),
                        class: clean(&self.system_form.class),
                        effect: clean(&self.system_form.effect),
                        status: clean(&self.system_form.status),
                        owner: clean(&self.system_form.owner),
                        notes: clean(&self.system_form.notes),
                        created_at: now(),
                    };
                    self.data.systems.push(system);
                    self.system_form.name.clear();
                    self.system_form.effect.clear();
                    self.system_form.owner.clear();
                    self.system_form.notes.clear();
                    self.save();
                }
            });
    }

    fn connection_form_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Connection")
            .default_open(true)
            .show(ui, |ui| {
                system_combo(
                    ui,
                    "From",
                    &self.data.systems,
                    &mut self.connection_form.from_system,
                );
                system_combo(
                    ui,
                    "To",
                    &self.data.systems,
                    &mut self.connection_form.to_system,
                );
                ui.text_edit_singleline(&mut self.connection_form.signature_from)
                    .on_hover_text("Source side sig, e.g. ABC-123");
                ui.text_edit_singleline(&mut self.connection_form.signature_to)
                    .on_hover_text("Destination side sig");
                combo(
                    ui,
                    "Mass",
                    &mut self.connection_form.mass_status,
                    MASS_STATES,
                );
                combo(
                    ui,
                    "Life",
                    &mut self.connection_form.life_status,
                    LIFE_STATES,
                );
                ui.text_edit_singleline(&mut self.connection_form.ship_size)
                    .on_hover_text("frig, medium, large, capital");
                ui.text_edit_multiline(&mut self.connection_form.notes);

                let can_add = !self.connection_form.from_system.is_empty()
                    && !self.connection_form.to_system.is_empty()
                    && self.connection_form.from_system != self.connection_form.to_system;
                if ui
                    .add_enabled(can_add, egui::Button::new("Add connection"))
                    .clicked()
                {
                    let connection = Connection {
                        id: self.new_id("conn"),
                        from_system: self.connection_form.from_system.clone(),
                        to_system: self.connection_form.to_system.clone(),
                        signature_from: clean(&self.connection_form.signature_from),
                        signature_to: clean(&self.connection_form.signature_to),
                        mass_status: clean(&self.connection_form.mass_status),
                        life_status: clean(&self.connection_form.life_status),
                        ship_size: clean(&self.connection_form.ship_size),
                        notes: clean(&self.connection_form.notes),
                        created_at: now(),
                    };
                    self.data.connections.push(connection);
                    self.connection_form.signature_from.clear();
                    self.connection_form.signature_to.clear();
                    self.connection_form.ship_size.clear();
                    self.connection_form.notes.clear();
                    self.save();
                }
            });
    }

    fn signature_form_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Signature")
            .default_open(false)
            .show(ui, |ui| {
                system_combo(
                    ui,
                    "System",
                    &self.data.systems,
                    &mut self.signature_form.system_id,
                );
                ui.text_edit_singleline(&mut self.signature_form.code)
                    .on_hover_text("ABC-123");
                combo(ui, "Type", &mut self.signature_form.sig_type, SIG_TYPES);
                ui.text_edit_singleline(&mut self.signature_form.target)
                    .on_hover_text("Destination or site name");
                combo(ui, "Life", &mut self.signature_form.life, LIFE_STATES);
                ui.text_edit_singleline(&mut self.signature_form.status)
                    .on_hover_text("unscanned, scanned, ignored, despawned");
                ui.text_edit_multiline(&mut self.signature_form.notes);

                let can_add = !self.signature_form.system_id.is_empty()
                    && !self.signature_form.code.trim().is_empty();
                if ui
                    .add_enabled(can_add, egui::Button::new("Add signature"))
                    .clicked()
                {
                    let signature = Signature {
                        id: self.new_id("sig"),
                        system_id: self.signature_form.system_id.clone(),
                        code: clean(&self.signature_form.code),
                        sig_type: clean(&self.signature_form.sig_type),
                        target: clean(&self.signature_form.target),
                        life: clean(&self.signature_form.life),
                        status: clean(&self.signature_form.status),
                        notes: clean(&self.signature_form.notes),
                        created_at: now(),
                    };
                    self.data.signatures.push(signature);
                    self.signature_form.code.clear();
                    self.signature_form.target.clear();
                    self.signature_form.notes.clear();
                    self.save();
                }
            });
    }

    fn visit_form_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Visit log")
            .default_open(false)
            .show(ui, |ui| {
                system_combo(
                    ui,
                    "System",
                    &self.data.systems,
                    &mut self.visit_form.system_id,
                );
                ui.text_edit_singleline(&mut self.visit_form.pilot);
                ui.text_edit_singleline(&mut self.visit_form.activity)
                    .on_hover_text("scanned, hacked, rolled, observed hostiles...");
                ui.text_edit_multiline(&mut self.visit_form.notes);

                let can_add = !self.visit_form.system_id.is_empty();
                if ui
                    .add_enabled(can_add, egui::Button::new("Log visit"))
                    .clicked()
                {
                    let visit = VisitLog {
                        id: self.new_id("visit"),
                        system_id: self.visit_form.system_id.clone(),
                        pilot: clean(&self.visit_form.pilot),
                        activity: clean(&self.visit_form.activity),
                        notes: clean(&self.visit_form.notes),
                        created_at: now(),
                    };
                    self.data.visits.push(visit);
                    self.visit_form.activity.clear();
                    self.visit_form.notes.clear();
                    self.save();
                }
            });
    }

    fn note_form_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Note")
            .default_open(false)
            .show(ui, |ui| {
                system_combo(
                    ui,
                    "System",
                    &self.data.systems,
                    &mut self.note_form.system_id,
                );
                ui.text_edit_singleline(&mut self.note_form.title);
                ui.text_edit_multiline(&mut self.note_form.body);

                let can_add = !self.note_form.title.trim().is_empty()
                    || !self.note_form.body.trim().is_empty();
                if ui
                    .add_enabled(can_add, egui::Button::new("Add note"))
                    .clicked()
                {
                    let note = Note {
                        id: self.new_id("note"),
                        system_id: self.note_form.system_id.clone(),
                        title: clean(&self.note_form.title),
                        body: clean(&self.note_form.body),
                        created_at: now(),
                    };
                    self.data.notes.push(note);
                    self.note_form.title.clear();
                    self.note_form.body.clear();
                    self.save();
                }
            });
    }

    fn start_authorize_account(&mut self, account_id: String) {
        if self.esi_busy {
            self.status = "An ESI task is already running".to_owned();
            return;
        }
        if !self.save_esi_settings_from_form() {
            return;
        }
        if self.data.esi_settings.client_id.is_empty() {
            self.status = "Set your ESI Client ID before authorizing an account".to_owned();
            return;
        }

        self.esi_busy = true;
        self.status = "Opening browser for ESI authorization...".to_owned();
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
            self.status = "An ESI task is already running".to_owned();
            return;
        }
        let Some(account) = self
            .data
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .cloned()
        else {
            self.status = "Account not found".to_owned();
            return;
        };

        self.esi_busy = true;
        self.status = format!("Syncing {} location from ESI...", account.character_name);
        let tx = self.esi_tx.clone();
        let settings = self.data.esi_settings.clone();
        thread::spawn(move || {
            let result = esi::sync_location(settings, account).map(EsiTaskResult::Location);
            let _ = tx.send(result);
        });
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
                    self.status = format!("ESI failed: {error}");
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.esi_busy = false;
                    self.status = "ESI worker disconnected".to_owned();
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
            self.status = "ESI account authorized".to_owned();
        } else {
            self.status =
                "ESI authorization completed, but the account no longer exists".to_owned();
        }
    }

    fn apply_location_sync(&mut self, location: LocationSync) {
        let system_id = self.ensure_esi_system(
            location.solar_system_id,
            &location.solar_system_name,
            &location.solar_system_class,
        );

        let Some(account_index) = self
            .data
            .accounts
            .iter()
            .position(|account| account.id == location.account_id)
        else {
            self.status = "ESI location synced, but the account no longer exists".to_owned();
            return;
        };

        let previous_system_id = self.data.accounts[account_index].last_system_id.clone();
        let character_name = self.data.accounts[account_index].character_name.clone();

        self.data.accounts[account_index].access_token = Some(location.access_token);
        self.data.accounts[account_index].refresh_token = Some(location.refresh_token);
        self.data.accounts[account_index].token_expires_at = Some(location.token_expires_at);
        self.data.accounts[account_index].last_system_id = system_id.clone();
        self.data.accounts[account_index].last_system_name = location.solar_system_name.clone();
        self.data.accounts[account_index].auth_status = "authorized".to_owned();
        self.data.accounts[account_index].updated_at = now();

        let visit_id = self.new_id("visit");
        self.data.visits.push(VisitLog {
            id: visit_id,
            system_id: system_id.clone(),
            pilot: character_name.clone(),
            activity: "ESI location sync".to_owned(),
            notes: String::new(),
            created_at: now(),
        });

        if !previous_system_id.is_empty() && previous_system_id != system_id {
            self.ensure_connection(&previous_system_id, &system_id, &character_name);
        }

        self.save();
        self.status = format!("Synced {character_name} in {}", location.solar_system_name);
    }

    fn ensure_esi_system(&mut self, solar_system_id: u64, name: &str, class: &str) -> String {
        let esi_id = format!("esi_sys_{solar_system_id}");
        if self.data.systems.iter().any(|system| system.id == esi_id) {
            return esi_id;
        }
        if let Some(existing) = self.data.systems.iter().find(|system| system.name == name) {
            return existing.id.clone();
        }

        self.data.systems.push(SystemNode {
            id: esi_id.clone(),
            name: name.to_owned(),
            class: class.to_owned(),
            effect: String::new(),
            status: "scouted".to_owned(),
            owner: String::new(),
            notes: format!("Added from ESI solar system ID {solar_system_id}"),
            created_at: now(),
        });
        esi_id
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
            notes: format!("Inferred from {character_name} ESI movement"),
            created_at: now(),
        });
    }

    fn central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Chain map");
                ui.label(format!(
                    "{} systems · {} connections · {} signatures",
                    self.data.systems.len(),
                    self.data.connections.len(),
                    self.data.signatures.len()
                ));
            });
            self.chain_map(ui);
            ui.separator();
            self.tabs(ui);
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| match self.active_tab {
                PanelTab::Accounts => self.accounts_list(ui),
                PanelTab::Systems => self.systems_list(ui),
                PanelTab::Connections => self.connections_list(ui),
                PanelTab::Signatures => self.signatures_list(ui),
                PanelTab::Visits => self.visits_list(ui),
                PanelTab::Notes => self.notes_list(ui),
            });
        });
    }

    fn chain_map(&self, ui: &mut egui::Ui) {
        let desired = Vec2::new(ui.available_width(), 360.0);
        let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 12.0, Color32::from_rgb(6, 17, 22));
        painter.rect_stroke(
            rect,
            12.0,
            Stroke::new(1.0_f32, Color32::from_rgb(34, 78, 90)),
            egui::StrokeKind::Inside,
        );

        if self.data.systems.is_empty() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Add your home system to start mapping the chain",
                FontId::proportional(18.0),
                Color32::from_rgb(130, 165, 173),
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
                painter.circle_stroke(
                    *pos,
                    38.0,
                    Stroke::new(2.0_f32, Color32::from_rgb(93, 242, 255)),
                );
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
                    Color32::from_rgb(150, 180, 188),
                );
            }
        }
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            tab_button(ui, &mut self.active_tab, PanelTab::Accounts, "Accounts");
            tab_button(ui, &mut self.active_tab, PanelTab::Systems, "Systems");
            tab_button(
                ui,
                &mut self.active_tab,
                PanelTab::Connections,
                "Connections",
            );
            tab_button(ui, &mut self.active_tab, PanelTab::Signatures, "Signatures");
            tab_button(ui, &mut self.active_tab, PanelTab::Visits, "Visits");
            tab_button(ui, &mut self.active_tab, PanelTab::Notes, "Notes");
        });
    }

    fn accounts_list(&mut self, ui: &mut egui::Ui) {
        let mut delete_id = None;
        let mut authorize_id = None;
        let mut sync_id = None;
        for account in &self.data.accounts {
            record_frame(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(&account.character_name);
                    pill(ui, &account.auth_status);
                    if !account.last_system_name.is_empty() {
                        pill(ui, &account.last_system_name);
                    }
                    if ui
                        .add_enabled(!self.esi_busy, egui::Button::new("Authorize ESI"))
                        .clicked()
                    {
                        authorize_id = Some(account.id.clone());
                    }
                    if ui
                        .add_enabled(
                            !self.esi_busy && account.auth_status == "authorized",
                            egui::Button::new("Sync location"),
                        )
                        .clicked()
                    {
                        sync_id = Some(account.id.clone());
                    }
                    if ui.small_button("Delete").clicked() {
                        delete_id = Some(account.id.clone());
                    }
                });
                if !account.character_id.is_empty() {
                    ui.label(format!("Character ID: {}", account.character_id));
                }
                if account.auth_status == "unauthorized" {
                    ui.label("Not authorized with ESI yet.");
                }
                if account.auth_status == "authorized" && account.refresh_token.is_some() {
                    ui.label("ESI tokens are stored locally in SQLite.");
                }
            });
        }
        if self.data.accounts.is_empty() {
            ui.label("No accounts yet. Add a character from the Account form.");
        }
        if let Some(id) = delete_id {
            self.data.accounts.retain(|account| account.id != id);
            self.save();
        }
        if let Some(id) = authorize_id {
            self.start_authorize_account(id);
        }
        if let Some(id) = sync_id {
            self.start_sync_location(id);
        }
    }

    fn systems_list(&mut self, ui: &mut egui::Ui) {
        let mut delete_id = None;
        for system in &self.data.systems {
            record_frame(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(&system.name);
                    pill(ui, &system.class);
                    pill(ui, &system.status);
                    if !system.effect.is_empty() {
                        pill(ui, &system.effect);
                    }
                    if ui.small_button("Delete").clicked() {
                        delete_id = Some(system.id.clone());
                    }
                });
                if !system.owner.is_empty() {
                    ui.label(format!("Owner/occupants: {}", system.owner));
                }
                if !system.notes.is_empty() {
                    ui.label(&system.notes);
                }
            });
        }
        if self.data.systems.is_empty() {
            ui.label("No systems yet.");
        }
        if let Some(id) = delete_id {
            self.delete_system(&id);
        }
    }

    fn connections_list(&mut self, ui: &mut egui::Ui) {
        let mut delete_id = None;
        for connection in &self.data.connections {
            record_frame(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(format!(
                        "{} → {}",
                        self.system_name(&connection.from_system),
                        self.system_name(&connection.to_system)
                    ));
                    pill(ui, &connection.mass_status);
                    pill(ui, &connection.life_status);
                    if ui.small_button("Delete").clicked() {
                        delete_id = Some(connection.id.clone());
                    }
                });
                ui.label(format!(
                    "Sigs: {} / {}",
                    empty_dash(&connection.signature_from),
                    empty_dash(&connection.signature_to)
                ));
                if !connection.ship_size.is_empty() {
                    ui.label(format!("Ship size: {}", connection.ship_size));
                }
                if !connection.notes.is_empty() {
                    ui.label(&connection.notes);
                }
            });
        }
        if self.data.connections.is_empty() {
            ui.label("No connections yet.");
        }
        if let Some(id) = delete_id {
            self.data
                .connections
                .retain(|connection| connection.id != id);
            self.save();
        }
    }

    fn signatures_list(&mut self, ui: &mut egui::Ui) {
        let mut delete_id = None;
        for signature in &self.data.signatures {
            record_frame(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(format!(
                        "{} · {}",
                        signature.code,
                        self.system_name(&signature.system_id)
                    ));
                    pill(ui, &signature.sig_type);
                    pill(ui, &signature.life);
                    pill(ui, &signature.status);
                    if ui.small_button("Delete").clicked() {
                        delete_id = Some(signature.id.clone());
                    }
                });
                if !signature.target.is_empty() {
                    ui.label(format!("Target: {}", signature.target));
                }
                if !signature.notes.is_empty() {
                    ui.label(&signature.notes);
                }
            });
        }
        if self.data.signatures.is_empty() {
            ui.label("No signatures yet.");
        }
        if let Some(id) = delete_id {
            self.data.signatures.retain(|signature| signature.id != id);
            self.save();
        }
    }

    fn visits_list(&mut self, ui: &mut egui::Ui) {
        let mut delete_id = None;
        for visit in &self.data.visits {
            record_frame(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(format!(
                        "{} · {}",
                        empty_dash(&visit.activity),
                        self.system_name(&visit.system_id)
                    ));
                    if !visit.pilot.is_empty() {
                        pill(ui, &visit.pilot);
                    }
                    if ui.small_button("Delete").clicked() {
                        delete_id = Some(visit.id.clone());
                    }
                });
                if !visit.notes.is_empty() {
                    ui.label(&visit.notes);
                }
            });
        }
        if self.data.visits.is_empty() {
            ui.label("No visits yet.");
        }
        if let Some(id) = delete_id {
            self.data.visits.retain(|visit| visit.id != id);
            self.save();
        }
    }

    fn notes_list(&mut self, ui: &mut egui::Ui) {
        let mut delete_id = None;
        for note in &self.data.notes {
            record_frame(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(if note.title.is_empty() {
                        "Untitled note"
                    } else {
                        &note.title
                    });
                    if !note.system_id.is_empty() {
                        pill(ui, &self.system_name(&note.system_id));
                    }
                    if ui.small_button("Delete").clicked() {
                        delete_id = Some(note.id.clone());
                    }
                });
                if !note.body.is_empty() {
                    ui.label(&note.body);
                }
            });
        }
        if self.data.notes.is_empty() {
            ui.label("No notes yet.");
        }
        if let Some(id) = delete_id {
            self.data.notes.retain(|note| note.id != id);
            self.save();
        }
    }

    fn delete_system(&mut self, id: &str) {
        self.data.systems.retain(|system| system.id != id);
        self.data
            .connections
            .retain(|connection| connection.from_system != id && connection.to_system != id);
        self.data
            .signatures
            .retain(|signature| signature.system_id != id);
        self.data.visits.retain(|visit| visit.system_id != id);
        self.data.notes.retain(|note| note.system_id != id);
        self.save();
    }
}

impl eframe::App for NomadeerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_esi_tasks();
        self.top_bar(ctx);
        self.side_panel(ctx);
        self.central_panel(ctx);
    }
}
