//! The chain map's geometry: where systems sit in map units, and the camera that
//! turns map units into points on screen as the player pans and zooms.

use crate::model::{Connection, MapPosition, SystemNode};
use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};
use std::collections::{HashMap, HashSet, VecDeque};

/// A system's radius in map units; at 100% zoom a map unit is a pixel.
pub(super) const NODE_RADIUS: f32 = 38.0;
pub(super) const MIN_ZOOM: f32 = 0.25;
pub(super) const MAX_ZOOM: f32 = 3.0;
/// The zoom buttons' step.
pub(super) const ZOOM_STEP: f32 = 1.25;
/// Fitting never zooms in past this, so a short chain is not blown up.
const MAX_FIT_ZOOM: f32 = 1.0;
/// Pixels kept between the chain and the edge of the map when fitting.
const FIT_MARGIN: f32 = 24.0;
/// Pixels of the chain that panning always leaves on screen.
const KEEP_VISIBLE: f32 = 48.0;
/// Room around each system's centre for its art, its glow and the name beneath it,
/// in map units.
const NODE_EXTENT: f32 = NODE_RADIUS + 30.0;
/// Distance between hops, and between systems the same hop out, in map units.
const COLUMN_SPACING: f32 = 190.0;
const ROW_SPACING: f32 = 110.0;
/// Systems closer than this to one already placed are moved along.
const MIN_SPACING: f32 = 2.0 * NODE_RADIUS + 12.0;
/// The connector handle's radius, in pixels whatever the zoom.
pub(super) const HANDLE_RADIUS: f32 = 7.0;
/// How near a connection the pointer must be to pick it, in pixels.
pub(super) const CONNECTION_PICK_DISTANCE: f32 = 8.0;

/// What part of the map is on screen, and how large.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Camera {
    /// The map point in the middle of the view, so resizing the map keeps it there.
    pub(super) center: Pos2,
    pub(super) zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: Pos2::ZERO,
            zoom: 1.0,
        }
    }
}

impl Camera {
    pub(super) fn to_screen(self, view: Rect, point: Pos2) -> Pos2 {
        view.center() + (point - self.center) * self.zoom
    }

    pub(super) fn to_map(self, view: Rect, point: Pos2) -> Pos2 {
        self.center + (point - view.center()) / self.zoom
    }

    /// Zooms by `factor` within the limits, keeping the map point under `anchor` there.
    pub(super) fn zoomed_at(self, view: Rect, anchor: Pos2, factor: f32) -> Self {
        if !factor.is_finite() || factor <= 0.0 {
            return self;
        }
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let under_anchor = self.to_map(view, anchor);
        Self {
            center: under_anchor - (anchor - view.center()) / zoom,
            zoom,
        }
    }

    /// Moves the map with the pointer by `delta` pixels.
    pub(super) fn panned(self, delta: Vec2) -> Self {
        Self {
            center: self.center - delta / self.zoom,
            ..self
        }
    }

    /// Shows all of `bounds` with a margin, never zoomed in past 100%.
    pub(super) fn fit(view: Rect, bounds: Rect) -> Self {
        let room = (view.size() - Vec2::splat(2.0 * FIT_MARGIN)).max(Vec2::splat(1.0));
        let size = bounds.size().max(Vec2::splat(1.0));
        let zoom = (room.x / size.x)
            .min(room.y / size.y)
            .clamp(MIN_ZOOM, MAX_FIT_ZOOM);
        Self {
            center: bounds.center(),
            zoom,
        }
    }

    /// Pulls the view back until at least a little of `bounds` is on screen, so the
    /// chain can never be panned out of sight.
    pub(super) fn kept_in_view(self, view: Rect, bounds: Rect) -> Self {
        let slack = ((view.size() / 2.0 - Vec2::splat(KEEP_VISIBLE)) / self.zoom).max(Vec2::ZERO);
        let x = self
            .center
            .x
            .clamp(bounds.left() - slack.x, bounds.right() + slack.x);
        let y = self
            .center
            .y
            .clamp(bounds.top() - slack.y, bounds.bottom() + slack.y);
        Self {
            center: pos2(x, y),
            ..self
        }
    }
}

