//! Pasting from the game: Ctrl+V anywhere outside a text field, or a Paste button,
//! reads the clipboard and applies it to the selected system. Probe scans update its
//! signatures, which the sidebar lists; local and d-scans are read in `live`.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, PopupCloseBehavior,
    containers::menu::{MenuButton, MenuConfig, SubMenuButton},
};

use super::wormholes::{ConnectionAction, SignatureAction, state_color, wormhole_type_picker};
use super::{NomadeerApp, WARNING_TEXT};
use crate::config::theme;
use crate::local_intel;
use crate::model::Signature;
use crate::paste::{self, Paste};
use crate::probe_scan::{ProbeScan, ScanGroup, SiteKind};
use crate::signatures;
use crate::util::now;
use crate::wormhole;

/// How long the Paste scan button waits for the clipboard before giving up.
const PASTE_WAIT: Duration = Duration::from_secs(1);

/// What the last probe scan pasted into a system did, kept beside its signatures.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ScanNotice {
    pub(super) system_id: String,
    pub(super) summary: String,
    /// Signatures missing from the scan, offered for removal.
    pub(super) gone: Vec<String>,
}

impl NomadeerApp {
    /// Takes this frame's paste, if it is meant for Nomadeer rather than a text field.
    pub(super) fn handle_paste(&mut self, ctx: &egui::Context) {
        let requested = self
            .paste_requested
            .is_some_and(|at| at.elapsed() < PASTE_WAIT);
        let pasted = ctx.input(|input| {
            input.events.iter().rev().find_map(|event| match event {
                egui::Event::Paste(text) => Some(text.clone()),
                _ => None,
            })
        });
        match pasted {
            Some(text) if requested || !ctx.wants_keyboard_input() => {
                self.paste_requested = None;
                self.apply_paste(&text);
            }
            _ if self.paste_requested.is_some() && !requested => {
                self.paste_requested = None;
                self.status = "Nothing to paste. Copy the probe scanner, local or the d-scan first"
                    .to_owned();
            }
            _ => {}
        }
    }

    /// Reads the clipboard on the next frame, as Ctrl+V would.
    pub(super) fn request_paste(&mut self, ctx: &egui::Context) {
        self.paste_requested = Some(Instant::now());
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        ctx.request_repaint();
        ctx.request_repaint_after(PASTE_WAIT);
    }

    /// Where every paste from the game lands.
    pub(super) fn apply_paste(&mut self, text: &str) {
        match paste::classify(text) {
            Paste::ProbeScan(scan) => self.apply_probe_scan(&scan),
            Paste::Local(pilots) => self.apply_local_paste(&pilots),
            Paste::DirectionalScan(rows) => self.apply_directional_scan(rows),
            Paste::Unrecognised => {
                self.status = "That paste is not a probe scan, local list or d-scan".to_owned();
            }
        }
    }

    fn apply_probe_scan(&mut self, scan: &ProbeScan) {
        let Some(system) = self.selected_system() else {
            self.status =
                "Select a system on the chain map before pasting its probe scan".to_owned();
            return;
        };
        let system_id = system.id.clone();
        let system_name = system.name.clone();
        let in_wormhole_space = local_intel::is_wormhole_space(system);
        let mut signatures = std::mem::take(&mut self.data.signatures);
        let merge = signatures::merge_probe_scan(&mut signatures, &system_id, scan, now(), || {
            self.new_id("sig")
        });
        self.data.signatures = signatures;
        if !merge.added.is_empty() || !merge.updated.is_empty() {
            self.save_quietly();
        }
        let mut summary = format!("Probe scan · {}", merge.summary());
        if in_wormhole_space
            && let Some((connection_id, code)) =
                signatures::sure_link(&self.data.signatures, &self.data.connections, &system_id)
        {
            let far = self
                .data
                .connections
                .iter()
                .find(|connection| connection.id == connection_id)
                .and_then(|connection| wormhole::other_side(connection, &system_id))
                .map(|far| self.system_name(far))
                .unwrap_or_default();
            self.apply_connection_action(
                &connection_id,
                ConnectionAction::LinkSignature {
                    system_id: system_id.clone(),
                    code: code.clone(),
                },
            );
            summary.push_str(&format!(" · {code} leads to {far}"));
        }
        self.status = format!("{summary} in {system_name}");
        if !merge.gone.is_empty() {
            self.status
                .push_str(&format!(" · {} no longer on scan", merge.gone.len()));
        }
        self.scan_notice = Some(ScanNotice {
            system_id,
            summary,
            gone: merge.gone,
        });
    }

