//! Umbra on screen: her voice in the intel panels, and her own panel beside the
//! dashboard tabs with a feed of everything she has briefed.

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Sense, Stroke, Vec2};

use super::{NomadeerApp, UmbraTask, WARNING_TEXT};
use crate::config::theme;
use crate::local_intel::{self, CoreStatus, DownloadProgress};
use crate::model::{SystemAssessment, SystemNode, UmbraBriefing};
use crate::util::now;
use crate::widgets::pill;

/// Her portrait: Aura's hologram half taken by Sleeper plating, one eye gone amber.
/// Square; larger art is scaled down on load.
const AVATAR_PNG: &[u8] = include_bytes!("../../assets/umbra/umbra-avatar.png");
/// She is never drawn larger than 96 points, so twice that covers high-DPI screens.
const AVATAR_TEXTURE_SIZE: u32 = 192;
const HEADER_AVATAR_SIZE: f32 = 96.0;
/// Her portrait when her panel is narrow or short.
const COMPACT_HEADER_AVATAR_SIZE: f32 = 72.0;
/// Below this width or height her panel uses the compact portrait.
const COMPACT_PANEL_WIDTH: f32 = 420.0;
const COMPACT_PANEL_HEIGHT: f32 = 360.0;
const MESSAGE_AVATAR_SIZE: f32 = 30.0;
/// Room the feed always gets, about one briefing; on a short window her panel opens
/// scrolled down to it, header above the fold, instead of squeezing it away.
const MIN_FEED_HEIGHT: f32 = 120.0;
/// Briefings kept on screen; the feed itself holds more until they age out.
pub(super) const FEED_SHOWN: usize = 200;
const QUIET_NOTE: &str = "Public killmails may be delayed or absent; quiet never means safe.";
const EMPTY_FEED_LINE: &str = "Nothing assessed yet. Fly a recon pilot into a system, or press Ask Umbra in Intel, and I will tell you what the evidence says.";
const INSTALL_LINE: &str = "I am not aboard yet. Bring my core onto this computer and I will brief you on every system your pilots fly into.";
const REPAIR_LINE: &str = "My core is damaged and I will not brief you from it. Let me pull a clean copy and I will be back on watch.";

/// The card in her panel that brings her aboard, while she cannot brief.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InstallCard {
    /// Her core is not on this computer.
    Install,
    /// Her core failed its check or could not be loaded.
    Repair,
}

impl InstallCard {
    /// The card her panel shows, if any: none once her core is in place, even
    /// before it has been checked.
    pub(super) fn for_core(core: CoreStatus, needs_repair: bool) -> Option<Self> {
        if needs_repair {
            Some(Self::Repair)
        } else if core == CoreStatus::Missing {
            Some(Self::Install)
        } else {
            None
        }
    }

    fn line(self) -> &'static str {
        match self {
            Self::Install => INSTALL_LINE,
            Self::Repair => REPAIR_LINE,
        }
    }

    fn button(self) -> &'static str {
        match self {
            Self::Install => "Install Umbra",
            Self::Repair => "Repair Umbra",
        }
    }

    /// What it costs and where she runs.
    fn details(self, size: &str) -> String {
        match self {
            Self::Install => {
                format!("A one-time download of about {size}. She runs entirely on this computer.")
            }
            Self::Repair => format!(
                "Whatever is still sound is kept; the rest is downloaded again, up to {size}. She runs entirely on this computer."
            ),
        }
    }
}

/// How far an install has got, and the words on its progress bar.
pub(super) fn progress_bar_text(progress: &DownloadProgress) -> (f32, String) {
    let fraction = if progress.total == 0 {
        0.0
    } else {
        (progress.done as f64 / progress.total as f64).clamp(0.0, 1.0) as f32
    };
    let megabytes = |bytes: u64| (bytes as f64 / 1_000_000.0).round() as u64;
    let text = format!(
        "{} · {} of {} MB · {:.0}%",
        progress.label,
        megabytes(progress.done),
        megabytes(progress.total),
        fraction * 100.0
    );
    (fraction, text)
}

/// Whose words an assessment shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BriefingVoice {
    /// A briefing Umbra wrote and Nomadeer accepted.
    Umbra,
    /// The plain evidence summary: she has not briefed it, or her briefing was set aside.
    Summary,
}

impl BriefingVoice {
    pub(super) fn of(assessment: &SystemAssessment) -> Self {
        if assessment.model_generated && !assessment.briefing.trim().is_empty() {
            Self::Umbra
        } else {
            Self::Summary
        }
    }

