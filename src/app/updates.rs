//! New releases on screen: the pill in the top bar, the panel it opens, the
//! update settings and the one-time note that Nomadeer checks for updates.

use super::NomadeerApp;
use crate::config::theme;
use crate::updates::{
    self, CURRENT_VERSION, CheckOutcome, Edition, InstallKind, NoteBlock, Release, UpdateRoute,
    UpdateState,
};
use crate::util::now;
use eframe::egui::{self, RichText};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// The first automatic check waits this long after launch, so it never competes
/// with loading the chain and signing characters in.
const STARTUP_DELAY: Duration = Duration::from_secs(5);
const NOTICE_TEXT: &str =
    "Nomadeer checks GitHub once a day for updates. You can turn this off in Settings.";
const NOTES_HEIGHT: f32 = 280.0;

pub(super) struct UpdateUi {
    pub(super) state: UpdateState,
    install_kind: InstallKind,
    edition: Edition,
    launched: Instant,
    /// A check is running; true when the player asked for it.
    in_flight: Option<bool>,
    tx: Sender<CheckOutcome>,
    rx: Receiver<CheckOutcome>,
    show_panel: bool,
    /// The player pressed Later: no pill until the next launch.
    later: bool,
    /// The one-time note about update checks is on screen.
    show_notice: bool,
    /// How the last check the player asked for went, and whether it failed.
    message: Option<(String, bool)>,
}

impl UpdateUi {
    pub(super) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            state: UpdateState::default(),
            install_kind: updates::current_install_kind(),
            edition: Edition::current(),
            launched: Instant::now(),
            in_flight: None,
            tx,
            rx,
            show_panel: false,
            later: false,
            show_notice: false,
            message: None,
        }
    }

    fn checking(&self) -> bool {
        self.in_flight.is_some()
    }
}

impl NomadeerApp {
    pub(super) fn load_update_state(&mut self) {
        match self.store.load_update_state() {
            Ok(state) => self.updates.state = state,
            Err(error) => {
                self.status = format!("Could not load update settings: {error}");
                return;
            }
        }
        if !self.updates.state.notice_shown {
            // Shown on this launch only; it stays until dismissed.
            self.updates.show_notice = true;
            self.updates.state.notice_shown = true;
            self.save_update_state();
        }
    }

    fn save_update_state(&mut self) {
        if let Err(error) = self.store.save_update_state(&self.updates.state) {
            self.status = format!("Save failed: {error}");
        }
    }

    /// Starts the daily check once Nomadeer has settled in after launch.
    pub(super) fn maybe_check_for_updates(&mut self) {
        if !self.updates.checking()
            && self.updates.launched.elapsed() >= STARTUP_DELAY
            && self.updates.state.automatic_check_due(now())
        {
            self.start_update_check(false);
        }
    }

    fn start_update_check(&mut self, manual: bool) {
        if self.updates.checking() {
            return;
        }
        if manual && let Some(wait) = self.updates.state.rate_limit_wait(now()) {
            self.updates.message = Some((
                format!(
                    "GitHub asked Nomadeer to wait. Try again in {} min.",
                    wait.div_ceil(60)
                ),
                true,
            ));
            return;
        }
        let url = updates::latest_release_url();
        let etag = self.updates.state.request_etag().map(str::to_owned);
        let tx = self.updates.tx.clone();
        let spawned = thread::Builder::new()
            .name("update-check".to_owned())
            .spawn(move || {
                let _ = tx.send(updates::check_latest(&url, etag.as_deref(), now()));
            });
        match spawned {
            Ok(_) => {
                self.updates.in_flight = Some(manual);
                if manual {
                    self.updates.message = None;
                }
            }
            Err(error) => {
                if manual {
                    self.updates.message =
                        Some((format!("Could not check for updates: {error}"), true));
                }
            }
        }
    }