    fn remove_gone_signatures(&mut self) {
        let Some(notice) = self.scan_notice.as_mut() else {
            return;
        };
        let gone = std::mem::take(&mut notice.gone);
        let system_id = notice.system_id.clone();
        let removed = signatures::remove_signatures(
            &mut self.data.signatures,
            &mut self.data.connections,
            &system_id,
            &gone,
        );
        if removed > 0 {
            self.save();
            self.status = format!(
                "Removed {removed} signature{} no longer on scan",
                if removed == 1 { "" } else { "s" }
            );
        }
    }

    /// The selected system's signatures, with the Paste scan button.
    pub(super) fn signatures_section(&mut self, ui: &mut egui::Ui, system_id: &str) {
        let mut system_signatures = self
            .data
            .signatures
            .iter()
            .filter(|signature| signature.system_id == system_id)
            .cloned()
            .collect::<Vec<_>>();
        system_signatures.sort_by(|a, b| {
            (!signatures::is_wormhole(a), &a.code).cmp(&(!signatures::is_wormhole(b), &b.code))
        });
        let (anomalies, signatures): (Vec<_>, Vec<_>) = system_signatures
            .into_iter()
            .partition(|signature| signatures::group_kind(signature) == ScanGroup::Anomaly);

        ui.horizontal(|ui| {
            ui.strong(format!("Signatures ({})", signatures.len()));
            if ui
                .button("Paste scan")
                .on_hover_text("Paste the probe scanner's rows (Ctrl+A, Ctrl+C in the scanner)")
                .clicked()
            {
                self.request_paste(ui.ctx());
            }
        });

        let notice = self
            .scan_notice
            .clone()
            .filter(|notice| notice.system_id == system_id);
        let mut remove_gone = false;
        let mut keep_gone = false;
        if let Some(notice) = &notice {
            ui.label(egui::RichText::new(&notice.summary).color(theme::accent()));
            if !notice.gone.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} gone", notice.gone.len()))
                            .color(WARNING_TEXT),
                    )
                    .on_hover_text(format!(
                        "Not on the scanner any more: {}",
                        notice.gone.join(", ")
                    ));
                    if ui
                        .small_button("Remove")
                        .on_hover_text("Remove them from the chain")
                        .clicked()
                    {
                        remove_gone = true;
                    }
                    if ui.small_button("Keep").clicked() {
                        keep_gone = true;
                    }
                });
            }
        } else if signatures.is_empty() && anomalies.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Copy the probe scanner's rows in game, then press Paste scan or Ctrl+V here.",
                )
                .color(theme::muted_text()),
            );
        }
        if remove_gone {
            self.remove_gone_signatures();
        }
        if keep_gone && let Some(notice) = self.scan_notice.as_mut() {
            notice.gone.clear();
        }

        let current_time = now();
        let mut chosen = None;
        for signature in &signatures {
            if let Some(action) = self.signature_row(ui, signature, current_time) {
                chosen = Some((signature.code.clone(), action));
            }
        }
        if !anomalies.is_empty() {
            egui::CollapsingHeader::new(format!("Anomalies ({})", anomalies.len()))
                .id_salt(("anomalies", system_id))
                .show(ui, |ui| {
                    for anomaly in &anomalies {
                        if let Some(action) = self.signature_row(ui, anomaly, current_time) {
                            chosen = Some((anomaly.code.clone(), action));
                        }
                    }
                });
        }
        if let Some((code, action)) = chosen {
            self.apply_signature_action(system_id, &code, action);
        }
    }

    /// One signature: its ID (a menu), what it is, where it leads and its age.
    fn signature_row(
        &self,
        ui: &mut egui::Ui,
        signature: &Signature,
        current_time: u64,
    ) -> Option<SignatureAction> {
        let kind = signatures::site_kind(signature);
        let is_wormhole = kind == SiteKind::Wormhole;
        let linked = signatures::linked_connection(
            &self.data.connections,
            &signature.system_id,
            &signature.code,
        );
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let id_color = if is_wormhole {
                theme::wormhole()
            } else {
                ui.visuals().text_color()
            };
            let (_, inner) = MenuButton::new(
                egui::RichText::new(&signature.code)
                    .monospace()
                    .color(id_color),
            )
            .config(MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside))
            .ui(ui, |ui| self.signature_menu(ui, signature, kind));
            if let Some(chosen) = inner.and_then(|inner| inner.inner) {
                action = Some(chosen);
            }

            let what = match kind {
                SiteKind::Unknown if signature.signal > 0.0 => {
                    format!("Unscanned {:.0}%", signature.signal)
                }
                SiteKind::Unknown => "Unscanned".to_owned(),
                _ => signature.sig_type.clone(),
            };
            ui.label(egui::RichText::new(what).color(if is_wormhole {
                theme::wormhole()
            } else {
                ui.visuals().text_color()
            }));

            let seen_type = linked
                .and_then(|connection| wormhole::type_on_side(connection, &signature.system_id))
                .map(|wormhole| wormhole.code.to_owned())
                .or_else(|| {
                    (!signature.wormhole_code.is_empty()).then(|| signature.wormhole_code.clone())
                });
            if let Some(code) = seen_type {
                ui.label(egui::RichText::new(code).strong().color(theme::wormhole()));
            }
            if let Some(connection) = linked {
                if let Some(far) = wormhole::other_side(connection, &signature.system_id) {
                    ui.label(egui::RichText::new("→").color(theme::muted_text()));
                    ui.label(
                        egui::RichText::new(self.system_name(far)).color(theme::system_text()),
                    );
                }
                let state = wormhole::state_text(connection, current_time);
                if !state.is_empty() {
                    ui.label(egui::RichText::new(state).color(state_color(connection)));
                }
            } else if !is_wormhole && !signature.target.is_empty() {
                ui.label(egui::RichText::new(&signature.target).color(theme::muted_text()));
            }
            ui.label(
                egui::RichText::new(wormhole::short_duration(
                    current_time.saturating_sub(signature.created_at),
                ))
                .color(theme::muted_text()),
            )
            .on_hover_text("Time since it was first scanned");
        });
        action
    }

    fn signature_menu(
        &self,
        ui: &mut egui::Ui,
        signature: &Signature,
        kind: SiteKind,
    ) -> Option<SignatureAction> {
        let mut action = None;
        let group = if signature.scan_group.is_empty() {
            "Signature"
        } else {
            &signature.scan_group
        };
        ui.label(format!("{} · {group}", signature.code));
        if !signature.target.is_empty() {
            ui.label(egui::RichText::new(&signature.target).color(theme::muted_text()));
        }
        if signature.signal > 0.0 {
            ui.label(
                egui::RichText::new(format!("Signal {:.0}%", signature.signal))
                    .color(theme::muted_text()),
            );
        }
        ui.separator();

        if matches!(kind, SiteKind::Wormhole | SiteKind::Unknown) {
            let linked = signatures::linked_connection(
                &self.data.connections,
                &signature.system_id,
                &signature.code,
            );
            let leads_to = linked
                .and_then(|connection| wormhole::other_side(connection, &signature.system_id))
                .and_then(|far| self.target_class_of(far));
            let (_, inner) = SubMenuButton::new("Wormhole type")
                .config(MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside))
                .ui(ui, |ui| {
                    let chosen = wormhole_type_picker(
                        ui,
                        ui.id()
                            .with(("signature_type", &signature.system_id, &signature.code)),
                        leads_to,
                    )
                    .map(|wormhole| SignatureAction::SetType(wormhole.code));
                    if !signature.wormhole_code.is_empty() {
                        ui.separator();
                        if ui.button("Clear type").clicked() {
                            return Some(SignatureAction::ClearType);
                        }
                    }
                    chosen
                });
            if let Some(chosen) = inner.and_then(|inner| inner.inner) {
                action = Some(chosen);
            }

            let (_, inner) = SubMenuButton::new("Leads to").ui(ui, |ui| {
                let mut chosen = None;
                let mut any = false;
                for connection in &self.data.connections {
                    let Some(far) = wormhole::other_side(connection, &signature.system_id) else {
                        continue;
                    };
                    any = true;
                    let is_linked = linked.is_some_and(|linked| linked.id == connection.id);
                    let name =
                        egui::RichText::new(self.system_name(far)).color(theme::system_text());
                    if ui.selectable_label(is_linked, name).clicked() {
                        chosen = Some(SignatureAction::Link(
                            (!is_linked).then(|| connection.id.clone()),
                        ));
                    }
                }
                if !any {
                    ui.label(
                        egui::RichText::new("No connections from this system yet")
                            .color(theme::muted_text()),
                    );
                }
                chosen
            });
            if let Some(chosen) = inner.and_then(|inner| inner.inner) {
                action = Some(chosen);
            }
            ui.separator();
        }
        if ui.button("Delete signature").clicked() {
            action = Some(SignatureAction::Delete);
        }
        if action.is_some() {
            ui.close();
        }
        action
    }
}