    pub(super) fn ask_label(self) -> &'static str {
        match self {
            Self::Umbra => "Ask Umbra again",
            Self::Summary => "Ask Umbra",
        }
    }
}

pub(super) fn evidence_heading(evidence: &[String]) -> String {
    format!("Evidence ({})", evidence.len())
}

/// Decodes her portrait as a square, cropped to the centre, no larger than `max_side`.
pub(super) fn avatar_image(bytes: &[u8], max_side: u32) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(bytes).ok()?;
    let side = image.width().min(image.height());
    if side == 0 {
        return None;
    }
    let mut square = image.crop_imm(
        (image.width() - side) / 2,
        (image.height() - side) / 2,
        side,
        side,
    );
    if side > max_side {
        square = square.resize_exact(max_side, max_side, image::imageops::FilterType::Lanczos3);
    }
    let rgba = square.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        rgba.as_raw(),
    ))
}

/// The class pill a feed entry keeps: the w-space class, or the known-space band with
/// its security status.
pub(super) fn feed_class_label(system: &SystemNode) -> String {
    let class = NomadeerApp::inferred_system_class(system);
    if local_intel::is_wormhole_space(system) || class == "Unknown" {
        class
    } else {
        format!("{class} {:.1}", system.security_status)
    }
}

/// The feed entry for a briefing she just wrote.
pub(super) fn feed_entry(
    system: Option<&SystemNode>,
    assessment: &SystemAssessment,
    briefed_at: u64,
    auto: bool,
) -> UmbraBriefing {
    UmbraBriefing {
        id: 0,
        solar_system_id: system.map_or(0, |system| system.solar_system_id),
        system_id: assessment.system_id.clone(),
        system_name: system.map_or_else(|| "Unknown".to_owned(), |system| system.name.clone()),
        system_class: system.map(feed_class_label).unwrap_or_default(),
        threat_score: assessment.threat_score,
        activity_score: assessment.activity_score,
        confidence: assessment.confidence.clone(),
        briefing: assessment.briefing.clone(),
        assessed_at: assessment.generated_at,
        briefed_at,
        auto,
    }
}

/// Whether the feed already holds a briefing of the same assessment.
pub(super) fn feed_has(feed: &[UmbraBriefing], entry: &UmbraBriefing) -> bool {
    feed.iter().any(|existing| {
        existing.system_id == entry.system_id && existing.assessed_at == entry.assessed_at
    })
}

/// Adds the entry at the newest end unless it repeats one already there, dropping the
/// oldest beyond what is shown. Returns whether it was added.
pub(super) fn push_feed_entry(feed: &mut Vec<UmbraBriefing>, entry: UmbraBriefing) -> bool {
    if feed_has(feed, &entry) {
        return false;
    }
    feed.push(entry);
    if feed.len() > FEED_SHOWN {
        let excess = feed.len() - FEED_SHOWN;
        feed.drain(..excess);
    }
    true
}

/// Finds where a feed entry's system sits on the chain map now.
pub(super) struct ChainLookup<'a> {
    by_solar_system: HashMap<u64, &'a str>,
    by_id: HashSet<&'a str>,
}

impl<'a> ChainLookup<'a> {
    pub(super) fn new(systems: &'a [SystemNode]) -> Self {
        let mut by_solar_system = HashMap::new();
        for system in systems.iter().filter(|system| system.solar_system_id != 0) {
            by_solar_system
                .entry(system.solar_system_id)
                .or_insert(system.id.as_str());
        }
        Self {
            by_solar_system,
            by_id: systems.iter().map(|system| system.id.as_str()).collect(),
        }
    }

    /// The map node for the entry's solar system, wherever it sits on the map now; the
    /// node she briefed when the solar system is not known.
    pub(super) fn target(&self, entry: &UmbraBriefing) -> Option<&'a str> {
        if entry.solar_system_id != 0
            && let Some(id) = self.by_solar_system.get(&entry.solar_system_id)
        {
            return Some(id);
        }
        self.by_id.get(entry.system_id.as_str()).copied()
    }
}

/// "14:32 EVE · 5m ago".
pub(super) fn message_time(briefed_at: u64, current_time: u64) -> String {
    format!(
        "{} EVE · {}",
        local_intel::eve_clock(briefed_at),
        NomadeerApp::format_duration_ago(current_time.saturating_sub(briefed_at))
    )
}

fn score_pill(name: &str, score: u8) -> String {
    format!("{name} {}", local_intel::score_label(score))
}

