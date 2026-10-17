//! Pasted local and d-scans: read on worker threads, kept per system, shown beside
//! its signatures and, while fresh, folded into its assessment. A new paste
//! re-assesses the system, and Umbra briefs it if she briefs on her own.

use super::{NomadeerApp, WARNING_TEXT};
use crate::auto_intel::{self, IntelRequest};
use crate::config::theme;
use crate::eve_lookup;
use crate::live_intel::{
    self, DscanReport, DscanRow, KillerOverlap, LIVE_INTEL_FRESH_SECS, LiveContribution,
    LocalGroup, LocalReport, SystemKills,
};
use crate::local_intel;
use crate::model::{SystemAssessment, SystemNode, SystemTrafficHistory};
use crate::paste;
use crate::store::KILLMAIL_RETENTION_SECS;
use crate::util::now;
use crate::wormhole;
use crate::zkill::ZkillReport;
use eframe::egui;
use std::collections::HashSet;
use std::thread;
use std::time::Instant;

/// How each kind of paste is kept in the database.
const LOCAL: &str = "local";
const DSCAN: &str = "dscan";
/// When live intel has run out and the system has no killboard read to re-assess
/// it from, the read is tried again after this long if it fails.
const EXPIRY_RETRY_SECS: u64 = 5 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LiveKind {
    Local,
    Dscan,
}

/// A paste read on a worker thread.
pub(super) enum LiveTaskResult {
    Local {
        system_id: String,
        report: LocalReport,
        /// The system's solar system ID and cached week of kills, when its killboard
        /// has been read.
        killmails: Option<(u64, SystemKills)>,
    },
    Dscan {
        system_id: String,
        report: DscanReport,
    },
}

/// What the sidebar shows of a local paste that takes a pass over the system's kills,
/// worked out again only when the paste, the killboard or the minute changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct LocalCheck {
    /// The system, paste time, killboard read, kills held and minute it is for.
    key: (String, u64, u64, usize, u64),
    groups: Vec<LocalGroup>,
    /// None until the system's killboard has been read.
    overlap: Option<KillerOverlap>,
    /// The whole week has been read recently, so no kills here means none.
    week_read: bool,
    /// Pilots in local who got kills here this week.
    killers: HashSet<u64>,
}

impl LocalCheck {
    fn key(
        system_id: &str,
        report: &LocalReport,
        kills: Option<&SystemKills>,
        current_time: u64,
    ) -> (String, u64, u64, usize, u64) {
        (
            system_id.to_owned(),
            report.captured_at,
            kills.map_or(0, SystemKills::read_at),
            kills.map_or(0, |kills| kills.killmails.len()),
            current_time / 60,
        )
    }

    fn new(
        system_id: &str,
        report: &LocalReport,
        kills: Option<&SystemKills>,
        current_time: u64,
    ) -> Self {
        let pilots = report
            .pilots
            .iter()
            .map(|pilot| pilot.character_id)
            .collect::<HashSet<_>>();
        let killers = kills
            .map(|kills| {
                kills
                    .killmails
                    .iter()
                    .filter(|kill| {
                        !kill.npc
                            && current_time.saturating_sub(kill.killmail_time)
                                <= 7 * crate::util::DAY_SECS
                    })
                    .flat_map(|kill| kill.attacker_ids.iter().copied())
                    .filter(|id| pilots.contains(id))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            key: Self::key(system_id, report, kills, current_time),
            groups: live_intel::local_groups(report),
            overlap: kills
                .map(|kills| live_intel::killer_overlap(report, &kills.killmails, current_time)),
            week_read: kills.is_some_and(|kills| kills.covers_week(current_time)),
            killers,
        }
    }
}

impl NomadeerApp {
    /// Picks up pastes kept from earlier sessions.
    pub(super) fn load_live_intel(&mut self) {
        let rows = match self.store.live_intel() {
            Ok(rows) => rows,
            Err(error) => {
                self.status = format!("Could not load pasted intel: {error}");
                return;
            }
        };
        for (system_id, kind, report) in rows {
            let intel = self.live_intel.entry(system_id).or_default();
            // A paste that no longer reads is simply dropped.
            match kind.as_str() {
                LOCAL => intel.local = serde_json::from_str(&report).ok(),
                DSCAN => intel.dscan = serde_json::from_str(&report).ok(),
                _ => {}
            }
        }
        // Assessments saved with live intel are redone once it has run out.
        let current_time = now();
        for assessment in self.system_assessments.values() {
            if assessment
                .confidence
                .ends_with(local_intel::LIVE_CONFIDENCE)
            {
                let expires_at = self
                    .live_intel
                    .get(&assessment.system_id)
                    .and_then(|intel| intel.expires_at(current_time))
                    .unwrap_or(current_time);
                self.live_expiry
                    .insert(assessment.system_id.clone(), expires_at);
            }
        }
    }

