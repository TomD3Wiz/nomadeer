//! The chain map on screen: a view the player pans and zooms, systems they can drag
//! into place, and connections drawn by dragging from one system to another.

use super::chain_view::{self, CONNECTION_PICK_DISTANCE, Camera, HANDLE_RADIUS, NODE_RADIUS};
use super::{ChainMapTarget, NomadeerApp};
use crate::config::theme;
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
                    let to_system_id = drop.and_then(|pos| {
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

        let mut refit = false;
        match self.chain_map_controls(ui, rect, camera.zoom) {
            Some(MapControl::ZoomIn) => {
                camera = camera.zoomed_at(rect, rect.center(), chain_view::ZOOM_STEP);
            }
            Some(MapControl::ZoomOut) => {
                camera = camera.zoomed_at(rect, rect.center(), 1.0 / chain_view::ZOOM_STEP);
            }
            Some(MapControl::Fit) => camera = Camera::fit(rect, bounds),
            Some(MapControl::AskResetLayout) => self.chain_confirm_reset_layout = true,
            Some(MapControl::KeepLayout) => self.chain_confirm_reset_layout = false,
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

        if let Some(from_system_id) =
            chain_view::handle_at(&self.data.systems, positions, camera, rect, origin)
        {
            return Some(ChainDrag::Link { from_system_id });
        }
        let point = camera.to_map(rect, origin);
        let Some(system_id) = chain_view::system_at(&self.data.systems, positions, point) else {
            return Some(ChainDrag::Pan);
        };
        if response.ctx.input(|input| input.modifiers.shift) {
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
        let hovered_handle = pointer.filter(|_| !dragging).and_then(|pos| {
            chain_view::handle_at(&self.data.systems, positions, camera, rect, pos)
        });
        let hovered_system_id = pointer_point
            .and_then(|point| chain_view::system_at(&self.data.systems, positions, point))
            .or_else(|| hovered_handle.clone());
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
                let endpoints = if distance > 2.0 * NODE_RADIUS {
                    let offset = delta / distance * NODE_RADIUS;
                    [screen(&(*a + offset)), screen(&(*b - offset))]
                } else {
                    [screen(a), screen(b)]
                };
                let is_hovered = hovered_connection_id.as_deref() == Some(&connection.id);
                painter.line_segment(
                    endpoints,
                    Stroke::new(
                        if is_hovered { 4.0 } else { 2.0 } * line_scale,
                        if is_hovered { Color32::WHITE } else { color },
                    ),
                );
            }
        }

        let name_alpha = chain_view::name_opacity(zoom);
        let detail_alpha = chain_view::detail_opacity(zoom);
        let radius = NODE_RADIUS * zoom;
        for system in &self.data.systems {
            let Some(pos) = positions.get(&system.id).map(screen) else {
                continue;
            };
            let is_hovered = hovered_system_id.as_deref() == Some(&system.id);
            let fill = match system.status.as_str() {
                "home" => Color32::from_rgb(20, 89, 82),
                "hostile" => Color32::from_rgb(92, 31, 43),
                "farmable" => Color32::from_rgb(37, 75, 42),
                "rolled" => Color32::from_rgb(45, 48, 56),
                _ => Color32::from_rgb(16, 41, 50),
            };
            painter.circle_filled(pos, radius, fill);
            let stroke = if system.id == self.selected_system_id {
                Stroke::new(4.0 * line_scale, theme::character_text())
            } else if linking_id == Some(&system.id) {
                Stroke::new(4.0 * line_scale, LINK_COLOR)
            } else if is_hovered || moving_id == Some(&system.id) {
                Stroke::new(4.0 * line_scale, Color32::WHITE)
            } else {
                Stroke::new(2.0 * line_scale, theme::accent())
            };
            painter.circle_stroke(pos, radius, stroke);
            if name_alpha > 0.0 {
                painter.text(
                    pos + vec2(0.0, -6.0) * zoom,
                    Align2::CENTER_CENTER,
                    &system.name,
                    FontId::proportional(14.0 * zoom),
                    Color32::WHITE.gamma_multiply(name_alpha),
                );
            }
            if detail_alpha > 0.0 {
                painter.text(
                    pos + vec2(0.0, 12.0) * zoom,
                    Align2::CENTER_CENTER,
                    Self::inferred_system_class(system),
                    FontId::proportional(12.0 * zoom),
                    theme::muted_text().gamma_multiply(detail_alpha),
                );
            }
        }

        if !dragging
            && let Some(system_id) = &hovered_system_id
            && let Some(center) = positions.get(system_id)
        {
            let handle = screen(&chain_view::connector_handle(*center));
            let on_handle = hovered_handle.as_deref() == Some(system_id.as_str());
            let (fill, mark) = if on_handle {
                (LINK_COLOR, theme::panel_background())
            } else {
                (theme::panel_background(), LINK_COLOR)
            };
            painter.circle_filled(handle, HANDLE_RADIUS, fill);
            painter.circle_stroke(handle, HANDLE_RADIUS, Stroke::new(1.5_f32, LINK_COLOR));
            let arm = HANDLE_RADIUS * 0.5;
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
            (None, Some(system_id), _) if hovered_handle.is_some() => Some(format!(
                "Drag to another system to link {}",
                self.system_name(system_id)
            )),
            (None, Some(system_id), _) => Some(format!(
                "{} · right-click for actions",
                self.system_name(system_id)
            )),
            (None, None, Some(connection_id)) => self
                .data
                .connections
                .iter()
                .find(|connection| connection.id == *connection_id)
                .map(|connection| {
                    format!(
                        "{} ↔ {} · right-click for actions",
                        self.system_name(&connection.from_system),
                        self.system_name(&connection.to_system)
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

    /// Zoom, Fit and Reset layout, in the map's top-right corner.
    fn chain_map_controls(&self, ui: &mut egui::Ui, rect: Rect, zoom: f32) -> Option<MapControl> {
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
        egui::Frame::new()
            .fill(theme::panel_background().gamma_multiply(0.92))
            .stroke(Stroke::new(1.0_f32, theme::panel_stroke()))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(&mut controls, |ui| {
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
            });
        control
    }

    /// The right-click menu for whatever was under the pointer.
    fn chain_map_menu(&mut self, response: &egui::Response) {
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
}