/// Umbra's words: her name above them and a bronze bar down the left.
pub(super) fn umbra_quote(ui: &mut egui::Ui, text: &str) {
    let response = egui::Frame::new()
        .fill(theme::umbra_background())
        .corner_radius(4.0)
        .inner_margin(egui::Margin {
            left: 14,
            right: 10,
            top: 6,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                egui::RichText::new("UMBRA")
                    .small()
                    .strong()
                    .extra_letter_spacing(2.0)
                    .color(theme::umbra()),
            );
            ui.label(
                egui::RichText::new(text)
                    .italics()
                    .color(theme::umbra_text()),
            );
        })
        .response;
    let rect = response.rect;
    ui.painter().rect_filled(
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.min.x + 3.0, rect.max.y)),
        egui::CornerRadius {
            nw: 4,
            sw: 4,
            ne: 0,
            se: 0,
        },
        theme::umbra(),
    );
}

/// The plain evidence summary, labelled as such.
fn summary_text(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new("Summary")
            .small()
            .strong()
            .color(theme::muted_text()),
    );
    ui.label(text);
}

/// Paints her portrait in a circle with a bronze ring; a plain disc stands in when
/// the portrait cannot be shown.
fn paint_avatar(ui: &mut egui::Ui, texture: Option<&egui::TextureHandle>, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let radius = size / 2.0;
    match texture {
        Some(texture) => egui::Image::from_texture(texture)
            .corner_radius(radius)
            .paint_at(ui, rect),
        None => {
            ui.painter()
                .circle_filled(rect.center(), radius, theme::umbra_background());
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "U",
                egui::FontId::proportional(size * 0.5),
                theme::umbra(),
            );
        }
    }
    ui.painter().circle_stroke(
        rect.center(),
        radius - 0.5,
        Stroke::new(if size > 48.0 { 2.0_f32 } else { 1.0 }, theme::umbra()),
    );
}

impl NomadeerApp {
    /// Her portrait, loaded once.
    fn umbra_avatar(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        self.umbra_avatar
            .get_or_insert_with(|| {
                avatar_image(AVATAR_PNG, AVATAR_TEXTURE_SIZE).map(|image| {
                    ctx.load_texture(
                        "umbra-avatar",
                        image,
                        egui::TextureOptions {
                            mipmap_mode: Some(egui::TextureFilter::Linear),
                            ..egui::TextureOptions::LINEAR
                        },
                    )
                })
            })
            .clone()
    }

    /// The assessment's words, the Ask Umbra button and the evidence behind them.
    pub(super) fn assessment_briefing(
        &mut self,
        ui: &mut egui::Ui,
        system: &SystemNode,
        assessment: &SystemAssessment,
    ) {
        let voice = BriefingVoice::of(assessment);
        match voice {
            BriefingVoice::Umbra => umbra_quote(ui, &assessment.briefing),
            BriefingVoice::Summary => summary_text(ui, &assessment.briefing),
        }
        let core = local_intel::core_status();
        let umbra_idle = self.umbra_free_for_player();
        let row = Vec2::new(ui.available_width(), ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(
            row,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if core != CoreStatus::Missing && !self.umbra_needs_repair {
                    // An unchecked core is being checked, or is checked on request.
                    if ui
                        .add_enabled(umbra_idle, egui::Button::new(voice.ask_label()))
                        .clicked()
                    {
                        self.start_local_briefing(&system.id);
                    }
                } else if ui
                    .button(if self.umbra_needs_repair {
                        "Repair Umbra"
                    } else {
                        "Install Umbra"
                    })
                    .clicked()
                {
                    self.show_local_intel_settings = true;
                }
            },
        );
        self.umbra_feedback(ui, Some(&system.id));
        egui::CollapsingHeader::new(evidence_heading(&assessment.evidence))
            .id_salt(("assessment_evidence", &system.id))
            .default_open(false)
            .show(ui, |ui| {
                for evidence in &assessment.evidence {
                    ui.label(format!("• {evidence}"));
                }
                ui.label(egui::RichText::new(QUIET_NOTE).color(theme::muted_text()));
            });
    }

    pub(super) fn umbra_auto_brief_toggle(&mut self, ui: &mut egui::Ui) {
        let mut auto_brief = self.data.auto_intel.umbra_briefs_automatically;
        if ui
            .checkbox(&mut auto_brief, "Umbra briefs automatically")
            .on_hover_text(
                "Umbra briefs each system as soon as fresh intel is in, without being asked.",
            )
            .changed()
        {
            self.data.auto_intel.umbra_briefs_automatically = auto_brief;
            if !auto_brief {
                self.auto_briefings.clear();
                self.interrupt_auto_briefing();
            }
            self.save();
        }
    }