    pub(super) fn apply_local_paste(&mut self, lines: &[String]) {
        let own_names = self
            .data
            .accounts
            .iter()
            .map(|account| account.character_name.as_str())
            .collect::<Vec<_>>();
        if !paste::could_be_local(&live_intel::local_names(lines), &own_names) {
            self.status = "That paste is a single name, not the local member list. Copy all \
                           of local in game (Ctrl+A, Ctrl+C)"
                .to_owned();
            return;
        }
        let Some(system) = self.selected_system() else {
            self.status = "Select a system on the chain map before pasting local".to_owned();
            return;
        };
        let system_id = system.id.clone();
        let solar_system_id = system.solar_system_id;
        let pilots = live_intel::local_names(lines).len();
        self.status = format!("Reading local in {}: {} names...", system.name, pilots);
        let own = self
            .data
            .accounts
            .iter()
            .map(|account| {
                (
                    account.character_name.clone(),
                    account.character_id.parse::<u64>().unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        let lines = lines.to_vec();
        let store = self.store.clone();
        let tx = self.live_tx.clone();
        self.live_reading.push((system_id.clone(), LiveKind::Local));
        // Taken now, so pastes that finish out of order still sort by when they
        // were made.
        let captured_at = now();
        thread::spawn(move || {
            let report = eve_lookup::read_local(&store, &lines, &own, captured_at);
            let current_time = now();
            let killmails = (solar_system_id != 0)
                .then(|| {
                    let killmails = store
                        .cached_killmails(
                            solar_system_id,
                            current_time.saturating_sub(KILLMAIL_RETENTION_SECS),
                        )
                        .ok()?;
                    let coverage = store.killboard_coverage(solar_system_id).ok().flatten();
                    let read = !killmails.is_empty() || coverage.is_some();
                    read.then_some((
                        solar_system_id,
                        SystemKills {
                            killmails,
                            coverage,
                        },
                    ))
                })
                .flatten();
            let _ = tx.send(LiveTaskResult::Local {
                system_id,
                report,
                killmails,
            });
        });
    }

    pub(super) fn apply_directional_scan(&mut self, rows: Vec<DscanRow>) {
        let Some(system) = self.selected_system() else {
            self.status = "Select a system on the chain map before pasting its d-scan".to_owned();
            return;
        };
        let system_id = system.id.clone();
        self.status = format!(
            "Reading d-scan in {}: {} results...",
            system.name,
            rows.len()
        );
        let store = self.store.clone();
        let tx = self.live_tx.clone();
        self.live_reading.push((system_id.clone(), LiveKind::Dscan));
        let captured_at = now();
        thread::spawn(move || {
            let report = eve_lookup::read_dscan(&store, &rows, captured_at);
            let _ = tx.send(LiveTaskResult::Dscan { system_id, report });
        });
    }

    pub(super) fn poll_live_tasks(&mut self) {
        while let Ok(task) = self.live_rx.try_recv() {
            let (system_id, kind, captured_at) = match &task {
                LiveTaskResult::Local {
                    system_id, report, ..
                } => (system_id.clone(), LiveKind::Local, report.captured_at),
                LiveTaskResult::Dscan { system_id, report } => {
                    (system_id.clone(), LiveKind::Dscan, report.captured_at)
                }
            };
            // This paste is read; another of the same kind may still be reading.
            if let Some(index) = self
                .live_reading
                .iter()
                .position(|(reading, reading_kind)| *reading == system_id && *reading_kind == kind)
            {
                self.live_reading.remove(index);
            }
            if let LiveTaskResult::Local {
                killmails: Some((solar_system_id, kills)),
                ..
            } = &task
            {
                self.keep_latest_kills(*solar_system_id, kills.clone());
            }
            // The system left the chain while its paste was being read.
            if self.system_by_id(&system_id).is_none() {
                continue;
            }
            // Pastes are read side by side, so a newer one can finish first; it stays.
            let held = self
                .live_intel
                .get(&system_id)
                .and_then(|intel| match kind {
                    LiveKind::Local => intel.local.as_ref().map(|report| report.captured_at),
                    LiveKind::Dscan => intel.dscan.as_ref().map(|report| report.captured_at),
                });
            if held.is_some_and(|held| held > captured_at) {
                continue;
            }
            let (saved, summary, error) = match task {
                LiveTaskResult::Local { report, .. } => {
                    let summary = self.local_status(&system_id, &report);
                    let saved =
                        serde_json::to_string(&report).map(|json| (report.captured_at, json));
                    let error = report.lookup_error.clone();
                    self.live_intel.entry(system_id.clone()).or_default().local = Some(report);
                    (saved, summary, error)
                }
                LiveTaskResult::Dscan { report, .. } => {
                    let summary = dscan_status(&report);
                    let saved =
                        serde_json::to_string(&report).map(|json| (report.captured_at, json));
                    let error = report.lookup_error.clone();
                    self.live_intel.entry(system_id.clone()).or_default().dscan = Some(report);
                    (saved, summary, error)
                }
            };
            let kind_key = match kind {
                LiveKind::Local => LOCAL,
                LiveKind::Dscan => DSCAN,
            };
            let save_error =
                saved
                    .map_err(|error| error.to_string())
                    .and_then(|(captured_at, json)| {
                        self.store
                            .record_live_intel(&system_id, kind_key, captured_at, &json)
                    });
            self.reassess_with_live_intel(&system_id, true);
            let mut status = format!("{summary} in {}", self.system_name(&system_id));
            if !error.is_empty() {
                status.push_str(&format!(" · some lookups failed: {error}"));
            }
            if let Err(error) = save_error {
                status.push_str(&format!(" · could not be saved: {error}"));
            }
            self.status = status;
        }
    }

    /// Assesses a system from its killboard and traffic, with its fresh live intel
    /// and what the player marked on the chain.
    pub(super) fn assess(
        &mut self,
        system: &SystemNode,
        report: &ZkillReport,
        traffic: &SystemTrafficHistory,
        current_time: u64,
    ) -> SystemAssessment {
        self.assess_as_of(system, report, traffic, current_time, current_time)
    }

    /// As [`Self::assess`], dated `assessed_at`: an assessment from an older
    /// killboard read keeps that read's time, so it still shows as due a refresh.
    fn assess_as_of(
        &mut self,
        system: &SystemNode,
        report: &ZkillReport,
        traffic: &SystemTrafficHistory,
        assessed_at: u64,
        current_time: u64,
    ) -> SystemAssessment {
        let live = self.live_contribution(system, current_time);
        let expires_at = self
            .live_intel
            .get(&system.id)
            .and_then(|intel| intel.expires_at(current_time));
        match expires_at {
            Some(expires_at) if live.is_live() => {
                self.live_expiry.insert(system.id.clone(), expires_at);
            }
            _ => {
                self.live_expiry.remove(&system.id);
            }
        }
        let mut assessment =
            local_intel::assess_with_live_intel(system, report, traffic, &live, assessed_at);
        // What the player marked on the chain is evidence too.
        assessment.evidence.extend(self.chain_facts_for(&system.id));
        assessment
    }

    fn live_contribution(&self, system: &SystemNode, current_time: u64) -> LiveContribution {
        let Some(intel) = self.live_intel.get(&system.id) else {
            return LiveContribution::default();
        };
        let kills = self.system_killmails.get(&system.solar_system_id);
        live_intel::contribution(intel, kills, current_time)
    }

    /// Keeps a system's kills unless those already held were read more recently.
    pub(super) fn keep_latest_kills(&mut self, solar_system_id: u64, kills: SystemKills) {
        let held = self
            .system_killmails
            .get(&solar_system_id)
            .map(SystemKills::read_at);
        if held.is_none_or(|held| held <= kills.read_at()) {
            self.system_killmails.insert(solar_system_id, kills);
        }
    }

    /// Re-assesses a system for its live intel. With `brief`, for a new paste, that is
    /// straight away from the killboard already read when that is recent, otherwise
    /// by reading it again, and Umbra briefs the new assessment if she briefs on her
    /// own. Without, for intel that ran out, the last killboard read is reused however
    /// old, so taking the paste out never starts a read or a briefing of its own.
    fn reassess_with_live_intel(&mut self, system_id: &str, brief: bool) {
        let Some(system) = self.system_by_id(system_id).cloned() else {
            return;
        };
        let current_time = now();
        let report = self
            .zkill_reports
            .get(&system.solar_system_id)
            .filter(|report| {
                !brief || !auto_intel::assessment_is_stale(report.coverage.until, current_time)
            })
            .cloned();
        match report {
            Some(report) => {
                let traffic = self
                    .refreshed_traffic
                    .get(&system.solar_system_id)
                    .cloned()
                    .unwrap_or_default();
                let assessed_at =
                    if auto_intel::assessment_is_stale(report.coverage.until, current_time) {
                        report.coverage.until
                    } else {
                        current_time
                    };
                let mut assessment =
                    self.assess_as_of(&system, &report, &traffic, assessed_at, current_time);
                if let Some(previous) = self.system_assessments.get(system_id) {
                    local_intel::keep_briefing(&mut assessment, previous);
                }
                self.system_assessments
                    .insert(assessment.system_id.clone(), assessment.clone());
                if brief {
                    self.queue_auto_briefing(&assessment);
                }
                self.upsert_system_assessment(assessment);
                self.save_quietly();
            }
            // The refresh assesses the system with its live intel when it is done.
            None if system.solar_system_id != 0 => {
                let request = if brief {
                    IntelRequest::Player
                } else {
                    // Only an assessment from an earlier session has no killboard read
                    // to fall back on. Should the read fail, it is tried again later,
                    // so intel that ran out never stays in the assessment.
                    self.live_expiry
                        .insert(system.id.clone(), current_time + EXPIRY_RETRY_SECS);
                    IntelRequest::Arrival
                };
                self.intel_queue
                    .push(system.solar_system_id, request, Instant::now());
                self.maybe_start_intel();
            }
            None => {}
        }
    }

    /// Takes live intel that has run out back out of the assessments it was in.
    pub(super) fn maybe_expire_live_intel(&mut self) {
        let current_time = now();
        let expired = self
            .live_expiry
            .iter()
            .filter(|(_, expires_at)| **expires_at <= current_time)
            .map(|(system_id, _)| system_id.clone())
            .collect::<Vec<_>>();
        for system_id in expired {
            self.live_expiry.remove(&system_id);
            self.reassess_with_live_intel(&system_id, false);
        }
    }

    /// Forgets a system's pastes when it leaves the chain.
    pub(super) fn forget_live_intel(&mut self, system_id: &str) {
        self.live_expiry.remove(system_id);
        if self.live_intel.remove(system_id).is_some()
            && let Err(error) = self.store.forget_live_intel(system_id)
        {
            self.status = format!("Could not forget pasted intel: {error}");
        }
    }

    /// "Local · 23 pilots · 5 from Goonswarm Federation · 2 got kills here today".
    fn local_status(&self, system_id: &str, report: &LocalReport) -> String {
        let mut parts = vec!["Local".to_owned(), pilot_count_text(report.pilot_count)];
        if let Some(group) = live_intel::local_groups(report).first() {
            parts.push(format!("{} from {}", group.count, group.name));
        }
        if let Some(overlap) = self.killer_overlap(system_id, report, now())
            && overlap.pilots_24h > 0
        {
            parts.push(format!("{} got kills here today", overlap.pilots_24h));
        }
        parts.join(" · ")
    }

    fn killer_overlap(
        &self,
        system_id: &str,
        report: &LocalReport,
        current_time: u64,
    ) -> Option<KillerOverlap> {
        let kills = self.system_kills(system_id)?;
        Some(live_intel::killer_overlap(
            report,
            &kills.killmails,
            current_time,
        ))
    }

    fn system_kills(&self, system_id: &str) -> Option<&SystemKills> {
        let system = self.system_by_id(system_id)?;
        self.system_killmails.get(&system.solar_system_id)
    }

    /// The selected system's latest local and d-scan, with the Paste button.
    pub(super) fn live_intel_section(&mut self, ui: &mut egui::Ui, system_id: &str) {
        ui.horizontal(|ui| {
            ui.strong("Local and d-scan");
            if ui
                .button("Paste")
                .on_hover_text(
                    "Paste the local member list or the d-scan results (Ctrl+A, Ctrl+C in game)",
                )
                .clicked()
            {
                self.request_paste(ui.ctx());
            }
        });
        for (_, kind) in self
            .live_reading
            .iter()
            .filter(|(reading, _)| reading == system_id)
        {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(match kind {
                    LiveKind::Local => "Looking up the pilots in local...",
                    LiveKind::Dscan => "Reading the d-scan...",
                });
            });
        }
        let current_time = now();
        // Worked out once, not on every frame the sidebar is drawn.
        let check = self
            .live_intel
            .get(system_id)
            .and_then(|intel| intel.local.as_ref())
            .and_then(|report| {
                let kills = self.system_kills(system_id);
                let key = LocalCheck::key(system_id, report, kills, current_time);
                (self.local_check.as_ref().map(|check| &check.key) != Some(&key))
                    .then(|| LocalCheck::new(system_id, report, kills, current_time))
            });
        if let Some(check) = check {
            self.local_check = Some(check);
        }
        let intel = self
            .live_intel
            .get(system_id)
            .filter(|intel| intel.local.is_some() || intel.dscan.is_some());
        let Some(intel) = intel else {
            if !self
                .live_reading
                .iter()
                .any(|(reading, _)| reading == system_id)
            {
                ui.label(
                    egui::RichText::new(format!(
                        "Copy local or the d-scan in game, then press Paste or Ctrl+V here. \
                         Umbra weighs a paste for {} minutes.",
                        LIVE_INTEL_FRESH_SECS / 60
                    ))
                    .color(theme::muted_text()),
                );
            }
            return;
        };
        if let (Some(report), Some(check)) = (&intel.local, &self.local_check) {
            self.local_panel(ui, system_id, report, check, current_time);
        }
        if let Some(report) = &intel.dscan {
            dscan_panel(ui, system_id, report, current_time);
        }
    }

    fn local_panel(
        &self,
        ui: &mut egui::Ui,
        system_id: &str,
        report: &LocalReport,
        check: &LocalCheck,
        current_time: u64,
    ) {
        let fresh = paste_heading(ui, "Local", report.captured_at, current_time);
        let text_color = if fresh {
            theme::accent()
        } else {
            theme::muted_text()
        };
        let warning_color = if fresh {
            WARNING_TEXT
        } else {
            theme::muted_text()
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(pilot_count_text(report.pilot_count)).color(text_color));
            if !report.own.is_empty() {
                ui.label(
                    egui::RichText::new(format!("· {} yours", report.own.len()))
                        .color(theme::character_text()),
                )
                .on_hover_text(format!("Not counted: {}", report.own.join(", ")));
            }
        });
        if !check.groups.is_empty() {
            let shown = check
                .groups
                .iter()
                .take(5)
                .map(|group| format!("{} {}", group.count, group.name))
                .collect::<Vec<_>>();
            ui.label(egui::RichText::new(shown.join(" · ")).color(text_color));
        }
        match check.overlap {
            Some(overlap) if overlap.pilots_7d > 0 => {
                let text = if overlap.pilots_24h > 0 {
                    format!(
                        "{} got kills here in the last 24h, {} this week",
                        overlap.pilots_24h, overlap.pilots_7d
                    )
                } else {
                    format!("{} got kills here this week", overlap.pilots_7d)
                };
                ui.label(egui::RichText::new(text).color(warning_color));
            }
            Some(overlap) if overlap.corporations_7d > 0 => {
                ui.label(
                    egui::RichText::new(format!(
                        "{} of their corporations got kills here this week",
                        overlap.corporations_7d
                    ))
                    .color(warning_color),
                );
            }
            Some(_) if check.week_read => {
                ui.label(
                    egui::RichText::new("No one here got kills in this system this week")
                        .color(theme::muted_text()),
                );
            }
            Some(_) => {
                ui.label(
                    egui::RichText::new("None of them in the kills read so far")
                        .color(theme::muted_text()),
                )
                .on_hover_text(
                    "The killboard has not been read through the whole week lately, \
                     so this is not an all-clear. Refresh it to check again.",
                );
            }
            None => {
                ui.label(
                    egui::RichText::new("Refresh the killboard to check local against it")
                        .color(theme::muted_text()),
                );
            }
        }
        if !report.unidentified.is_empty() {
            ui.label(
                egui::RichText::new(format!(
                    "{} not found",
                    if report.unidentified.len() == 1 {
                        "1 name".to_owned()
                    } else {
                        format!("{} names", report.unidentified.len())
                    }
                ))
                .color(theme::muted_text()),
            )
            .on_hover_text(report.unidentified.join(", "));
        }
        if !report.lookup_error.is_empty() {
            ui.label(egui::RichText::new("Some pilots could not be looked up").color(WARNING_TEXT))
                .on_hover_text(&report.lookup_error);
        }
        if !report.pilots.is_empty() {
            egui::CollapsingHeader::new(format!("Pilots ({})", report.pilots.len()))
                .id_salt(("local_pilots", system_id))
                .show(ui, |ui| {
                    for pilot in &report.pilots {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(
                                egui::RichText::new(&pilot.name).color(theme::character_text()),
                            );
                            let affiliation = [&pilot.corporation, &pilot.alliance]
                                .into_iter()
                                .filter(|name| !name.is_empty())
                                .cloned()
                                .collect::<Vec<_>>();
                            if !affiliation.is_empty() {
                                ui.label(
                                    egui::RichText::new(affiliation.join(" · "))
                                        .color(theme::muted_text()),
                                );
                            }
                            if check.killers.contains(&pilot.character_id) {
                                ui.label(
                                    egui::RichText::new("got kills here").color(warning_color),
                                );
                            }
                        });
                    }
                });
        }
    }
}

