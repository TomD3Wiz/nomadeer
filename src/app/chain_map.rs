//! The chain map on screen: a view the player pans and zooms, systems they can drag
//! into place, and connections drawn by dragging from one system to another.

use super::chain_art::{self, ConnectionStyle, Detail, Legend, NodeLook};
use super::chain_view::{self, CONNECTION_PICK_DISTANCE, Camera, NODE_RADIUS};
use super::{ChainMapTarget, NomadeerApp, umbra};
use crate::config::theme;
use crate::util::now;
use crate::wormhole;
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, FontId, Layout, PointerButton, Pos2, Rect, Sense,
    Stroke, Vec2, pos2, vec2,
};
use std::collections::HashMap;

/// The hint beside the map's heading.
pub(super) const CHAIN_MAP_HINT: &str =
    "Drag systems to arrange them · drag a rim handle or Shift+drag to link · scroll to zoom";
const LINK_COLOR: Color32 = Color32::from_rgb(255, 194, 87);
/// How fast the scroll wheel zooms, per point scrolled.
const SCROLL_ZOOM_RATE: f32 = 0.0025;

/// What a drag on the map is doing.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ChainDrag {
    /// Moving the view.
    Pan,
    /// Moving a system, held `grab` map units from its centre.
    Move { system_id: String, grab: Vec2 },
    /// Drawing a connection out of a system.
    Link { from_system_id: String },
}

/// What the controls in the map's corner asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapControl {
    ZoomIn,
    ZoomOut,
    Fit,
    AskResetLayout,
    ResetLayout,
    KeepLayout,
    ToggleLegend,
}

impl NomadeerApp {
    pub(super) fn chain_map(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let response = ui.interact(rect, ui.id().with("chain_map"), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 12.0, theme::panel_background());
        painter.rect_stroke(
            rect,
            12.0,
            Stroke::new(1.0_f32, theme::panel_stroke()),
            egui::StrokeKind::Inside,
        );

        if self.data.systems.is_empty() {
            // The first system mapped is fitted into view.
            self.chain_camera = None;
            self.chain_drag = None;
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Sync a character location to start mapping the chain",
                FontId::proportional(18.0),
                theme::muted_text(),
            );
            return;
        }

        let mut positions = chain_view::layout(&self.data.systems, &self.data.connections);
        let bounds = chain_view::chain_bounds(positions.values()).unwrap_or(Rect::ZERO);
        let mut camera = self
            .chain_camera
            .unwrap_or_else(|| Camera::fit(rect, bounds));
        // Not through the corner controls or an open menu.
        let pointer = ui
            .input(|input| input.pointer.hover_pos())
            .filter(|pos| rect.contains(*pos))
            .filter(|_| response.hovered() || response.dragged());

        if response.hovered()
            && let Some(anchor) = pointer
        {
            let (scroll, pinch) =
                ui.input(|input| (input.smooth_scroll_delta.y, input.zoom_delta()));
            let factor = pinch * (scroll * SCROLL_ZOOM_RATE).exp();
            if factor != 1.0 {
                camera = camera.zoomed_at(rect, anchor, factor);
            }
        }

        if response.drag_started() {
            let origin = ui
                .input(|input| input.pointer.press_origin())
                .or(response.interact_pointer_pos());
            self.chain_drag = origin.and_then(|origin| {
                self.start_chain_drag(&response, &positions, camera, rect, origin)
            });
        }