    /// Adds a briefing she just wrote to her feed, once per assessment.
    pub(super) fn record_umbra_briefing(&mut self, assessment: &SystemAssessment, auto: bool) {
        let entry = feed_entry(
            self.system_by_id(&assessment.system_id),
            assessment,
            now(),
            auto,
        );
        if feed_has(&self.umbra_feed, &entry) {
            return;
        }
        match self.store.record_umbra_briefing(&entry, now()) {
            Ok(Some(saved)) => {
                push_feed_entry(&mut self.umbra_feed, saved);
            }
            Ok(None) => {}
            Err(error) => {
                push_feed_entry(&mut self.umbra_feed, entry);
                self.note_umbra(
                    Some(assessment.system_id.clone()),
                    format!("Umbra's briefing could not be kept for her next session: {error}"),
                    true,
                );
            }
        }
    }

    pub(super) fn load_umbra_feed(&mut self) {
        match self.store.umbra_briefings(FEED_SHOWN) {
            Ok(feed) => self.umbra_feed = feed,
            Err(error) => {
                let message = format!("Umbra's earlier briefings could not be recalled: {error}");
                self.status = message.clone();
                self.note_umbra(None, message, true);
            }
        }
    }

    fn clear_umbra_feed(&mut self) {
        match self.store.clear_umbra_briefings() {
            Ok(()) => {
                self.umbra_feed.clear();
                self.status = "Umbra's briefings cleared".to_owned();
            }
            Err(error) => self.status = format!("Could not clear Umbra's briefings: {error}"),
        }
    }

    /// The install's progress bar, while her core is coming aboard.
    pub(super) fn umbra_progress_bar(&self, ui: &mut egui::Ui) {
        if let Some(progress) = &self.local_intel_progress {
            let (fraction, text) = progress_bar_text(progress);
            ui.add(egui::ProgressBar::new(fraction).text(text));
        }
    }