/// The map area the chain covers, with room for each system's circle and name.
pub(super) fn chain_bounds<'a>(positions: impl IntoIterator<Item = &'a Pos2>) -> Option<Rect> {
    let mut points = positions.into_iter().peekable();
    points.peek()?;
    let bounds = points.fold(Rect::NOTHING, |bounds, point| {
        bounds.union(Rect::from_pos(*point))
    });
    Some(bounds.expand(NODE_EXTENT))
}

/// How strongly system names show: fully from 60% zoom, gone below 40%.
pub(super) fn name_opacity(zoom: f32) -> f32 {
    fade_in(zoom, 0.4, 0.6)
}

/// Secondary text, such as the class under a name, goes sooner: from 85% to 65%.
pub(super) fn detail_opacity(zoom: f32) -> f32 {
    fade_in(zoom, 0.65, 0.85)
}

fn fade_in(zoom: f32, hidden_below: f32, shown_from: f32) -> f32 {
    ((zoom - hidden_below) / (shown_from - hidden_below)).clamp(0.0, 1.0)
}

/// Where the handle for dragging out a connection sits on a system's rim, top right.
pub(super) fn connector_handle(system: Pos2) -> Pos2 {
    system + vec2(1.0, -1.0).normalized() * NODE_RADIUS
}

/// The system under a map point; the one drawn last wins where two overlap.
pub(super) fn system_at(
    systems: &[SystemNode],
    positions: &HashMap<String, Pos2>,
    point: Pos2,
) -> Option<String> {
    systems
        .iter()
        .rev()
        .find(|system| {
            positions
                .get(&system.id)
                .is_some_and(|pos| pos.distance(point) <= NODE_RADIUS)
        })
        .map(|system| system.id.clone())
}

/// The system whose connector handle is under the pointer, in screen pixels.
pub(super) fn handle_at(
    systems: &[SystemNode],
    positions: &HashMap<String, Pos2>,
    camera: Camera,
    view: Rect,
    pointer: Pos2,
) -> Option<String> {
    systems
        .iter()
        .rev()
        .find(|system| {
            positions.get(&system.id).is_some_and(|pos| {
                camera
                    .to_screen(view, connector_handle(*pos))
                    .distance(pointer)
                    <= HANDLE_RADIUS + 2.0
            })
        })
        .map(|system| system.id.clone())
}

