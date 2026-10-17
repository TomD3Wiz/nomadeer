//! Wormhole connections in the UI: the player sets each one's type, mass and age
//! from the chain map's right-click menu or the Connections tab, and links it to the
//! signatures it was found by.

use super::{NomadeerApp, WARNING_TEXT, umbra};
use crate::config::theme;
use crate::data::wormhole_types::{self, TargetClass, WormholeType};
use crate::model::Connection;
use crate::probe_scan::{ScanGroup, SiteKind};
use crate::signatures;
use crate::util::now;
use crate::widgets::record_frame;
use crate::wormhole::{self, LifeState, MassState};
use eframe::egui::{
    self, PopupCloseBehavior,
    containers::menu::{MenuButton, MenuConfig, SubMenuButton},
};

/// Pochven's region: its systems are null-sec by security, but a wormhole type of
/// their own.
const POCHVEN_REGION_ID: u64 = 10_000_070;

/// Something the player asked of a connection.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ConnectionAction {
    MarkEol,
    Mass(MassState),
    Reset,
    /// The type read off its signature in `system_id`.
    SetType {
        system_id: String,
        code: &'static str,
    },
    ClearType,
    /// Links the signature `code` in `system_id`; an empty code unlinks that side.
    LinkSignature {
        system_id: String,
        code: String,
    },
    Delete,
}

/// Something the player asked of a signature.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum SignatureAction {
    SetType(&'static str),
    ClearType,
    /// Links it to a connection, or unlinks it with None.
    Link(Option<String>),
    Delete,
}

/// A menu that stays open while the player types or ticks boxes in it.
fn sticky_menu() -> MenuConfig {
    MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside)
}

/// "B274 → High-sec · Large · 24h", or "K162 · exit".
fn type_choice_label(wormhole: &WormholeType) -> String {
    match (wormhole.stats, wormhole.jump_mass_category()) {
        (Some(stats), Some(size)) => format!(
            "{} → {} · {} · {}h",
            wormhole.code,
            wormhole.target.label(),
            size.label(),
            stats.max_stable_hours.round() as u64
        ),
        _ => format!("{} · exit", wormhole.code),
    }
}

/// A code box and the types matching it, leading to `leads_to` unless the player
/// asks for all. Returns the type they chose.
pub(super) fn wormhole_type_picker(
    ui: &mut egui::Ui,
    id: egui::Id,
    leads_to: Option<TargetClass>,
) -> Option<&'static WormholeType> {
    let typed_id = id.with("typed");
    let filter_id = id.with("filter");
    let mut typed = ui
        .data_mut(|data| data.get_temp::<String>(typed_id))
        .unwrap_or_default();
    let mut filtered = ui
        .data_mut(|data| data.get_temp::<bool>(filter_id))
        .unwrap_or(true);

    let mut chosen = None;
    let field = ui.add(
        egui::TextEdit::singleline(&mut typed)
            .hint_text("Code, e.g. B274")
            .desired_width(150.0)
            .char_limit(4),
    );
    if let Some(target) = leads_to {
        ui.checkbox(&mut filtered, format!("Only to {}", target.label()));
    }
    let candidates = wormhole::candidate_types(&typed, leads_to.filter(|_| filtered));
    // Enter takes an exact code, or the only match.
    if field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
        chosen = candidates
            .iter()
            .find(|wormhole| wormhole.code.eq_ignore_ascii_case(typed.trim()))
            .or_else(|| (candidates.len() == 1).then(|| &candidates[0]))
            .copied();
    }
    egui::ScrollArea::vertical()
        .id_salt(id.with("list"))
        .max_height(220.0)
        .show(ui, |ui| {
            if candidates.is_empty() {
                ui.label(egui::RichText::new("No type matches").color(theme::muted_text()));
            }
            for wormhole in &candidates {
                if ui
                    .button(
                        egui::RichText::new(type_choice_label(wormhole)).color(theme::wormhole()),
                    )
                    .on_hover_text(wormhole::type_summary(wormhole))
                    .clicked()
                {
                    chosen = Some(*wormhole);
                }
            }
        });

    if chosen.is_some() {
        typed.clear();
    }
    ui.data_mut(|data| {
        data.insert_temp(typed_id, typed);
        data.insert_temp(filter_id, filtered);
    });
    chosen
}