        if response.dragged() {
            match &self.chain_drag {
                Some(ChainDrag::Pan) => camera = camera.panned(response.drag_delta()),
                Some(ChainDrag::Move { system_id, grab }) => {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let target = camera.to_map(rect, pos) - *grab;
                        if let Some(system) = self
                            .data
                            .systems
                            .iter_mut()
                            .find(|system| system.id == *system_id)
                        {
                            system.map_position = Some(chain_view::to_map_position(target));
                            positions.insert(system_id.clone(), target);
                        }
                    }
                }
                Some(ChainDrag::Link { .. }) | None => {}
            }
        }

        // A drag that ended without egui saying so, e.g. the map was rebuilt mid-drag,
        // ends here too, just without making a connection.
        if response.drag_stopped() || (self.chain_drag.is_some() && !response.dragged()) {
            let drop = response
                .interact_pointer_pos()
                .or_else(|| ui.input(|input| input.pointer.latest_pos()));
            match self.chain_drag.take() {
                Some(ChainDrag::Move { .. }) => self.save_quietly(),
                Some(ChainDrag::Link { from_system_id }) if response.drag_stopped() => {
                    // Only a system the player can see takes the connection, not one
                    // off the edge of the map or under its legend or controls.
                    let on_map = |pos: &Pos2| {
                        rect.contains(*pos)
                            && !self
                                .chain_overlays
                                .iter()
                                .any(|overlay| overlay.contains(*pos))
                    };
                    let to_system_id = drop.filter(on_map).and_then(|pos| {
                        chain_view::system_at(
                            &self.data.systems,
                            &positions,
                            camera.to_map(rect, pos),
                        )
                    });
                    match to_system_id {
                        Some(to_system_id) => {
                            self.create_manual_connection(&from_system_id, &to_system_id);
                        }
                        None => {
                            self.status = "Drop on another system to make a connection".to_owned();
                        }
                    }
                }
                _ => {}
            }
        }

        let clicked_system = response
            .interact_pointer_pos()
            .filter(|_| response.clicked() || response.double_clicked())
            .and_then(|pos| {
                chain_view::system_at(&self.data.systems, &positions, camera.to_map(rect, pos))
            });
        if response.clicked()
            && let Some(system_id) = &clicked_system
        {
            self.selected_system_id = system_id.clone();
        }
        if response.double_clicked() && clicked_system.is_none() {
            camera = Camera::fit(rect, bounds);
        }

        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let point = camera.to_map(rect, pos);
            let pick = CONNECTION_PICK_DISTANCE / camera.zoom;
            self.chain_context_target =
                chain_view::system_at(&self.data.systems, &positions, point)
                    .map(ChainMapTarget::System)
                    .or_else(|| {
                        chain_view::connection_at(&self.data.connections, &positions, point, pick)
                            .map(ChainMapTarget::Connection)
                    })
                    .or(Some(ChainMapTarget::Map));
        }

        let bounds = chain_view::chain_bounds(positions.values()).unwrap_or(bounds);
        camera = camera.kept_in_view(rect, bounds);
        self.paint_chain(ui, &painter, rect, camera, &positions, pointer);
        self.chain_overlays.clear();
        if self.chain_show_legend {
            let legend = Legend::fit(rect.height() - 16.0);
            let legend_rect = Rect::from_min_size(
                pos2(rect.left() + 8.0, rect.bottom() - 8.0 - legend.size.y),
                legend.size,
            )
            .intersect(rect.shrink(8.0));
            // Clicks, drags and scrolling on the legend stay off the map beneath it.
            ui.interact(
                legend_rect,
                ui.id().with("chain_map_legend"),
                Sense::click_and_drag(),
            );
            legend.paint(&painter, legend_rect);
            self.chain_overlays.push(legend_rect);
        }

        let mut refit = false;
        let (controls_rect, control) = self.chain_map_controls(ui, rect, camera.zoom);
        self.chain_overlays.push(controls_rect);
        match control {
            Some(MapControl::ZoomIn) => {
                camera = camera.zoomed_at(rect, rect.center(), chain_view::ZOOM_STEP);
            }
            Some(MapControl::ZoomOut) => {
                camera = camera.zoomed_at(rect, rect.center(), 1.0 / chain_view::ZOOM_STEP);
            }
            Some(MapControl::Fit) => camera = Camera::fit(rect, bounds),
            Some(MapControl::AskResetLayout) => self.chain_confirm_reset_layout = true,
            Some(MapControl::KeepLayout) => self.chain_confirm_reset_layout = false,
            Some(MapControl::ToggleLegend) => self.chain_show_legend = !self.chain_show_legend,
            Some(MapControl::ResetLayout) => {
                self.chain_confirm_reset_layout = false;
                for system in &mut self.data.systems {
                    system.map_position = None;
                }
                self.save_quietly();
                self.status = "Arranged the chain automatically".to_owned();
                refit = true;
            }
            None => {}
        }
        self.chain_camera = (!refit).then_some(camera);
        self.chain_map_menu(&response);
    }

    /// Decides what a drag starting at `origin` does: the middle button and empty
    /// space pan, a system's rim handle or Shift on a system draws a connection, and
    /// anywhere else on a system moves it.
    fn start_chain_drag(
        &mut self,
        response: &egui::Response,
        positions: &HashMap<String, Pos2>,
        camera: Camera,
        rect: Rect,
        origin: Pos2,
    ) -> Option<ChainDrag> {
        if response.drag_started_by(PointerButton::Middle) {
            return Some(ChainDrag::Pan);
        }
        if !response.drag_started_by(PointerButton::Primary) {
            return None;
        }

        let Some((system_id, on_handle)) =
            chain_view::target_at(&self.data.systems, positions, camera, rect, origin)
        else {
            return Some(ChainDrag::Pan);
        };
        if on_handle || response.ctx.input(|input| input.modifiers.shift) {
            return Some(ChainDrag::Link {
                from_system_id: system_id,
            });
        }

        // Moving one system fixes the rest where they are, so the automatic
        // arrangement does not shuffle them around it.
        for system in &mut self.data.systems {
            if system.map_position.is_none()
                && let Some(pos) = positions.get(&system.id)
            {
                system.map_position = Some(chain_view::to_map_position(*pos));
            }
        }
        let point = camera.to_map(rect, origin);
        let grab = point - positions.get(&system_id).copied().unwrap_or(point);
        Some(ChainDrag::Move { system_id, grab })
    }

    fn paint_chain(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: Rect,
        camera: Camera,
        positions: &HashMap<String, Pos2>,
        pointer: Option<Pos2>,
    ) {
        let zoom = camera.zoom;
        let line_scale = zoom.clamp(0.5, 1.5);
        let screen = |point: &Pos2| camera.to_screen(rect, *point);
        let dragging = self.chain_drag.is_some();
        let pointer_point = pointer.map(|pos| camera.to_map(rect, pos));
        // While dragging only bodies count, as the drop target; otherwise the handle
        // shown is the one a drag from here would start at.
        let (hovered_system_id, on_handle) = if dragging {
            let system_id = pointer_point
                .and_then(|point| chain_view::system_at(&self.data.systems, positions, point));
            (system_id, false)
        } else {
            pointer
                .and_then(|pos| {
                    chain_view::target_at(&self.data.systems, positions, camera, rect, pos)
                })
                .map_or((None, false), |(system_id, on_handle)| {
                    (Some(system_id), on_handle)
                })
        };
        let hovered_connection_id = if hovered_system_id.is_none() && !dragging {
            pointer_point.and_then(|point| {
                chain_view::connection_at(
                    &self.data.connections,
                    positions,
                    point,
                    CONNECTION_PICK_DISTANCE / zoom,
                )
            })
        } else {
            None
        };
        let (moving_id, linking_id) = match &self.chain_drag {
            Some(ChainDrag::Move { system_id, .. }) => (Some(system_id.as_str()), None),
            Some(ChainDrag::Link { from_system_id }) => (None, Some(from_system_id.as_str())),
            _ => (None, None),
        };

        let name_alpha = chain_view::name_opacity(zoom);
        let detail_alpha = chain_view::detail_opacity(zoom);
        let current_time = now();
        for connection in &self.data.connections {
            if let (Some(a), Some(b)) = (
                positions.get(&connection.from_system),
                positions.get(&connection.to_system),
            ) {
                let delta = *b - *a;
                let distance = delta.length();
                let endpoints = if distance > 2.0 * NODE_RADIUS {
                    let offset = delta / distance * NODE_RADIUS;
                    [screen(&(*a + offset)), screen(&(*b - offset))]
                } else {
                    [screen(a), screen(b)]
                };
                // Wormhole codes stay readable as long as system names do.
                chain_art::paint_connection(
                    painter,
                    endpoints,
                    &ConnectionStyle::of(connection, current_time),
                    line_scale,
                    zoom,
                    hovered_connection_id.as_deref() == Some(&connection.id),
                    name_alpha,
                );
            }
        }

        let radius = NODE_RADIUS * zoom;
        let detail = chain_art::detail_for_radius(radius);
        // A wormhole turns only while the pointer is on it, so an idle map does not
        // keep redrawing, and not at all when animations are switched off.
        let still = ui.style().animation_time <= 0.0 || detail == Detail::Minimal;
        let time = ui.input(|input| input.time);
        let mut turning = false;
        let mut characters: HashMap<&str, usize> = HashMap::new();
        for account in &self.data.accounts {
            if !account.last_system_id.is_empty() {
                *characters
                    .entry(account.last_system_id.as_str())
                    .or_default() += 1;
            }
        }
        let visible = rect.expand(radius * 1.5 + 60.0);
        for system in &self.data.systems {
            let Some(pos) = positions.get(&system.id).map(screen) else {
                continue;
            };
            if !visible.contains(pos) {
                continue;
            }
            let is_hovered = hovered_system_id.as_deref() == Some(&system.id);
            let is_selected = system.id == self.selected_system_id;
            let look = chain_art::node_look(system);
            let opacity = chain_art::status_opacity(&system.status);
            let mut phase = chain_art::resting_phase(&system.id);
            if !still && is_hovered && matches!(look, NodeLook::Wormhole(_)) {
                phase += chain_art::spin(time);
                turning = true;
            }

            if is_selected {
                chain_art::paint_glow_ring(painter, pos, radius, theme::accent(), 3.0 * line_scale);
            }
            chain_art::paint_node(painter, pos, radius, look, phase, detail, opacity);
            let ring = if linking_id == Some(&system.id) {
                Some(LINK_COLOR)
            } else if is_hovered || moving_id == Some(&system.id) {
                Some(Color32::WHITE)
            } else {
                None
            };
            if let Some(color) = ring {
                painter.circle_stroke(pos, radius * 1.1, Stroke::new(2.5 * line_scale, color));
            }

            if detail != Detail::Minimal {
                let badge = (radius * 0.3).max(5.0);
                if let NodeLook::KnownSpace {
                    faction: Some(faction),
                    ..
                } = look
                {
                    chain_art::paint_faction_badge(
                        painter,
                        chain_art::rim_point(pos, radius, 45.0),
                        badge,
                        faction,
                        detail,
                        opacity,
                    );
                }
                if let Some(color) = chain_art::status_color(&system.status) {
                    let pip = chain_art::rim_point(pos, radius, 135.0);
                    painter.circle_filled(pip, badge * 0.5, color);
                    painter.circle_stroke(
                        pip,
                        badge * 0.5,
                        Stroke::new(1.0_f32, theme::panel_background()),
                    );
                }
            }
            if let Some(&count) = characters.get(system.id.as_str()) {
                chain_art::paint_count_badge(
                    painter,
                    chain_art::rim_point(pos, radius, 225.0),
                    (radius * 0.27).max(6.0),
                    count,
                );
            }

            // A system not located yet already shows its "?".
            if detail_alpha > 0.0 && look != NodeLook::Unknown {
                chain_art::shadowed_text(
                    painter,
                    pos,
                    Align2::CENTER_CENTER,
                    &umbra::feed_class_label(system),
                    chain_art::label_font(12.0 * zoom),
                    Color32::WHITE.gamma_multiply(detail_alpha * opacity),
                );
            }
            if name_alpha > 0.0 {
                chain_art::shadowed_text(
                    painter,
                    pos + vec2(0.0, radius + 6.0 * zoom),
                    Align2::CENTER_TOP,
                    &system.name,
                    chain_art::label_font(13.0 * zoom),
                    theme::system_text().gamma_multiply(name_alpha),
                );
            }
        }
        if turning {
            ui.ctx().request_repaint_after(chain_art::ANIMATION_FRAME);
        }

        if !dragging
            && let Some(system_id) = &hovered_system_id
            && let Some(center) = positions.get(system_id)
        {
            let handle = screen(&chain_view::connector_handle(*center));
            let handle_radius = chain_view::handle_radius(zoom);
            let (fill, mark) = if on_handle {
                (LINK_COLOR, theme::panel_background())
            } else {
                (theme::panel_background(), LINK_COLOR)
            };
            painter.circle_filled(handle, handle_radius, fill);
            painter.circle_stroke(handle, handle_radius, Stroke::new(1.5_f32, LINK_COLOR));
            let arm = handle_radius * 0.5;
            for offset in [vec2(arm, 0.0), vec2(0.0, arm)] {
                painter.line_segment(
                    [handle - offset, handle + offset],
                    Stroke::new(1.5_f32, mark),
                );
            }
            ui.ctx().set_cursor_icon(if on_handle {
                CursorIcon::Crosshair
            } else {
                CursorIcon::Grab
            });
        }

        match &self.chain_drag {
            Some(ChainDrag::Link { from_system_id }) => {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                if let (Some(start), Some(end)) = (
                    positions.get(from_system_id),
                    ui.input(|input| input.pointer.latest_pos()),
                ) {
                    painter.line_segment(
                        [screen(start), end],
                        Stroke::new(3.0 * line_scale, LINK_COLOR),
                    );
                }
            }
            Some(ChainDrag::Pan | ChainDrag::Move { .. }) => {
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
            None => {}
        }

        let Some(pos) = pointer else {
            return;
        };
        let label = match (&self.chain_drag, &hovered_system_id, &hovered_connection_id) {
            (Some(ChainDrag::Link { from_system_id }), Some(system_id), _)
                if system_id != from_system_id =>
            {
                Some(format!(
                    "Link {} to {}",
                    self.system_name(from_system_id),
                    self.system_name(system_id)
                ))
            }
            (Some(_), _, _) => None,
            (None, Some(system_id), _) if on_handle => Some(format!(
                "Drag to another system to link {}",
                self.system_name(system_id)
            )),
            (None, Some(system_id), _) => {
                // Name who holds the space the badge stands for.
                let held_by = self
                    .data
                    .systems
                    .iter()
                    .find(|system| system.id == *system_id)
                    .and_then(|system| match chain_art::node_look(system) {
                        NodeLook::KnownSpace {
                            faction: Some(faction),
                            ..
                        } => Some(format!(" · {}", faction.display_name())),
                        _ => None,
                    })
                    .unwrap_or_default();
                Some(format!(
                    "{}{held_by} · right-click for actions",
                    self.system_name(system_id)
                ))
            }
            (None, None, Some(connection_id)) => self
                .data
                .connections
                .iter()
                .find(|connection| connection.id == *connection_id)
                .map(|connection| {
                    let mut facts = Vec::new();
                    if let Some(wormhole) = wormhole::label_type(connection) {
                        facts.push(wormhole::type_summary(wormhole));
                    }
                    let state = wormhole::state_text(connection, current_time);
                    if !state.is_empty() {
                        facts.push(state);
                    }
                    facts.push("right-click for actions".to_owned());
                    format!(
                        "{} ↔ {}\n{}",
                        self.system_name(&connection.from_system),
                        self.system_name(&connection.to_system),
                        facts.join("\n")
                    )
                }),
            (None, None, None) => None,
        };
        if let Some(label) = label {
            painter.text(
                pos + Vec2::new(14.0, 14.0),
                Align2::LEFT_TOP,
                label,
                FontId::proportional(13.0),
                Color32::WHITE,
            );
        }
    }

    /// Zoom, Fit and Reset layout, in the map's top-right corner. Returns where they
    /// are drawn with what the player pressed.
    fn chain_map_controls(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        zoom: f32,
    ) -> (Rect, Option<MapControl>) {
        let row_height = ui.spacing().interact_size.y + 8.0;
        let strip = Rect::from_min_max(
            pos2(rect.left() + 8.0, rect.top() + 8.0),
            pos2(
                rect.right() - 8.0,
                (rect.top() + 8.0 + row_height).min(rect.bottom()),
            ),
        );
        let mut controls = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("chain_map_controls")
                .max_rect(strip)
                .layout(Layout::right_to_left(Align::Center)),
        );
        controls.set_clip_rect(rect.intersect(ui.clip_rect()));

        // Laid out right to left, so the controls are added last first.
        let mut control = None;
        // The whole strip, its gaps, labels and greyed-out buttons too, keeps clicks,
        // drags and scrolling off the systems beneath it.
        let shown = controls.scope_builder(
            egui::UiBuilder::new().sense(Sense::click_and_drag()),
            |strip| {
                egui::Frame::new()
                    .fill(theme::panel_background().gamma_multiply(0.92))
                    .stroke(Stroke::new(1.0_f32, theme::panel_stroke()))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(strip, |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if self.chain_confirm_reset_layout {
                            if ui.small_button("Keep").clicked() {
                                control = Some(MapControl::KeepLayout);
                            }
                            if ui.small_button("Reset").clicked() {
                                control = Some(MapControl::ResetLayout);
                            }
                            ui.label(
                                egui::RichText::new("Arrange every system automatically?")
                                    .color(theme::muted_text()),
                            );
                        } else {
                            let placed = self
                                .data
                                .systems
                                .iter()
                                .any(|system| system.map_position.is_some());
                            if ui
                        .add_enabled(placed, egui::Button::new("Reset layout").small())
                        .on_hover_text(
                            "Forget where systems were dragged and arrange the chain automatically",
                        )
                        .on_disabled_hover_text(
                            "Systems are arranged automatically until you drag one",
                        )
                        .clicked()
                    {
                        control = Some(MapControl::AskResetLayout);
                    }
                        }
                        ui.separator();
                        if ui
                            .small_button("Fit")
                            .on_hover_text("Show the whole chain (or double-click empty space)")
                            .clicked()
                        {
                            control = Some(MapControl::Fit);
                        }
                        if ui.small_button("+").on_hover_text("Zoom in").clicked() {
                            control = Some(MapControl::ZoomIn);
                        }
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", zoom * 100.0))
                                .monospace()
                                .color(theme::muted_text()),
                        );
                        if ui.small_button("−").on_hover_text("Zoom out").clicked() {
                            control = Some(MapControl::ZoomOut);
                        }
                        ui.separator();
                        if ui
                            .add(
                                egui::Button::new("Legend")
                                    .small()
                                    .selected(self.chain_show_legend),
                            )
                            .on_hover_text(
                                "What the systems, badges and connections on the map mean",
                            )
                            .clicked()
                        {
                            control = Some(MapControl::ToggleLegend);
                        }
                    });
            },
        );
        (shown.response.rect, control)
    }

    /// The right-click menu for whatever was under the pointer.
    fn chain_map_menu(&mut self, response: &egui::Response) {
        let menu_target = self
            .chain_context_target
            .clone()
            .unwrap_or(ChainMapTarget::Map);
        let mut select_system_id = None;
        let mut delete_system_id = None;
        let mut connection_action = None;
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
                match self
                    .data
                    .connections
                    .iter()
                    .find(|connection| connection.id == *connection_id)
                {
                    Some(connection) => {
                        if let Some(action) = self.connection_menu(ui, connection) {
                            connection_action = Some((connection_id.clone(), action));
                        }
                    }
                    None => {
                        ui.label("This connection is gone");
                    }
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
        if let Some((connection_id, action)) = connection_action {
            self.apply_connection_action(&connection_id, action);
        }
        if let Some(system_id) = delete_system_id {
            self.delete_system(&system_id);
        }
        if clear_chain {
            self.clear_chain();
        }
    }
}