/// A paste's heading, "Local · 3m ago" or "Earlier d-scan · 40m ago" once it no
/// longer counts. Returns whether it is fresh.
fn paste_heading(ui: &mut egui::Ui, what: &str, captured_at: u64, current_time: u64) -> bool {
    let age = current_time.saturating_sub(captured_at);
    let fresh = live_intel::is_fresh(captured_at, current_time);
    let ago = format!("{} ago", wormhole::short_duration(age));
    if fresh {
        let left = LIVE_INTEL_FRESH_SECS.saturating_sub(age);
        ui.label(egui::RichText::new(format!("{what} · {ago}")).strong())
            .on_hover_text(format!(
                "Part of the assessment for another {}",
                wormhole::short_duration(left)
            ));
    } else {
        ui.label(
            egui::RichText::new(format!("Earlier {} · {ago}", what.to_lowercase()))
                .strong()
                .color(theme::muted_text()),
        )
        .on_hover_text(format!(
            "Older than {} minutes, so no longer part of the assessment",
            LIVE_INTEL_FRESH_SECS / 60
        ));
    }
    fresh
}

fn dscan_panel(ui: &mut egui::Ui, system_id: &str, report: &DscanReport, current_time: u64) {
    let fresh = paste_heading(ui, "D-scan", report.captured_at, current_time);
    let text_color = if fresh {
        theme::accent()
    } else {
        theme::muted_text()
    };
    let ships = report.ships();
    let on_grid = report.ships_on_grid();
    ui.label(
        egui::RichText::new(match (ships, on_grid) {
            (0, _) => "No ships".to_owned(),
            (ships, 0) => format!("{} · none on grid", ship_count_text(ships)),
            (ships, on_grid) => format!("{} · {on_grid} on grid", ship_count_text(ships)),
        })
        .color(text_color),
    )
    .on_hover_text(format!(
        "On grid means within {} km of you",
        live_intel::ON_GRID_KM as u64
    ));
    let threats = report.threats();
    if !threats.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (class, count, on_grid) in threats {
                let mut text = class.count_text(count);
                if on_grid > 0 {
                    text.push_str(&format!(" ({on_grid} on grid)"));
                }
                ui.label(egui::RichText::new(text).color(if fresh {
                    WARNING_TEXT
                } else {
                    theme::muted_text()
                }));
            }
        });
    }
    let classes = report
        .ship_entries()
        .take(6)
        .map(|entry| live_intel::group_count_text(&entry.label, entry.count))
        .collect::<Vec<_>>();
    if !classes.is_empty() {
        ui.label(egui::RichText::new(classes.join(" · ")).color(text_color));
    }
    if report.unidentified > 0 || !report.lookup_error.is_empty() {
        let label = ui.label(
            egui::RichText::new(format!(
                "{} not identified",
                if report.unidentified == 1 {
                    "1 result".to_owned()
                } else {
                    format!("{} results", report.unidentified)
                }
            ))
            .color(if report.lookup_error.is_empty() {
                theme::muted_text()
            } else {
                WARNING_TEXT
            }),
        );
        if !report.lookup_error.is_empty() {
            label.on_hover_text(&report.lookup_error);
        }
    }
    if !report.entries.is_empty() {
        egui::CollapsingHeader::new(format!("Everything on scan ({})", report.rows))
            .id_salt(("dscan_entries", system_id))
            .show(ui, |ui| {
                for entry in &report.entries {
                    let mut text = live_intel::group_count_text(&entry.label, entry.count);
                    if entry.on_grid > 0 {
                        text.push_str(&format!(" · {} on grid", entry.on_grid));
                    }
                    ui.label(
                        egui::RichText::new(text).color(if entry.threat.is_some() && fresh {
                            WARNING_TEXT
                        } else if entry.ship {
                            ui.visuals().text_color()
                        } else {
                            theme::muted_text()
                        }),
                    );
                }
            });
    }
}