    /// Brings her aboard from her own panel: what she needs, the button that starts
    /// it, and how it is going, including why it failed.
    fn umbra_install_card(&mut self, ui: &mut egui::Ui, card: InstallCard) {
        let installing = matches!(
            self.umbra_task,
            Some(UmbraTask::Installing | UmbraTask::Importing)
        );
        egui::Frame::new()
            .stroke(Stroke::new(1.0_f32, theme::umbra()))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                umbra_quote(ui, card.line());
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(card.details(&local_intel::download_size_label()))
                        .color(theme::muted_text()),
                );
                ui.add_space(6.0);
                if installing {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(match card {
                                InstallCard::Install => "Bringing Umbra aboard...",
                                InstallCard::Repair => "Repairing Umbra...",
                            })
                            .color(theme::umbra()),
                        );
                    });
                    self.umbra_progress_bar(ui);
                } else {
                    let free = self.umbra_free_for_player();
                    let button = egui::Button::new(
                        egui::RichText::new(card.button())
                            .strong()
                            .color(theme::umbra_text()),
                    )
                    .fill(theme::umbra_background())
                    .stroke(Stroke::new(1.0_f32, theme::umbra()))
                    .min_size(Vec2::new(160.0, 32.0));
                    let response = ui.add_enabled(free, button);
                    if response.clicked() {
                        self.start_local_model_install();
                    }
                    if !free {
                        response.on_disabled_hover_text("Umbra is busy; try again in a moment.");
                    }
                }
                if let Some(notice) = &self.umbra_notice
                    && notice.system_id.is_none()
                {
                    let color = if notice.is_error {
                        WARNING_TEXT
                    } else {
                        theme::muted_text()
                    };
                    ui.label(egui::RichText::new(&notice.message).color(color));
                }
            });
    }

    /// Her own panel: who she is, what she is doing, and every briefing she has written.
    /// It fills `ui`; the feed takes whatever height the header leaves. When the header
    /// leaves less than the feed's minimum, the panel stays scrolled to the feed's bottom so
    /// her newest briefing and her writing line stay in view.
    pub(super) fn umbra_panel(&mut self, ui: &mut egui::Ui) {
        let avatar = self.umbra_avatar(ui.ctx());
        let viewport_height = ui.available_height();
        let avatar_size = header_avatar_size(ui.available_width(), viewport_height);
        egui::ScrollArea::vertical()
            .id_salt("umbra_panel")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let header_top = ui.cursor().top();
                let card =
                    InstallCard::for_core(local_intel::core_status(), self.umbra_needs_repair);
                self.umbra_header(ui, avatar.as_ref(), avatar_size, card.is_some());
                if let Some(card) = card {
                    ui.add_space(6.0);
                    self.umbra_install_card(ui, card);
                }
                ui.separator();
                let header_height = ui.cursor().top() - header_top;
                let feed_height = (viewport_height - header_height).max(MIN_FEED_HEIGHT);
                self.umbra_feed_view(ui, avatar.as_ref(), feed_height);
            });
    }

    /// Her briefings as a chat, newest at the bottom, in exactly `height` points.
    fn umbra_feed_view(
        &mut self,
        ui: &mut egui::Ui,
        avatar: Option<&egui::TextureHandle>,
        height: f32,
    ) {
        let chain = ChainLookup::new(&self.data.systems);
        let current_time = now();
        let mut clicked_system = None;
        let writing = match &self.umbra_task {
            Some(task @ UmbraTask::Briefing { system_id, .. }) => {
                Some((self.system_name(system_id), task.is_waking()))
            }
            _ => None,
        };

        egui::ScrollArea::vertical()
            .id_salt("umbra_feed")
            .auto_shrink([false, false])
            .max_height(height)
            .min_scrolled_height(height)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.umbra_feed.is_empty() && writing.is_none() {
                    ui.horizontal_top(|ui| {
                        paint_avatar(ui, avatar, MESSAGE_AVATAR_SIZE);
                        ui.add_space(4.0);
                        ui.vertical(|ui| umbra_quote(ui, EMPTY_FEED_LINE));
                    });
                }
                for entry in &self.umbra_feed {
                    ui.horizontal_top(|ui| {
                        paint_avatar(ui, avatar, MESSAGE_AVATAR_SIZE);
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                let name = egui::RichText::new(&entry.system_name)
                                    .strong()
                                    .color(theme::system_text());
                                if let Some(target) = chain.target(entry) {
                                    if ui
                                        .add(egui::Label::new(name).sense(Sense::click()))
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text("Select on the chain map")
                                        .clicked()
                                    {
                                        clicked_system = Some(target.to_owned());
                                    }
                                } else {
                                    ui.label(name).on_hover_text("No longer on the chain map");
                                }
                                pill(ui, &entry.system_class);
                                pill(ui, &score_pill("Threat", entry.threat_score));
                                pill(ui, &score_pill("Activity", entry.activity_score));
                                let mut time = message_time(entry.briefed_at, current_time);
                                if !entry.auto {
                                    time.push_str(" · you asked");
                                }
                                ui.label(egui::RichText::new(time).color(theme::muted_text()));
                            });
                            umbra_quote(ui, &entry.briefing);
                        });
                    });
                    ui.add_space(8.0);
                }
                if let Some((system_name, waking)) = &writing {
                    ui.horizontal_top(|ui| {
                        paint_avatar(ui, avatar, MESSAGE_AVATAR_SIZE);
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            if *waking {
                                ui.label(
                                    egui::RichText::new("Umbra is waking up...")
                                        .italics()
                                        .color(theme::umbra()),
                                );
                            } else {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.label(
                                    egui::RichText::new("Umbra is writing about ")
                                        .italics()
                                        .color(theme::umbra()),
                                );
                                ui.label(
                                    egui::RichText::new(system_name).color(theme::system_text()),
                                );
                                ui.label(
                                    egui::RichText::new("...").italics().color(theme::umbra()),
                                );
                            }
                        });
                    });
                }
            });

        if let Some(system_id) = clicked_system {
            self.selected_system_id = system_id;
        }
    }

    fn umbra_header(
        &mut self,
        ui: &mut egui::Ui,
        avatar: Option<&egui::TextureHandle>,
        avatar_size: f32,
        install_card_shown: bool,
    ) {
        let core = local_intel::core_status();
        ui.horizontal_top(|ui| {
            paint_avatar(ui, avatar, avatar_size);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.heading(egui::RichText::new("Umbra").strong().color(theme::umbra()));
                    ui.add_space(4.0);
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let state = self.umbra_state_label(core);
                    let state_color = match state {
                        "Online" => theme::online(),
                        "Needs repair" => WARNING_TEXT,
                        _ => theme::muted_text(),
                    };
                    ui.label(egui::RichText::new(format!("● {state}")).color(state_color));
                    match &self.umbra_task {
                        Some(UmbraTask::Briefing { system_id, .. }) => {
                            ui.label(" · writing a briefing for ");
                            ui.label(
                                egui::RichText::new(self.system_name(system_id))
                                    .color(theme::system_text()),
                            );
                        }
                        Some(task) => {
                            ui.label(
                                egui::RichText::new(format!(" · {}", task.label()))
                                    .color(theme::muted_text()),
                            );
                        }
                        None => {}
                    }
                    let queued = self.auto_briefings.len();
                    if queued > 0 {
                        ui.label(
                            egui::RichText::new(format!(" · {queued} queued"))
                                .color(theme::muted_text()),
                        );
                    }
                });
                ui.label(
                    egui::RichText::new(local_intel::UMBRA_TAGLINE).color(theme::muted_text()),
                );
                ui.horizontal_wrapped(|ui| {
                    self.umbra_auto_brief_toggle(ui);
                    if ui.button("Manage Umbra").clicked() {
                        self.show_local_intel_settings = true;
                    }
                    if !self.umbra_feed.is_empty()
                        && !self.umbra_confirm_clear
                        && ui.button("Clear history").clicked()
                    {
                        self.umbra_confirm_clear = true;
                    }
                });
                if self.umbra_confirm_clear {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new("Forget every briefing Umbra has written?")
                                .color(WARNING_TEXT),
                        );
                        if ui.button("Forget them").clicked() {
                            self.clear_umbra_feed();
                            self.umbra_confirm_clear = false;
                        }
                        if ui.button("Keep").clicked() {
                            self.umbra_confirm_clear = false;
                        }
                    });
                }
                // While her install card is up, her own news is shown on it instead.
                if let Some(notice) = self
                    .umbra_notice
                    .as_ref()
                    .filter(|notice| !install_card_shown || notice.system_id.is_some())
                {
                    let color = if notice.is_error {
                        WARNING_TEXT
                    } else {
                        theme::muted_text()
                    };
                    ui.label(egui::RichText::new(&notice.message).color(color));
                }
            });
        });
    }
}