/// The colour for a connection's state line: warning once it is dying or critical.
pub(super) fn state_color(connection: &Connection) -> egui::Color32 {
    if wormhole::life(connection) == LifeState::EndOfLife
        || wormhole::mass(connection) == MassState::Critical
    {
        WARNING_TEXT
    } else {
        theme::muted_text()
    }
}

impl NomadeerApp {
    /// What kind of space a wormhole into `system_id` would lead to, when known.
    pub(super) fn target_class_of(&self, system_id: &str) -> Option<TargetClass> {
        let system = self.system_by_id(system_id)?;
        wormhole::target_for_class(
            &Self::inferred_system_class(system),
            system.region_id == POCHVEN_REGION_ID,
        )
    }

    /// The facts about a system's connections that go into Umbra's evidence.
    pub(super) fn chain_facts_for(&self, system_id: &str) -> Vec<String> {
        wormhole::chain_facts(
            system_id,
            &self.data.systems,
            &self.data.connections,
            now(),
            umbra::feed_class_label,
        )
    }

    /// Brings the chain facts in a system's assessment up to date with the map.
    pub(super) fn refresh_chain_facts(&mut self, system_id: &str) {
        let facts = self.chain_facts_for(system_id);
        let Some(assessment) = self.system_assessments.get_mut(system_id) else {
            return;
        };
        if wormhole::replace_chain_facts(&mut assessment.evidence, facts) {
            let assessment = assessment.clone();
            self.upsert_system_assessment(assessment);
            self.save_quietly();
        }
    }

    /// "J123456 ↔ Jita", for status lines.
    fn connection_name(&self, connection: &Connection) -> String {
        format!(
            "{} ↔ {}",
            self.system_name(&connection.from_system),
            self.system_name(&connection.to_system)
        )
    }

    pub(super) fn apply_connection_action(
        &mut self,
        connection_id: &str,
        action: ConnectionAction,
    ) {
        if action == ConnectionAction::Delete {
            self.delete_connection(connection_id);
            return;
        }
        let Some(index) = self
            .data
            .connections
            .iter()
            .position(|connection| connection.id == connection_id)
        else {
            return;
        };
        let name = self.connection_name(&self.data.connections[index]);
        let current_time = now();

        let message = match action {
            ConnectionAction::LinkSignature { system_id, code } => {
                signatures::link(&mut self.data.connections, connection_id, &system_id, &code);
                // A signature already read as some type tells the connection its type.
                let read_type = self
                    .data
                    .signatures
                    .iter()
                    .find(|signature| {
                        signature.system_id == system_id
                            && signature.code.eq_ignore_ascii_case(&code)
                    })
                    .and_then(|signature| wormhole_types::by_code(&signature.wormhole_code));
                let connection = &mut self.data.connections[index];
                if let Some(read_type) = read_type
                    && wormhole::type_on_side(connection, &system_id).is_none()
                {
                    wormhole::set_type_on_side(connection, &system_id, read_type);
                }
                if code.is_empty() {
                    format!(
                        "{name}: unlinked the signature in {}",
                        self.system_name(&system_id)
                    )
                } else {
                    format!("{name}: linked to {code}")
                }
            }
            action => {
                let connection = &mut self.data.connections[index];
                match action {
                    ConnectionAction::MarkEol => {
                        wormhole::mark_eol(connection, current_time);
                        format!("{name}: marked end of life")
                    }
                    ConnectionAction::Mass(mass) => {
                        wormhole::set_mass(connection, mass);
                        format!("{name}: {}", mass.label().to_lowercase())
                    }
                    ConnectionAction::Reset => {
                        wormhole::reset_state(connection);
                        format!("{name}: mass and age reset")
                    }
                    ConnectionAction::SetType { system_id, code } => {
                        if let Some(wormhole) = wormhole_types::by_code(code) {
                            wormhole::set_type_on_side(connection, &system_id, wormhole);
                        }
                        format!("{name}: {code} in {}", self.system_name(&system_id))
                    }
                    ConnectionAction::ClearType => {
                        wormhole::clear_type(connection);
                        format!("{name}: wormhole type cleared")
                    }
                    ConnectionAction::LinkSignature { .. } | ConnectionAction::Delete => {
                        unreachable!("handled above")
                    }
                }
            }
        };
        self.sync_linked_signature_types(connection_id);
        self.save();
        self.status = message;
    }