/// "D-scan · 14 ships, 3 on grid · 2 interdictors".
fn dscan_status(report: &DscanReport) -> String {
    let mut parts = vec![
        "D-scan".to_owned(),
        format!(
            "{}, {} on grid",
            ship_count_text(report.ships()),
            report.ships_on_grid()
        ),
    ];
    parts.extend(
        report
            .threats()
            .into_iter()
            .take(2)
            .map(|(class, count, _)| class.count_text(count)),
    );
    parts.join(" · ")
}

fn pilot_count_text(count: usize) -> String {
    match count {
        0 => "No other pilots".to_owned(),
        1 => "1 pilot".to_owned(),
        count => format!("{count} pilots"),
    }
}

fn ship_count_text(count: usize) -> String {
    match count {
        1 => "1 ship".to_owned(),
        count => format!("{count} ships"),
    }
}

#[cfg(test)]
mod tests {
    use super::{dscan_panel, dscan_status, paste_heading, pilot_count_text};
    use crate::live_intel::{DscanEntry, DscanReport, LIVE_INTEL_FRESH_SECS, ThreatClass};
    use eframe::egui;

    fn report(captured_at: u64) -> DscanReport {
        DscanReport {
            captured_at,
            rows: 5,
            entries: vec![
                DscanEntry {
                    label: "Interdictor".to_owned(),
                    ship: true,
                    threat: Some(ThreatClass::Interdictor),
                    count: 2,
                    on_grid: 1,
                },
                DscanEntry {
                    label: "Moon".to_owned(),
                    count: 3,
                    ..DscanEntry::default()
                },
            ],
            unidentified: 1,
            lookup_error: "ESI lookup returned 502 Bad Gateway".to_owned(),
        }
    }

    #[test]
    fn status_lines_read_plainly() {
        assert_eq!(
            dscan_status(&report(0)),
            "D-scan · 2 ships, 1 on grid · 2 interdictors"
        );
        assert_eq!(pilot_count_text(0), "No other pilots");
        assert_eq!(pilot_count_text(1), "1 pilot");
    }

    #[test]
    fn fresh_and_earlier_pastes_draw() {
        let ctx = egui::Context::default();
        let now = 10_000;
        for captured_at in [now - 60, now - LIVE_INTEL_FRESH_SECS] {
            let mut fresh = None;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    fresh = Some(paste_heading(ui, "Local", captured_at, now));
                    dscan_panel(ui, "sys", &report(captured_at), now);
                });
            });
            assert_eq!(fresh, Some(captured_at == now - 60));
        }
    }
}