/// Her portrait's size in a panel of the given size: full when there is room.
pub(super) fn header_avatar_size(width: f32, height: f32) -> f32 {
    if width < COMPACT_PANEL_WIDTH || height < COMPACT_PANEL_HEIGHT {
        COMPACT_HEADER_AVATAR_SIZE
    } else {
        HEADER_AVATAR_SIZE
    }
}

/// Her words for the sidebar, when she wrote them.
pub(super) fn sidebar_words(assessment: &SystemAssessment) -> Option<&str> {
    (BriefingVoice::of(assessment) == BriefingVoice::Umbra).then_some(assessment.briefing.as_str())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{
        AVATAR_PNG, AVATAR_TEXTURE_SIZE, BriefingVoice, COMPACT_HEADER_AVATAR_SIZE, ChainLookup,
        FEED_SHOWN, HEADER_AVATAR_SIZE, InstallCard, avatar_image, evidence_heading,
        feed_class_label, feed_entry, header_avatar_size, message_time, progress_bar_text,
        push_feed_entry, sidebar_words,
    };
    use crate::local_intel::{CoreStatus, DownloadProgress};
    use crate::model::{SystemAssessment, SystemNode};

    const NOW: u64 = 1_760_000_000;

    fn assessment(model_generated: bool, briefing: &str) -> SystemAssessment {
        SystemAssessment {
            system_id: "sys_a".to_owned(),
            threat_score: 64,
            activity_score: 22,
            confidence: "Medium".to_owned(),
            evidence: vec![
                "3 kills in the last hour".to_owned(),
                "2 NPC kills".to_owned(),
            ],
            briefing: briefing.to_owned(),
            model_generated,
            generated_at: NOW - 120,
        }
    }

    fn node(solar_system_id: u64, name: &str, security_status: f32) -> SystemNode {
        SystemNode {
            id: "sys_a".to_owned(),
            solar_system_id,
            name: name.to_owned(),
            class: String::new(),
            security_status,
            constellation_id: 0,
            constellation_name: String::new(),
            region_id: 10_000_002,
            region_name: "The Forge".to_owned(),
            planet_count: 0,
            stargate_count: 0,
            station_count: 0,
            jumps_1h: 0,
            ship_kills_1h: 0,
            pod_kills_1h: 0,
            npc_kills_1h: 0,
            intel_updated_at: 0,
            effect: String::new(),
            status: String::new(),
            owner: String::new(),
            notes: String::new(),
            created_at: 0,
            map_position: None,
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height))
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn only_an_accepted_briefing_speaks_as_umbra() {
        let briefed = assessment(true, "Threat is Elevated after 3 kills.");
        assert_eq!(BriefingVoice::of(&briefed), BriefingVoice::Umbra);
        assert_eq!(BriefingVoice::Umbra.ask_label(), "Ask Umbra again");
        assert_eq!(
            sidebar_words(&briefed),
            Some("Threat is Elevated after 3 kills.")
        );

        // Her briefing was set aside or never asked for: the evidence summary shows.
        let summary = assessment(false, "Threat is Elevated while activity is Low.");
        assert_eq!(BriefingVoice::of(&summary), BriefingVoice::Summary);
        assert_eq!(BriefingVoice::Summary.ask_label(), "Ask Umbra");
        assert_eq!(sidebar_words(&summary), None);
        assert_eq!(
            BriefingVoice::of(&assessment(true, "  ")),
            BriefingVoice::Summary
        );
    }

    #[test]
    fn evidence_heading_counts_the_lines() {
        assert_eq!(
            evidence_heading(&assessment(false, "").evidence),
            "Evidence (2)"
        );
        assert_eq!(evidence_heading(&[]), "Evidence (0)");
    }

    #[test]
    fn the_shipped_avatar_decodes_as_a_square() {
        let avatar = avatar_image(AVATAR_PNG, AVATAR_TEXTURE_SIZE).expect("avatar should decode");
        assert_eq!(avatar.size[0], avatar.size[1]);
        assert!(avatar.size[0] >= 128);
        assert!(avatar.size[0] <= AVATAR_TEXTURE_SIZE as usize);
    }

    #[test]
    fn the_shipped_avatar_is_her_own_portrait() {
        let art = image::load_from_memory(AVATAR_PNG).expect("avatar should decode");
        assert_eq!(art.width(), art.height());
        assert!(art.width() >= 512);
        // Not the app icon standing in for her.
        assert_ne!(
            AVATAR_PNG,
            include_bytes!("../../assets/icons/nomadeer-256.png").as_slice()
        );
    }

    #[test]
    fn avatars_are_cropped_square_and_scaled_down() {
        assert_eq!(avatar_image(&png(300, 200), 192).unwrap().size, [192, 192]);
        assert_eq!(
            avatar_image(&png(1024, 1024), 192).unwrap().size,
            [192, 192]
        );
        assert_eq!(avatar_image(&png(128, 128), 192).unwrap().size, [128, 128]);
        assert!(avatar_image(b"not an image", 192).is_none());
    }

    #[test]
    fn feed_entries_keep_the_system_as_it_was() {
        let briefed = assessment(true, "Threat is Elevated after 3 kills.");
        let wormhole = node(31_002_238, "J123456", -1.0);
        let entry = feed_entry(Some(&wormhole), &briefed, NOW, true);
        assert_eq!(entry.system_name, "J123456");
        assert_eq!(entry.solar_system_id, 31_002_238);
        assert_eq!(entry.assessed_at, NOW - 120);
        assert_eq!(entry.briefed_at, NOW);
        assert_eq!(entry.briefing, briefed.briefing);
        assert!(entry.auto);

        assert_eq!(feed_class_label(&node(30_002_813, "Tama", 0.3)), "LS 0.3");
        assert_eq!(feed_class_label(&node(30_000_142, "Jita", 0.9)), "HS 0.9");
        let missing = feed_entry(None, &briefed, NOW, false);
        assert_eq!(missing.system_name, "Unknown");
        assert!(missing.system_class.is_empty());
    }

    #[test]
    fn feed_links_follow_the_solar_system_to_its_current_node() {
        let briefed = assessment(true, "Threat is Elevated after 3 kills.");
        let mut legacy = node(31_002_238, "J123456", -1.0);
        legacy.id = "sys_1700000000_3".to_owned();
        let entry = feed_entry(Some(&legacy), &briefed, NOW, true);

        // Still on the map under the id she briefed.
        let systems = vec![legacy.clone()];
        assert_eq!(
            ChainLookup::new(&systems).target(&entry),
            Some("sys_1700000000_3")
        );

        // Cleared from the chain and flown back into: the same system, a new node.
        let mut readded = node(31_002_238, "J123456", -1.0);
        readded.id = "esi_sys_31002238".to_owned();
        let systems = vec![readded];
        assert_eq!(
            ChainLookup::new(&systems).target(&entry),
            Some("esi_sys_31002238")
        );

        // Gone from the map altogether.
        assert_eq!(ChainLookup::new(&[]).target(&entry), None);

        // A system whose solar system she never knew is found by the node she briefed.
        let unknown = feed_entry(None, &briefed, NOW, false);
        let mut manual = node(0, "Home", -1.0);
        manual.id = "sys_a".to_owned();
        let mut other = node(0, "Static", -1.0);
        other.id = "sys_b".to_owned();
        let systems = vec![other, manual];
        assert_eq!(ChainLookup::new(&systems).target(&unknown), Some("sys_a"));
    }

    #[test]
    fn the_feed_skips_repeats_and_keeps_the_newest() {
        let briefed = assessment(true, "Threat is Elevated after 3 kills.");
        let mut feed = Vec::new();
        let first = feed_entry(None, &briefed, NOW, true);
        assert!(push_feed_entry(&mut feed, first.clone()));
        // Asked again about the same assessment, she adds nothing new.
        assert!(!push_feed_entry(
            &mut feed,
            feed_entry(None, &briefed, NOW + 60, false)
        ));
        assert_eq!(feed, vec![first]);

        for index in 0..FEED_SHOWN as u64 + 3 {
            let mut newer = briefed.clone();
            newer.generated_at = NOW + index;
            push_feed_entry(&mut feed, feed_entry(None, &newer, NOW + index, true));
        }
        assert_eq!(feed.len(), FEED_SHOWN);
        assert_eq!(
            feed.last().unwrap().assessed_at,
            NOW + FEED_SHOWN as u64 + 2
        );
        assert!(
            feed.windows(2)
                .all(|pair| pair[0].briefed_at <= pair[1].briefed_at)
        );
    }

    #[test]
    fn messages_show_eve_time_and_age() {
        let midnight = NOW / 86_400 * 86_400;
        assert_eq!(
            message_time(
                midnight + 14 * 3_600 + 32 * 60,
                midnight + 14 * 3_600 + 37 * 60
            ),
            "14:32 EVE · 5m ago"
        );
        assert_eq!(
            message_time(NOW, NOW),
            format!("{} EVE · just now", crate::local_intel::eve_clock(NOW))
        );
    }

    #[test]
    fn her_portrait_shrinks_only_when_her_panel_is_cramped() {
        assert_eq!(header_avatar_size(640.0, 480.0), HEADER_AVATAR_SIZE);
        assert_eq!(header_avatar_size(300.0, 480.0), COMPACT_HEADER_AVATAR_SIZE);
        assert_eq!(header_avatar_size(640.0, 200.0), COMPACT_HEADER_AVATAR_SIZE);
    }

    #[test]
    fn her_install_card_shows_until_her_core_is_in_place() {
        assert_eq!(
            InstallCard::for_core(CoreStatus::Missing, false),
            Some(InstallCard::Install)
        );
        assert_eq!(InstallCard::for_core(CoreStatus::Ready, false), None);
        // A core that is only waiting for its check is in place.
        assert_eq!(InstallCard::for_core(CoreStatus::Unchecked, false), None);
        for core in [
            CoreStatus::Missing,
            CoreStatus::Unchecked,
            CoreStatus::Ready,
        ] {
            assert_eq!(InstallCard::for_core(core, true), Some(InstallCard::Repair));
        }
        assert_eq!(InstallCard::Install.button(), "Install Umbra");
        assert_eq!(InstallCard::Repair.button(), "Repair Umbra");
        assert!(
            InstallCard::Install
                .details("1.1 GB")
                .contains("about 1.1 GB")
        );
        for card in [InstallCard::Install, InstallCard::Repair] {
            assert!(card.details("1.1 GB").contains("entirely on this computer"));
        }
    }

    #[test]
    fn install_progress_reads_in_megabytes() {
        let progress = |done, total| DownloadProgress {
            label: "Downloading Umbra's core".to_owned(),
            done,
            total,
        };
        let (fraction, text) = progress_bar_text(&progress(558_660_368, 1_117_320_736));
        assert!((fraction - 0.5).abs() < 1e-6);
        assert_eq!(text, "Downloading Umbra's core · 559 of 1117 MB · 50%");
        assert_eq!(progress_bar_text(&progress(10, 0)).0, 0.0);
        assert_eq!(progress_bar_text(&progress(20, 10)).0, 1.0);
    }
}
