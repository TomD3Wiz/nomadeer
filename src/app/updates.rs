//! New releases on screen: the pill in the top bar, the panel it opens, the
//! update settings, the one-time note that Nomadeer checks for updates, and
//! the download and hand-over once the player presses Update.

use super::NomadeerApp;
use crate::config::theme;
use crate::self_update::{self, Prepared, Sources, Stage, UpdateError};
use crate::updates::{
    self, CURRENT_VERSION, CheckOutcome, Edition, InstallKind, NoteBlock, Release, UpdateRoute,
    UpdateState,
};
use crate::util::now;
use eframe::egui::{self, RichText};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// The first automatic check waits this long after launch, so it never competes
/// with loading the chain and signing characters in.
const STARTUP_DELAY: Duration = Duration::from_secs(5);
const NOTICE_TEXT: &str =
    "Nomadeer checks GitHub once a day for updates. You can turn this off in Settings.";
const NOTES_HEIGHT: f32 = 280.0;
/// How often the panel redraws while an update downloads.
const PROGRESS_REPAINT: Duration = Duration::from_millis(100);

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
    /// The update the player started, while it downloads and installs.
    install: Option<InstallJob>,
    /// Why the last update the player started did not finish.
    install_error: Option<UpdateError>,
    /// A new AppImage is in place but this copy could not start it.
    installed_version: Option<String>,
}

struct InstallJob {
    version: String,
    stage: Stage,
    cancel: Arc<AtomicBool>,
    rx: Receiver<InstallEvent>,
}

enum InstallEvent {
    Stage(Stage),
    Finished(Result<Prepared, UpdateError>),
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
            install: None,
            install_error: None,
            installed_version: None,
        }
    }

    fn checking(&self) -> bool {
        self.in_flight.is_some()
    }
}