    pub(super) fn poll_update_check(&mut self) {
        let outcome = match self.updates.rx.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.updates.in_flight = None;
                return;
            }
        };
        let manual = self.updates.in_flight.take().unwrap_or(false);
        let failure = match &outcome {
            CheckOutcome::Failed { error } => Some(format!("Could not check for updates. {error}")),
            CheckOutcome::RateLimited { until } => Some(format!(
                "GitHub asked Nomadeer to wait. Try again in {} min.",
                until.saturating_sub(now()).div_ceil(60)
            )),
            _ => None,
        };
        self.updates.state.apply(outcome, now());
        self.save_update_state();
        // Automatic checks only speak up through the pill; a failed one tries
        // again later without a word.
        if !manual {
            return;
        }
        self.updates.message = Some(match failure {
            Some(error) => (error, true),
            None => match self.updates.state.newer_release(CURRENT_VERSION) {
                Some(release) => {
                    // Asking again brings back a release put off with Later.
                    self.updates.later = false;
                    (format!("Nomadeer {} is available.", release.version), false)
                }
                None => (
                    format!("You have the latest Nomadeer ({CURRENT_VERSION})."),
                    false,
                ),
            },
        });
    }

    /// The pill in the top bar while a release is waiting.
    pub(super) fn update_pill(&mut self, ui: &mut egui::Ui) {
        if self.updates.later {
            return;
        }
        let Some(release) = self.updates.state.offered_update(CURRENT_VERSION) else {
            return;
        };
        let text = RichText::new(format!("Nomadeer {} is available", release.version))
            .color(theme::accent());
        if ui
            .add(egui::Button::new(text).stroke(egui::Stroke::new(1.0_f32, theme::accent())))
            .on_hover_text("See what's new")
            .clicked()
        {
            self.updates.show_panel = true;
        }
    }

    /// The one-time line under the top bar saying that Nomadeer checks for updates.
    pub(super) fn update_notice_bar(&mut self, ctx: &egui::Context) {
        if !self.updates.show_notice {
            return;
        }
        egui::TopBottomPanel::top("update_notice").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(NOTICE_TEXT).color(theme::muted_text()));
                if ui.small_button("Settings").clicked() {
                    self.show_settings = true;
                    self.updates.show_notice = false;
                }
                if ui.small_button("Dismiss").clicked() {
                    self.updates.show_notice = false;
                }
            });
        });
    }

    /// The Updates part of the Settings window.
    pub(super) fn update_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Updates");
        let mut automatic = self.updates.state.check_automatically;
        if ui
            .checkbox(&mut automatic, "Check for updates once a day")
            .changed()
        {
            self.updates.state.check_automatically = automatic;
            self.save_update_state();
        }
        ui.label(
            RichText::new(
                "Nomadeer looks on GitHub for a new release. Nothing is installed until you press Update.",
            )
            .color(theme::muted_text()),
        );

        ui.horizontal_wrapped(|ui| {
            let checking = self.updates.checking();
            let label = if checking { "Checking..." } else { "Check now" };
            if ui
                .add_enabled(!checking, egui::Button::new(label))
                .clicked()
            {
                self.start_update_check(true);
            }
            let last_checked = match self.updates.state.last_checked_at {
                Some(at) => format!(
                    "Last checked {}",
                    Self::format_duration_ago(now().saturating_sub(at))
                ),
                None => "Not checked yet".to_owned(),
            };
            ui.label(RichText::new(last_checked).color(theme::muted_text()));
        });
        if let Some(release) = self.updates.state.newer_release(CURRENT_VERSION) {
            let text = RichText::new(format!("Nomadeer {} is available", release.version))
                .color(theme::accent());
            if ui.link(text).clicked() {
                self.updates.show_panel = true;
            }
        }
        if let Some((message, failed)) = &self.updates.message {
            let color = if *failed {
                super::WARNING_TEXT
            } else {
                theme::muted_text()
            };
            ui.label(RichText::new(message).color(color));
        }
        ui.label(RichText::new(self.running_copy()).color(theme::muted_text()));
    }

    /// `You have Nomadeer 0.1.0 (AppImage, optimized edition).`
    fn running_copy(&self) -> String {
        let kind = self.updates.install_kind.label();
        let edition = self.updates.edition.label();
        if kind.is_empty() {
            format!("You have Nomadeer {CURRENT_VERSION} ({edition} edition).")
        } else {
            format!("You have Nomadeer {CURRENT_VERSION} ({kind}, {edition} edition).")
        }
    }

    /// The panel the pill opens: what is new and how to get it.
    pub(super) fn update_window(&mut self, ctx: &egui::Context) {
        if !self.updates.show_panel {
            return;
        }
        let Some(release) = self.updates.state.newer_release(CURRENT_VERSION).cloned() else {
            self.updates.show_panel = false;
            return;
        };
        let mut open = true;
        egui::Window::new(format!("Nomadeer {}", release.version))
            .id(egui::Id::new("update_panel"))
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_width(480.0)
            .show(ctx, |ui| self.update_panel_contents(ui, &release));
        if !open {
            self.updates.show_panel = false;
        }
    }

    fn update_panel_contents(&mut self, ui: &mut egui::Ui, release: &Release) {
        ui.heading(format!("Nomadeer {} is available", release.version));
        let released = match release.published_date() {
            Some(date) => format!("Released {date} · you have {CURRENT_VERSION}"),
            None => format!("You have {CURRENT_VERSION}"),
        };
        ui.label(RichText::new(released).color(theme::muted_text()));
        ui.separator();

        egui::ScrollArea::vertical()
            .max_height(NOTES_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| release_notes(ui, &release.notes));
        ui.separator();

        self.update_route(ui, release);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let skipped = self.updates.state.skipped_version == release.version;
            if ui
                .add_enabled(!skipped, egui::Button::new("Skip this version"))
                .on_hover_text("No reminders until the next release")
                .clicked()
            {
                self.updates.state.skipped_version = release.version.clone();
                self.save_update_state();
                self.updates.show_panel = false;
                self.status = format!("Skipped Nomadeer {}", release.version);
            }
            if ui
                .button("Later")
                .on_hover_text("Remind me next time Nomadeer starts")
                .clicked()
            {
                self.updates.later = true;
                self.updates.show_panel = false;
            }
        });
    }

    /// How this copy gets the release, which depends on how it was installed.
    fn update_route(&mut self, ui: &mut egui::Ui, release: &Release) {
        let kind = self.updates.install_kind;
        let choice = updates::pick_asset(release, kind, self.updates.edition);
        let file = choice.map(|choice| choice.file.name.clone());
        let route = match (kind.update_route(), &file) {
            // A release without this copy's file can still be fetched by hand.
            (UpdateRoute::InApp | UpdateRoute::FlatpakBundle, None) => UpdateRoute::Download,
            (route, _) => route,
        };

        match (route, file) {
            (UpdateRoute::InApp, Some(file)) => {
                ui.horizontal_wrapped(|ui| {
                    let update = egui::Button::new(RichText::new("Update").strong())
                        .stroke(egui::Stroke::new(1.0_f32, theme::accent()));
                    if ui.add(update).clicked() {
                        self.start_update(release);
                    }
                    if ui.button("Open download page").clicked() {
                        self.open_external_url(release.download_page());
                    }
                });
                ui.label(
                    RichText::new(format!(
                        "Gets {file}. Your characters, chains and Umbra are kept."
                    ))
                    .color(theme::muted_text()),
                );
            }
            (UpdateRoute::FlatpakBundle, Some(file)) => {
                ui.label(format!("Download {file}, then install it with:"));
                let command = updates::flatpak_install_command(&file);
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(&command);
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(command.clone());
                        self.status = "Copied the install command".to_owned();
                    }
                });
                if ui.button("Open download page").clicked() {
                    self.open_external_url(release.download_page());
                }
                ui.label(
                    RichText::new(
                        "Once Nomadeer is on Flathub, updates will arrive with your other apps.",
                    )
                    .color(theme::muted_text()),
                );
            }
            (UpdateRoute::Download, file) => {
                if let Some(file) = file {
                    ui.label(self.manual_update_steps(&file));
                } else {
                    ui.label("Pick the download for your system from the release page.");
                }
                if ui.button("Open download page").clicked() {
                    self.open_external_url(release.download_page());
                }
            }
            (UpdateRoute::NoticeOnly, _) | (_, None) => {
                let note = if kind == InstallKind::Development {
                    "This copy was built from source, so it does not update itself."
                } else {
                    "There is no download for this system yet; the release page has the source."
                };
                ui.label(RichText::new(note).color(theme::muted_text()));
                if ui.button("Open release page").clicked() {
                    self.open_external_url(release.download_page());
                }
            }
        }
    }

    /// What to download and what to do with it, for copies updated by hand.
    fn manual_update_steps(&self, file: &str) -> String {
        match (self.updates.install_kind, self.updates.edition) {
            (InstallKind::WindowsPortable, edition) => {
                let exe = match edition {
                    Edition::Optimized => "Nomadeer.exe",
                    Edition::Compatible => "Nomadeer-compatible.exe",
                };
                format!(
                    "Download {file} and copy what is inside over your Nomadeer folder, keeping the data folder. Then start {exe} as before."
                )
            }
            (InstallKind::LinuxTarball, _) => {
                format!("Download {file} and replace your nomadeer program with the one inside.")
            }
            _ => format!("Download {file} from the release page."),
        }
    }

    /// The player pressed Update.
    // TODO(stage 3): download, verify against the .sha256 and install in place
    // for the Windows installer and AppImage. Until then the file is fetched
    // through the browser.
    fn start_update(&mut self, release: &Release) {
        let Some(choice) =
            updates::pick_asset(release, self.updates.install_kind, self.updates.edition)
        else {
            self.open_external_url(release.download_page());
            return;
        };
        let file = choice.file.name.clone();
        match crate::util::open_url(&choice.file.download_url) {
            Ok(()) => {
                self.status = match self.updates.install_kind {
                    InstallKind::AppImage => {
                        format!("Downloading {file}. Use it in place of your current AppImage.")
                    }
                    _ => format!("Downloading {file}. Run it to update Nomadeer."),
                };
                self.updates.show_panel = false;
            }
            Err(error) => self.status = error,
        }
    }
}

/// Release notes as headings, paragraphs and bullets.
fn release_notes(ui: &mut egui::Ui, notes: &str) {
    let blocks = updates::note_blocks(notes);
    if blocks.is_empty() {
        ui.label(RichText::new("The release page lists what changed.").color(theme::muted_text()));
        return;
    }
    for block in blocks {
        match block {
            NoteBlock::Heading(text) => {
                ui.add_space(4.0);
                ui.label(RichText::new(text).strong());
            }
            NoteBlock::Paragraph(text) => {
                ui.add(egui::Label::new(text).wrap());
            }
            NoteBlock::Bullet(text) => {
                ui.horizontal_top(|ui| {
                    ui.label("•");
                    ui.add(egui::Label::new(text).wrap());
                });
            }
        }
    }
}