/// The connection nearest a map point, if within `tolerance` map units of it.
pub(super) fn connection_at(
    connections: &[Connection],
    positions: &HashMap<String, Pos2>,
    point: Pos2,
    tolerance: f32,
) -> Option<String> {
    connections
        .iter()
        .filter_map(|connection| {
            let a = positions.get(&connection.from_system)?;
            let b = positions.get(&connection.to_system)?;
            let distance = distance_to_segment(point, *a, *b);
            (distance <= tolerance).then_some((connection.id.clone(), distance))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(id, _)| id)
}

pub(super) fn distance_to_segment(point: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let length_squared = ab.length_sq();
    if length_squared <= f32::EPSILON {
        return point.distance(a);
    }

    let ap = point - a;
    let t = (ap.dot(ab) / length_squared).clamp(0.0, 1.0);
    point.distance(a + ab * t)
}

pub(super) fn to_map_position(point: Pos2) -> MapPosition {
    MapPosition {
        x: point.x,
        y: point.y,
    }
}

/// Where every system sits, in map units. Systems the player placed stay where they
/// put them. Once any are placed, a system that was not appears beside a connected
/// neighbour, and a group with no placed system keeps its automatic arrangement,
/// moved under the rest. With none placed, the whole chain is arranged automatically.
pub(super) fn layout(systems: &[SystemNode], connections: &[Connection]) -> HashMap<String, Pos2> {
    let mut placed = systems
        .iter()
        .filter_map(|system| {
            let position = system.map_position?;
            (position.x.is_finite() && position.y.is_finite())
                .then(|| (system.id.clone(), pos2(position.x, position.y)))
        })
        .collect::<HashMap<_, _>>();
    let adjacency = adjacency(systems, connections);
    if placed.is_empty() {
        return topology_layout(systems, &adjacency);
    }

    loop {
        let mut progress = false;
        for system in systems {
            if placed.contains_key(&system.id) {
                continue;
            }
            let neighbour = adjacency
                .get(&system.id)
                .into_iter()
                .flatten()
                .find_map(|neighbour| placed.get(neighbour).copied());
            if let Some(neighbour) = neighbour {
                let spot = free_spot(&placed, neighbour + vec2(COLUMN_SPACING, 0.0));
                placed.insert(system.id.clone(), spot);
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }

    let unplaced = systems
        .iter()
        .filter(|system| !placed.contains_key(&system.id))
        .cloned()
        .collect::<Vec<_>>();
    if !unplaced.is_empty() {
        let arranged = topology_layout(&unplaced, &adjacency);
        let above = chain_bounds(placed.values()).unwrap_or(Rect::ZERO);
        let below = chain_bounds(arranged.values()).unwrap_or(Rect::ZERO);
        let shift = pos2(above.left(), above.bottom() + ROW_SPACING / 2.0) - below.left_top();
        for (system_id, pos) in arranged {
            placed.insert(system_id, pos + shift);
        }
    }
    placed
}

/// The first spot down or up from `wanted`, a row at a time, clear of every system
/// already placed.
fn free_spot(placed: &HashMap<String, Pos2>, wanted: Pos2) -> Pos2 {
    (0..16)
        .map(|step: i32| {
            let rows = (step + 1) / 2 * if step % 2 == 0 { -1 } else { 1 };
            wanted + vec2(0.0, rows as f32 * ROW_SPACING)
        })
        .find(|spot| {
            placed
                .values()
                .all(|other| other.distance(*spot) >= MIN_SPACING)
        })
        .unwrap_or(wanted)
}

/// Each system's neighbours, in chain order.
fn adjacency(systems: &[SystemNode], connections: &[Connection]) -> HashMap<String, Vec<String>> {
    let system_order = systems
        .iter()
        .enumerate()
        .map(|(index, system)| (system.id.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut adjacency = systems
        .iter()
        .map(|system| (system.id.clone(), Vec::<String>::new()))
        .collect::<HashMap<_, _>>();

    for connection in connections {
        if connection.from_system == connection.to_system
            || !system_order.contains_key(&connection.from_system)
            || !system_order.contains_key(&connection.to_system)
        {
            continue;
        }

        if let Some(neighbors) = adjacency.get_mut(&connection.from_system) {
            neighbors.push(connection.to_system.clone());
        }
        if let Some(neighbors) = adjacency.get_mut(&connection.to_system) {
            neighbors.push(connection.from_system.clone());
        }
    }

    for neighbors in adjacency.values_mut() {
        neighbors.sort_by_key(|id| system_order.get(id).copied().unwrap_or(usize::MAX));
        neighbors.dedup();
    }
    adjacency
}

/// Lays each connected group out from its best-connected system: one column per hop,
/// groups stacked top to bottom. Neighbours outside `systems` are ignored.
fn topology_layout(
    systems: &[SystemNode],
    adjacency: &HashMap<String, Vec<String>>,
) -> HashMap<String, Pos2> {
    let mut positions = HashMap::<String, Pos2>::new();
    if systems.is_empty() {
        return positions;
    }

    let system_order = systems
        .iter()
        .enumerate()
        .map(|(index, system)| (system.id.clone(), index))
        .collect::<HashMap<_, _>>();
    let neighbours = |system_id: &str| {
        adjacency
            .get(system_id)
            .into_iter()
            .flatten()
            .filter(|id| system_order.contains_key(*id))
    };

    let mut visited = HashSet::<String>::new();
    let mut components = Vec::<Vec<Vec<String>>>::new();
    while visited.len() < systems.len() {
        let Some(root) = systems
            .iter()
            .filter(|system| !visited.contains(&system.id))
            .max_by_key(|system| {
                (
                    neighbours(&system.id).count(),
                    std::cmp::Reverse(system_order.get(&system.id).copied().unwrap_or(0)),
                )
            })
            .map(|system| system.id.clone())
        else {
            break;
        };

        let mut levels = Vec::<Vec<String>>::new();
        let mut queue = VecDeque::from([(root.clone(), 0_usize)]);
        visited.insert(root);

        while let Some((system_id, depth)) = queue.pop_front() {
            if levels.len() <= depth {
                levels.push(Vec::new());
            }
            levels[depth].push(system_id.clone());

            for neighbor in neighbours(&system_id) {
                if visited.insert(neighbor.clone()) {
                    queue.push_back((neighbor.clone(), depth + 1));
                }
            }
        }

        components.push(levels);
    }

    let component_gap = usize::from(components.len() > 1);
    let mut slot_cursor = 0_usize;
    for levels in components {
        let component_slots = levels.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for (depth, nodes) in levels.iter().enumerate() {
            let level_offset = (component_slots.saturating_sub(nodes.len())) as f32 / 2.0;
            for (index, system_id) in nodes.iter().enumerate() {
                let slot = slot_cursor as f32 + level_offset + index as f32;
                positions.insert(
                    system_id.clone(),
                    pos2(depth as f32 * COLUMN_SPACING, slot * ROW_SPACING),
                );
            }
        }
        slot_cursor += component_slots + component_gap;
    }

    positions
}

#[cfg(test)]
mod tests {
    use super::{
        COLUMN_SPACING, Camera, HANDLE_RADIUS, MAX_ZOOM, MIN_SPACING, MIN_ZOOM, NODE_RADIUS,
        chain_bounds, connection_at, connector_handle, detail_opacity, distance_to_segment,
        handle_at, layout, name_opacity, system_at,
    };
    use crate::model::{Connection, MapPosition, SystemNode};
    use eframe::egui::{Pos2, Rect, pos2, vec2};
    use std::collections::HashMap;

    fn view() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 400.0))
    }

    fn assert_near(a: Pos2, b: Pos2) {
        assert!(a.distance(b) < 1e-3, "{a:?} != {b:?}");
    }

    fn system(id: &str) -> SystemNode {
        SystemNode {
            id: id.to_owned(),
            solar_system_id: 0,
            name: id.to_owned(),
            class: String::new(),
            security_status: 0.0,
            constellation_id: 0,
            constellation_name: String::new(),
            region_id: 0,
            region_name: String::new(),
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

    fn placed(id: &str, x: f32, y: f32) -> SystemNode {
        SystemNode {
            map_position: Some(MapPosition { x, y }),
            ..system(id)
        }
    }

    fn link(from: &str, to: &str) -> Connection {
        Connection {
            id: format!("{from}-{to}"),
            from_system: from.to_owned(),
            to_system: to.to_owned(),
            signature_from: String::new(),
            signature_to: String::new(),
            mass_status: String::new(),
            life_status: String::new(),
            ship_size: String::new(),
            notes: String::new(),
            created_at: 0,
        }
    }

    #[test]
    fn screen_and_map_points_round_trip() {
        let camera = Camera {
            center: pos2(40.0, -20.0),
            zoom: 1.7,
        };
        assert_near(camera.to_screen(view(), camera.center), view().center());
        for point in [pos2(0.0, 0.0), pos2(-300.0, 125.5), pos2(1e4, -1e3)] {
            assert_near(
                camera.to_map(view(), camera.to_screen(view(), point)),
                point,
            );
        }
        let shifted = camera.to_screen(view(), camera.center + vec2(10.0, 0.0));
        assert!((shifted.x - view().center().x - 17.0).abs() < 1e-3);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_and_stays_in_range() {
        let camera = Camera::default();
        let cursor = pos2(730.0, 120.0);
        let under = camera.to_map(view(), cursor);
        let zoomed = camera.zoomed_at(view(), cursor, 1.6);
        assert!((zoomed.zoom - 1.6).abs() < 1e-6);
        assert_near(zoomed.to_map(view(), cursor), under);

        let far_in = camera.zoomed_at(view(), cursor, 100.0);
        assert_eq!(far_in.zoom, MAX_ZOOM);
        assert_near(far_in.to_map(view(), cursor), under);
        let far_out = camera.zoomed_at(view(), cursor, 0.001);
        assert_eq!(far_out.zoom, MIN_ZOOM);
        assert_near(far_out.to_map(view(), cursor), under);
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(camera.zoomed_at(view(), cursor, bad), camera);
        }
    }

    #[test]
    fn panning_moves_the_map_with_the_pointer() {
        let camera = Camera {
            center: pos2(10.0, 10.0),
            zoom: 2.0,
        };
        let point = pos2(50.0, 80.0);
        let before = camera.to_screen(view(), point);
        let after = camera.panned(vec2(30.0, -12.0)).to_screen(view(), point);
        assert_near(after, before + vec2(30.0, -12.0));
    }

    #[test]
    fn fitting_shows_the_whole_chain_without_blowing_it_up() {
        let bounds = Rect::from_min_max(pos2(-500.0, -100.0), pos2(1500.0, 300.0));
        let camera = Camera::fit(view(), bounds);
        assert_near(camera.center, bounds.center());
        for corner in [bounds.left_top(), bounds.right_bottom()] {
            assert!(view().contains(camera.to_screen(view(), corner)));
        }
        // It fills the width, the tighter side, apart from the margins.
        let left = camera.to_screen(view(), bounds.left_top()).x;
        assert!((left - view().left() - 24.0).abs() < 1e-3);

        // A single system is shown at 100%, not zoomed to fill the map.
        let one = chain_bounds([&pos2(5.0, 5.0)]).unwrap();
        let camera = Camera::fit(view(), one);
        assert_eq!(camera.zoom, 1.0);
        assert_near(camera.center, pos2(5.0, 5.0));

        // A sprawling chain stops at the furthest zoom out.
        let huge = Rect::from_min_max(pos2(0.0, 0.0), pos2(1e6, 1e6));
        assert_eq!(Camera::fit(view(), huge).zoom, MIN_ZOOM);
        assert_eq!(chain_bounds(std::iter::empty()), None);
    }

    #[test]
    fn the_chain_cannot_be_panned_out_of_sight() {
        let bounds = Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 100.0));
        let lost = Camera {
            center: pos2(1e5, -1e5),
            zoom: 1.0,
        }
        .kept_in_view(view(), bounds);
        let on_screen = view().intersect(Rect::from_min_max(
            lost.to_screen(view(), bounds.min),
            lost.to_screen(view(), bounds.max),
        ));
        assert!(on_screen.width() >= 47.0 && on_screen.height() >= 47.0);

        // A view already showing the chain is left alone.
        let fine = Camera {
            center: pos2(120.0, 40.0),
            zoom: 0.8,
        };
        assert_eq!(fine.kept_in_view(view(), bounds), fine);
    }

    #[test]
    fn names_fade_out_before_the_chain_gets_too_small_and_details_go_first() {
        assert_eq!(name_opacity(1.0), 1.0);
        assert_eq!(name_opacity(0.6), 1.0);
        assert_eq!(name_opacity(0.3), 0.0);
        assert!(name_opacity(0.5) > 0.0 && name_opacity(0.5) < 1.0);
        assert_eq!(detail_opacity(1.0), 1.0);
        assert_eq!(detail_opacity(0.6), 0.0);
        for zoom in [0.25, 0.5, 0.7, 0.9, 2.0] {
            assert!(detail_opacity(zoom) <= name_opacity(zoom), "{zoom}");
        }
    }

    #[test]
    fn hit_testing_works_in_map_units_under_any_camera() {
        let systems = vec![system("a"), system("b")];
        let positions = HashMap::from([
            ("a".to_owned(), pos2(0.0, 0.0)),
            ("b".to_owned(), pos2(30.0, 0.0)),
        ]);
        // Where two overlap, the one drawn last is picked.
        assert_eq!(
            system_at(&systems, &positions, pos2(15.0, 0.0)).as_deref(),
            Some("b")
        );
        assert_eq!(
            system_at(&systems, &positions, pos2(-30.0, 0.0)).as_deref(),
            Some("a")
        );
        assert_eq!(system_at(&systems, &positions, pos2(0.0, 60.0)), None);

        for camera in [
            Camera::default(),
            Camera {
                center: pos2(-80.0, 30.0),
                zoom: 0.4,
            },
            Camera {
                center: pos2(20.0, 5.0),
                zoom: 2.5,
            },
        ] {
            let pointer = camera.to_screen(view(), pos2(-30.0, 0.0));
            let point = camera.to_map(view(), pointer);
            assert_eq!(system_at(&systems, &positions, point).as_deref(), Some("a"));

            let handle = camera.to_screen(view(), connector_handle(pos2(30.0, 0.0)));
            assert_eq!(
                handle_at(&systems, &positions, camera, view(), handle).as_deref(),
                Some("b")
            );
            let beside = handle + vec2(HANDLE_RADIUS + 6.0, 0.0);
            assert_eq!(
                handle_at(&systems, &positions, camera, view(), beside),
                None
            );
        }
        assert!((connector_handle(Pos2::ZERO).distance(Pos2::ZERO) - NODE_RADIUS).abs() < 1e-3);
    }

    #[test]
    fn connections_are_picked_by_their_nearest_segment() {
        let positions = HashMap::from([
            ("a".to_owned(), pos2(0.0, 0.0)),
            ("b".to_owned(), pos2(200.0, 0.0)),
            ("c".to_owned(), pos2(200.0, 100.0)),
        ]);
        let connections = vec![link("a", "b"), link("b", "c")];
        let picked = connection_at(&connections, &positions, pos2(100.0, 5.0), 8.0);
        assert_eq!(picked.as_deref(), Some("a-b"));
        let picked = connection_at(&connections, &positions, pos2(196.0, 50.0), 8.0);
        assert_eq!(picked.as_deref(), Some("b-c"));
        assert_eq!(
            connection_at(&connections, &positions, pos2(100.0, 40.0), 8.0),
            None
        );
        assert_eq!(
            distance_to_segment(pos2(3.0, 4.0), Pos2::ZERO, Pos2::ZERO),
            5.0
        );
    }

    #[test]
    fn without_placed_systems_the_chain_is_laid_out_by_hops() {
        let systems = vec![system("home"), system("c2"), system("hs"), system("c5")];
        let connections = vec![link("home", "c2"), link("home", "hs"), link("c2", "c5")];
        let positions = layout(&systems, &connections);
        assert_eq!(positions.len(), 4);
        assert_eq!(positions["home"].x, 0.0);
        assert_eq!(positions["c2"].x, COLUMN_SPACING);
        assert_eq!(positions["hs"].x, COLUMN_SPACING);
        assert_eq!(positions["c5"].x, 2.0 * COLUMN_SPACING);
        assert_ne!(positions["c2"], positions["hs"]);
    }

    #[test]
    fn placed_systems_stay_put_and_new_ones_appear_beside_a_neighbour() {
        let systems = vec![
            placed("home", 500.0, 300.0),
            placed("c2", 700.0, 300.0),
            system("new"),
            system("newer"),
        ];
        let connections = vec![link("home", "c2"), link("c2", "new"), link("new", "newer")];
        let positions = layout(&systems, &connections);
        assert_eq!(positions["home"], pos2(500.0, 300.0));
        assert_eq!(positions["c2"], pos2(700.0, 300.0));
        assert!(positions["new"].distance(positions["c2"]) <= 2.0 * COLUMN_SPACING);
        assert!(positions["newer"].distance(positions["new"]) <= 2.0 * COLUMN_SPACING);
        for (a, b) in [
            ("new", "home"),
            ("new", "c2"),
            ("newer", "home"),
            ("newer", "c2"),
        ] {
            assert!(
                positions[a].distance(positions[b]) >= MIN_SPACING,
                "{a} {b}"
            );
        }

        // A second system off the same neighbour does not land on the first.
        let systems = vec![placed("home", 0.0, 0.0), system("x"), system("y")];
        let connections = vec![link("home", "x"), link("home", "y")];
        let positions = layout(&systems, &connections);
        assert!(positions["x"].distance(positions["y"]) >= MIN_SPACING);
    }

    #[test]
    fn a_group_with_nothing_placed_is_arranged_under_the_rest() {
        let systems = vec![
            placed("home", 0.0, 0.0),
            system("lone"),
            system("pair_a"),
            system("pair_b"),
        ];
        let connections = vec![link("pair_a", "pair_b")];
        let positions = layout(&systems, &connections);
        assert_eq!(positions.len(), 4);
        for id in ["lone", "pair_a", "pair_b"] {
            assert!(positions[id].y > positions["home"].y + NODE_RADIUS, "{id}");
        }
        assert!(positions["pair_a"].distance(positions["pair_b"]) >= MIN_SPACING);

        // A position that is not a real number counts as not placed.
        let systems = vec![placed("odd", f32::NAN, 0.0), system("b")];
        let positions = layout(&systems, &[]);
        assert!(
            positions
                .values()
                .all(|pos| pos.x.is_finite() && pos.y.is_finite())
        );
    }
}