    /// Signatures linked to a connection show the type it has on their side.
    fn sync_linked_signature_types(&mut self, connection_id: &str) {
        let Some(connection) = self
            .data
            .connections
            .iter()
            .find(|connection| connection.id == connection_id)
        else {
            return;
        };
        for system_id in [&connection.from_system, &connection.to_system] {
            let Some(code) = signatures::linked_code(connection, system_id) else {
                continue;
            };
            let seen = wormhole::type_on_side(connection, system_id)
                .map(|wormhole| wormhole.code.to_owned())
                .unwrap_or_default();
            for signature in &mut self.data.signatures {
                if signature.system_id == *system_id && signature.code.eq_ignore_ascii_case(code) {
                    signature.wormhole_code.clone_from(&seen);
                }
            }
        }
    }

    pub(super) fn apply_signature_action(
        &mut self,
        system_id: &str,
        code: &str,
        action: SignatureAction,
    ) {
        let linked = signatures::linked_connection(&self.data.connections, system_id, code)
            .map(|connection| connection.id.clone());
        match action {
            SignatureAction::SetType(type_code) => {
                for signature in &mut self.data.signatures {
                    if signature.system_id == system_id && signature.code == code {
                        type_code.clone_into(&mut signature.wormhole_code);
                    }
                }
                match linked {
                    Some(connection_id) => self.apply_connection_action(
                        &connection_id,
                        ConnectionAction::SetType {
                            system_id: system_id.to_owned(),
                            code: type_code,
                        },
                    ),
                    None => {
                        self.save();
                        self.status = format!("{code} is a {type_code}");
                    }
                }
            }
            SignatureAction::ClearType => {
                for signature in &mut self.data.signatures {
                    if signature.system_id == system_id && signature.code == code {
                        signature.wormhole_code.clear();
                    }
                }
                match linked {
                    Some(connection_id) => {
                        self.apply_connection_action(&connection_id, ConnectionAction::ClearType);
                    }
                    None => {
                        self.save();
                        self.status = format!("{code}: wormhole type cleared");
                    }
                }
            }
            SignatureAction::Link(target) => match (target, linked) {
                (Some(connection_id), _) => self.apply_connection_action(
                    &connection_id,
                    ConnectionAction::LinkSignature {
                        system_id: system_id.to_owned(),
                        code: code.to_owned(),
                    },
                ),
                (None, Some(connection_id)) => self.apply_connection_action(
                    &connection_id,
                    ConnectionAction::LinkSignature {
                        system_id: system_id.to_owned(),
                        code: String::new(),
                    },
                ),
                (None, None) => {}
            },
            SignatureAction::Delete => {
                let removed = signatures::remove_signatures(
                    &mut self.data.signatures,
                    &mut self.data.connections,
                    system_id,
                    &[code.to_owned()],
                );
                if removed > 0 {
                    self.save();
                    self.status = format!("Removed {code}");
                }
            }
        }
    }

    /// The type line, state and expiry of a connection.
    fn connection_details(&self, ui: &mut egui::Ui, connection: &Connection, current_time: u64) {
        if let Some(wormhole) = wormhole::label_type(connection) {
            let origin = self.system_name(&connection.wormhole_origin);
            ui.label(
                egui::RichText::new(if wormhole.is_exit() {
                    format!("Spawned in {origin}; its type is not known yet")
                } else {
                    format!("{} · in {origin}", wormhole::type_summary(wormhole))
                })
                .color(theme::wormhole()),
            );
        }
        let state = wormhole::state_text(connection, current_time);
        if !state.is_empty() {
            let marked = wormhole::eol_marked_text(connection, current_time)
                .map(|marked| format!(" · {marked}"))
                .unwrap_or_default();
            ui.label(
                egui::RichText::new(format!("{state}{marked}")).color(state_color(connection)),
            );
        }
    }