impl NomadeerApp {
    pub(super) fn load_update_state(&mut self) {
        self_update::clean_up_after_updates(self.updates.install_kind);
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
            let installing = self.updates.install.is_some();
            if ui
                .add_enabled(
                    !skipped && !installing,
                    egui::Button::new("Skip this version"),
                )
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
            (UpdateRoute::InApp, Some(file)) => self.in_app_update(ui, release, &file),
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

    /// Update, its progress, or what went wrong, for copies that update
    /// themselves.
    fn in_app_update(&mut self, ui: &mut egui::Ui, release: &Release, file: &str) {
        if let Some(version) = &self.updates.installed_version {
            ui.label(format!(
                "Nomadeer {version} is installed. Close Nomadeer and start it again to use it."
            ));
            return;
        }
        if let Some(job) = &self.updates.install {
            let (fraction, text) = match job.stage {
                Stage::Downloading { done, total } => {
                    let fraction = if total == 0 {
                        0.0
                    } else {
                        done as f32 / total as f32
                    };
                    (
                        fraction,
                        format!("Downloading {file} · {:.0}%", fraction * 100.0),
                    )
                }
                Stage::Installing => (
                    1.0,
                    match self.updates.install_kind {
                        InstallKind::AppImage => "Putting the new AppImage in place...".to_owned(),
                        _ => "Starting the installer...".to_owned(),
                    },
                ),
            };
            ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).text(text));
            // Once the download checks out it is put in place without stopping.
            let downloading = matches!(job.stage, Stage::Downloading { .. });
            if ui
                .add_enabled(downloading, egui::Button::new("Cancel"))
                .clicked()
            {
                job.cancel.store(true, Ordering::Relaxed);
            }
            return;
        }

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
        ui.label(RichText::new(self.update_explainer(file)).color(theme::muted_text()));
        if let Some(error) = &self.updates.install_error {
            let color = if error.is_failure() {
                super::WARNING_TEXT
            } else {
                theme::muted_text()
            };
            ui.label(RichText::new(error.message()).color(color));
        }
    }

    /// What pressing Update will do.
    fn update_explainer(&self, file: &str) -> String {
        let kept = "Your characters, chains and Umbra are kept.";
        match self.updates.install_kind {
            InstallKind::WindowsInstaller { per_user } => {
                let permission = if per_user {
                    ""
                } else {
                    " Windows asks for permission first, as Nomadeer is installed for all users."
                };
                format!(
                    "Downloads {file}, checks it and installs it. Nomadeer closes while it installs and opens again when it is done.{permission} {kept}"
                )
            }
            InstallKind::AppImage => format!(
                "Downloads {file}, checks it and puts it in place of this AppImage under the same name, then restarts Nomadeer. {kept}"
            ),
            _ => format!("Gets {file}. {kept}"),
        }
    }

    /// The player pressed Update: download and check the file in the background.
    fn start_update(&mut self, release: &Release) {
        if self.updates.install.is_some() {
            return;
        }
        self.updates.install_error = None;
        let exe = std::env::current_exe().ok();
        let appimage = self_update::current_appimage();
        let sources = Sources::current();
        let plan = match self_update::plan_update(
            release,
            self.updates.install_kind,
            self.updates.edition,
            exe.as_deref(),
            appimage.as_deref(),
            &sources,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                self.updates.install_error = Some(error);
                return;
            }
        };
        let total = plan.file.size;
        let downloads = self_update::downloads_dir();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("update-install".to_owned())
            .spawn(move || {
                let progress = tx.clone();
                let result = self_update::prepare_update(
                    &plan,
                    &downloads,
                    &sources,
                    &worker_cancel,
                    &mut |stage| {
                        let _ = progress.send(InstallEvent::Stage(stage));
                    },
                );
                let _ = tx.send(InstallEvent::Finished(result));
            });
        match spawned {
            Ok(_) => {
                self.updates.install = Some(InstallJob {
                    version: release.version.clone(),
                    stage: Stage::Downloading { done: 0, total },
                    cancel,
                    rx,
                });
            }
            Err(error) => {
                self.updates.install_error = Some(UpdateError::Launch(error.to_string()));
            }
        }
    }

    /// Follows the update the player started, and hands over to the new
    /// version once it is ready.
    pub(super) fn poll_update_install(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.updates.install else {
            return;
        };
        let finished = loop {
            match job.rx.try_recv() {
                Ok(InstallEvent::Stage(stage)) => job.stage = stage,
                Ok(InstallEvent::Finished(result)) => break Some(result),
                Err(TryRecvError::Empty) => break None,
                Err(TryRecvError::Disconnected) => {
                    break Some(Err(UpdateError::Launch(
                        "the download stopped unexpectedly".to_owned(),
                    )));
                }
            }
        };
        let Some(result) = finished else {
            ctx.request_repaint_after(PROGRESS_REPAINT);
            return;
        };
        let version = job.version.clone();
        self.updates.install = None;
        match result {
            Ok(prepared) => self.hand_over(ctx, &version, prepared),
            Err(error) => {
                self.updates.install_error = Some(error);
                self.updates.show_panel = true;
            }
        }
    }

    /// Saves everything, starts the new version and closes this one.
    fn hand_over(&mut self, ctx: &egui::Context, version: &str, prepared: Prepared) {
        let saved = self.store.save(&self.data);
        self.save_update_state();
        match prepared {
            Prepared::Installer {
                path,
                sha256,
                arguments,
                elevate,
            } => {
                if let Err(error) = saved {
                    self.updates.install_error = Some(UpdateError::Launch(format!(
                        "Nomadeer could not save first: {error}"
                    )));
                    self.updates.show_panel = true;
                    return;
                }
                match self_update::launch_installer(&path, &sha256, &arguments, elevate) {
                    Ok(()) => {
                        self.status = format!("Installing Nomadeer {version}...");
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(error) => {
                        self.updates.install_error = Some(error);
                        self.updates.show_panel = true;
                    }
                }
            }
            Prepared::AppImage { path } => {
                self.updates.installed_version = Some(version.to_owned());
                if let Err(error) = saved {
                    // The new AppImage is already in place; restarting now
                    // would lose what could not be saved.
                    self.status = format!("Save failed: {error}");
                    self.updates.show_panel = true;
                    return;
                }
                match self_update::relaunch_appimage(&path) {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(error) => {
                        self.status = format!("Could not restart Nomadeer: {error}");
                        self.updates.show_panel = true;
                    }
                }
            }
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