    /// The menu entries for a connection: its state, type and signatures. Returns
    /// what the player chose.
    pub(super) fn connection_menu(
        &self,
        ui: &mut egui::Ui,
        connection: &Connection,
    ) -> Option<ConnectionAction> {
        let mut action = None;
        let current_time = now();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(self.system_name(&connection.from_system))
                    .color(theme::system_text()),
            );
            ui.label(egui::RichText::new("↔").color(theme::muted_text()));
            ui.label(
                egui::RichText::new(self.system_name(&connection.to_system))
                    .color(theme::system_text()),
            );
        });
        self.connection_details(ui, connection, current_time);
        if !connection.notes.is_empty() {
            ui.label(egui::RichText::new(&connection.notes).color(theme::muted_text()));
        }
        ui.separator();
        for (label, hint, choice) in [
            (
                "Mark EOL",
                "Under 4 hours left; Nomadeer counts down from now",
                ConnectionAction::MarkEol,
            ),
            (
                "Mass reduced",
                "Under half its mass left",
                ConnectionAction::Mass(MassState::Reduced),
            ),
            (
                "Mass critical",
                "Under a tenth of its mass left",
                ConnectionAction::Mass(MassState::Critical),
            ),
            (
                "Mass stable",
                "Over half its mass left",
                ConnectionAction::Mass(MassState::Stable),
            ),
            (
                "Reset",
                "Forget its mass and age, e.g. after rolling it",
                ConnectionAction::Reset,
            ),
        ] {
            if ui.button(label).on_hover_text(hint).clicked() {
                action = Some(choice);
                ui.close();
            }
        }
        ui.separator();
        let (_, chosen) = SubMenuButton::new("Wormhole type")
            .config(sticky_menu())
            .ui(ui, |ui| self.connection_type_menu(ui, connection));
        if let Some(chosen) = chosen.and_then(|inner| inner.inner) {
            action = Some(chosen);
            ui.close();
        }
        let (_, chosen) = SubMenuButton::new("Signatures")
            .config(sticky_menu())
            .ui(ui, |ui| self.connection_signature_menu(ui, connection));
        if let Some(chosen) = chosen.and_then(|inner| inner.inner) {
            action = Some(chosen);
            ui.close();
        }
        ui.separator();
        if ui.button("Delete connection").clicked() {
            action = Some(ConnectionAction::Delete);
            ui.close();
        }
        action
    }

    /// Which side's signature the code was read from, and the code.
    fn connection_type_menu(
        &self,
        ui: &mut egui::Ui,
        connection: &Connection,
    ) -> Option<ConnectionAction> {
        let side_id = ui.id().with(("wormhole_side", &connection.id));
        let default_side = if connection.wormhole_origin.is_empty() {
            connection.from_system.clone()
        } else {
            connection.wormhole_origin.clone()
        };
        let mut side = ui
            .data_mut(|data| data.get_temp::<String>(side_id))
            .filter(|side| *side == connection.from_system || *side == connection.to_system)
            .unwrap_or(default_side);
        ui.label(egui::RichText::new("Read off the signature in").color(theme::muted_text()));
        ui.horizontal(|ui| {
            for system_id in [&connection.from_system, &connection.to_system] {
                let name =
                    egui::RichText::new(self.system_name(system_id)).color(theme::system_text());
                if ui.selectable_label(side == *system_id, name).clicked() {
                    side.clone_from(system_id);
                }
            }
        });
        ui.data_mut(|data| data.insert_temp(side_id, side.clone()));

        let leads_to =
            wormhole::other_side(connection, &side).and_then(|far| self.target_class_of(far));
        let chosen = wormhole_type_picker(
            ui,
            ui.id().with(("wormhole_type", &connection.id)),
            leads_to,
        )
        .map(|wormhole| ConnectionAction::SetType {
            system_id: side,
            code: wormhole.code,
        });
        if wormhole::label_type(connection).is_some() {
            ui.separator();
            if ui.button("Clear type").clicked() {
                return Some(ConnectionAction::ClearType);
            }
        }
        chosen
    }

    /// The signatures in each system the connection could have been found by.
    fn connection_signature_menu(
        &self,
        ui: &mut egui::Ui,
        connection: &Connection,
    ) -> Option<ConnectionAction> {
        let mut action = None;
        for system_id in [&connection.from_system, &connection.to_system] {
            ui.label(egui::RichText::new(self.system_name(system_id)).color(theme::system_text()));
            let linked = signatures::linked_code(connection, system_id);
            let candidates = self
                .data
                .signatures
                .iter()
                .filter(|signature| signature.system_id == *system_id)
                .filter(|signature| {
                    matches!(
                        signatures::site_kind(signature),
                        SiteKind::Wormhole | SiteKind::Unknown
                    ) && signatures::group_kind(signature) == ScanGroup::Signature
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                ui.label(
                    egui::RichText::new("No wormhole signatures scanned here yet")
                        .color(theme::muted_text()),
                );
            }
            for signature in candidates {
                let is_linked = linked == Some(signature.code.as_str());
                let text = egui::RichText::new(format!(
                    "{} · {}",
                    signature.code,
                    if signature.sig_type.is_empty() {
                        "unscanned"
                    } else {
                        &signature.sig_type
                    }
                ))
                .monospace()
                .color(theme::wormhole());
                if ui.selectable_label(is_linked, text).clicked() {
                    action = Some(ConnectionAction::LinkSignature {
                        system_id: system_id.clone(),
                        code: if is_linked {
                            String::new()
                        } else {
                            signature.code.clone()
                        },
                    });
                }
            }
            ui.add_space(4.0);
        }
        action
    }

    /// The Connections tab: every connection with its type, state and signatures.
    pub(super) fn connections_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Connections");
        let mut connections = self.data.connections.clone();
        connections.sort_by_key(|connection| std::cmp::Reverse(connection.created_at));

        if connections.is_empty() {
            ui.label("No connections yet. Jump through a wormhole, or drag between systems on the chain map.");
            return;
        }

        let current_time = now();
        let mut chosen = None;
        for connection in &connections {
            record_frame(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (index, system_id) in [&connection.from_system, &connection.to_system]
                        .into_iter()
                        .enumerate()
                    {
                        if index == 1 {
                            ui.label(egui::RichText::new("↔").color(theme::muted_text()));
                        }
                        let name = egui::RichText::new(self.system_name(system_id))
                            .color(theme::system_text());
                        if ui
                            .selectable_label(*system_id == self.selected_system_id, name)
                            .clicked()
                        {
                            self.selected_system_id.clone_from(system_id);
                        }
                        if let Some(code) = signatures::linked_code(connection, system_id) {
                            ui.label(
                                egui::RichText::new(code)
                                    .monospace()
                                    .color(theme::wormhole()),
                            );
                        }
                    }
                    let (_, inner) = MenuButton::new("Edit")
                        .config(sticky_menu())
                        .ui(ui, |ui| self.connection_menu(ui, connection));
                    if let Some(action) = inner.and_then(|inner| inner.inner) {
                        chosen = Some((connection.id.clone(), action));
                    }
                });
                self.connection_details(ui, connection, current_time);
                for system_id in [&connection.from_system, &connection.to_system] {
                    if let Some(system) = self.system_by_id(system_id) {
                        ui.label(
                            egui::RichText::new(self.system_intel_summary(system))
                                .color(theme::muted_text()),
                        );
                    }
                }
                if !connection.notes.is_empty() {
                    ui.label(egui::RichText::new(&connection.notes).color(theme::muted_text()));
                }
            });
        }

        if let Some((connection_id, action)) = chosen {
            self.apply_connection_action(&connection_id, action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{type_choice_label, wormhole_type_picker};
    use crate::data::wormhole_types::{self, TargetClass};
    use eframe::egui;

    #[test]
    fn type_choices_name_where_they_lead() {
        assert_eq!(
            type_choice_label(wormhole_types::by_code("B274").unwrap()),
            "B274 → High-sec · Large · 24h"
        );
        assert_eq!(
            type_choice_label(wormhole_types::by_code("K162").unwrap()),
            "K162 · exit"
        );
    }

    #[test]
    fn the_type_picker_draws_and_waits_for_a_choice() {
        let ctx = egui::Context::default();
        for leads_to in [None, Some(TargetClass::HighSec), Some(TargetClass::Pochven)] {
            let mut chosen = Some(None);
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    chosen = Some(wormhole_type_picker(ui, ui.id().with("picker"), leads_to));
                });
            });
            assert_eq!(chosen, Some(None));
        }
    }
}
