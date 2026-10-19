//! How the dashboard is shared: the chain map above, and under it the dashboard tabs
//! on the left and Umbra on the right, with dividers the player can drag between them.

use eframe::egui::{Pos2, Rect, pos2};

use crate::model::DashboardLayout;

/// Neither side is squeezed narrower than this; below twice this the two stack.
pub(super) const MIN_PANE_WIDTH: f32 = 260.0;
/// The divider's grab area between the two sides.
pub(super) const DIVIDER_WIDTH: f32 = 10.0;
/// The gap between the tabs and Umbra when they are stacked.
pub(super) const STACK_GAP: f32 = 8.0;
/// The tabs' share of the height when stacked; Umbra has the rest.
const STACKED_TABS_SHARE: f32 = 0.4;
/// Neither the chain map nor the area under it is squeezed shorter than these while
/// there is room for both; below that they shrink together.
pub(super) const MIN_MAP_HEIGHT: f32 = 200.0;
pub(super) const MIN_BELOW_MAP_HEIGHT: f32 = 180.0;

/// Where the tabs, Umbra and the divider sit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct DashboardPanes {
    pub(super) tabs: Rect,
    pub(super) umbra: Rect,
    /// None when the window is too narrow and the two are stacked.
    pub(super) divider: Option<Rect>,
}

/// Whether the area is too narrow for both sides side by side.
pub(super) fn stacks(width: f32) -> bool {
    width < 2.0 * MIN_PANE_WIDTH + DIVIDER_WIDTH
}

/// The tabs' share of `width`, kept so that both sides get at least their minimum.
/// A share that makes no sense falls back to the default.
pub(super) fn clamp_tabs_fraction(fraction: f32, width: f32) -> f32 {
    let fraction = if DashboardLayout::is_valid_fraction(fraction) {
        fraction
    } else {
        DashboardLayout::DEFAULT_TABS_FRACTION
    };
    let shared = width - DIVIDER_WIDTH;
    if shared < 2.0 * MIN_PANE_WIDTH {
        return fraction;
    }
    let min = MIN_PANE_WIDTH / shared;
    fraction.clamp(min, 1.0 - min)
}

/// The tabs' share that puts the middle of the divider under the pointer.
pub(super) fn fraction_at(pointer_x: f32, area: Rect) -> f32 {
    let shared = area.width() - DIVIDER_WIDTH;
    if shared <= 0.0 {
        return DashboardLayout::DEFAULT_TABS_FRACTION;
    }
    let fraction = (pointer_x - area.left() - DIVIDER_WIDTH / 2.0) / shared;
    clamp_tabs_fraction(fraction.clamp(0.01, 0.99), area.width())
}

/// Splits the area: side by side when wide enough, otherwise tabs above Umbra.
pub(super) fn dashboard_panes(area: Rect, tabs_fraction: f32) -> DashboardPanes {
    if stacks(area.width()) {
        let tabs_height = ((area.height() - STACK_GAP) * STACKED_TABS_SHARE).max(0.0);
        let tabs = Rect::from_min_max(area.min, pos2(area.right(), area.top() + tabs_height));
        let umbra = Rect::from_min_max(pos2(area.left(), tabs.bottom() + STACK_GAP), area.max);
        return DashboardPanes {
            tabs,
            umbra,
            divider: None,
        };
    }
    let fraction = clamp_tabs_fraction(tabs_fraction, area.width());
    let tabs_width = ((area.width() - DIVIDER_WIDTH) * fraction).round();
    let divider_left = area.left() + tabs_width;
    let divider_right = divider_left + DIVIDER_WIDTH;
    DashboardPanes {
        tabs: Rect::from_min_max(area.min, pos2(divider_left, area.bottom())),
        umbra: Rect::from_min_max(pos2(divider_right, area.top()), area.max),
        divider: Some(Rect::from_min_max(
            pos2(divider_left, area.top()),
            pos2(divider_right, area.bottom()),
        )),
    }
}

/// Where the chain map, the divider under it and the tabs and Umbra sit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct MapSplit {
    pub(super) map: Rect,
    pub(super) divider: Rect,
    pub(super) below: Rect,
}

/// The map's share of `height`, kept so that both it and the area under it get at
/// least their minimum. Too short for both, they share it in proportion to their
/// minimums. A share that makes no sense falls back to the default.
pub(super) fn clamp_map_fraction(fraction: f32, height: f32) -> f32 {
    let fraction = if DashboardLayout::is_valid_fraction(fraction) {
        fraction
    } else {
        DashboardLayout::DEFAULT_MAP_FRACTION
    };
    let shared = height - DIVIDER_WIDTH;
    let needed = MIN_MAP_HEIGHT + MIN_BELOW_MAP_HEIGHT;
    if shared < needed {
        return MIN_MAP_HEIGHT / needed;
    }
    fraction.clamp(MIN_MAP_HEIGHT / shared, 1.0 - MIN_BELOW_MAP_HEIGHT / shared)
}

/// The map's share that puts the middle of the divider under the pointer.
pub(super) fn map_fraction_at(pointer_y: f32, area: Rect) -> f32 {
    let shared = area.height() - DIVIDER_WIDTH;
    if shared <= 0.0 {
        return DashboardLayout::DEFAULT_MAP_FRACTION;
    }
    let fraction = (pointer_y - area.top() - DIVIDER_WIDTH / 2.0) / shared;
    clamp_map_fraction(fraction.clamp(0.01, 0.99), area.height())
}

/// Splits the area into the chain map, the divider and what goes under the map.
pub(super) fn map_split(area: Rect, map_fraction: f32) -> MapSplit {
    let fraction = clamp_map_fraction(map_fraction, area.height());
    let shared = (area.height() - DIVIDER_WIDTH).max(0.0);
    let divider_top = area.top() + (shared * fraction).round();
    let divider_bottom = (divider_top + DIVIDER_WIDTH).min(area.bottom());
    MapSplit {
        map: Rect::from_min_max(area.min, pos2(area.right(), divider_top)),
        divider: Rect::from_min_max(
            pos2(area.left(), divider_top),
            pos2(area.right(), divider_bottom),
        ),
        below: Rect::from_min_max(pos2(area.left(), divider_bottom), area.max),
    }
}

/// The grip's dots, centred in the divider and running along it.
pub(super) fn grip_dots(divider: Rect) -> [Pos2; 3] {
    let center = divider.center();
    if divider.height() >= divider.width() {
        [-6.0, 0.0, 6.0].map(|offset| pos2(center.x, center.y + offset))
    } else {
        [-6.0, 0.0, 6.0].map(|offset| pos2(center.x + offset, center.y))
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Rect, pos2};

    use super::{
        DIVIDER_WIDTH, MIN_BELOW_MAP_HEIGHT, MIN_MAP_HEIGHT, MIN_PANE_WIDTH, STACK_GAP,
        clamp_map_fraction, clamp_tabs_fraction, dashboard_panes, fraction_at, grip_dots,
        map_fraction_at, map_split, stacks,
    };
    use crate::model::DashboardLayout;

    const DEFAULT: f32 = DashboardLayout::DEFAULT_TABS_FRACTION;

    fn area(width: f32, height: f32) -> Rect {
        Rect::from_min_max(pos2(100.0, 400.0), pos2(100.0 + width, 400.0 + height))
    }

    #[test]
    fn the_two_stack_only_below_both_minimums() {
        let threshold = 2.0 * MIN_PANE_WIDTH + DIVIDER_WIDTH;
        assert!(stacks(threshold - 1.0));
        assert!(!stacks(threshold));
        assert!(!stacks(1200.0));
    }

    #[test]
    fn the_tabs_share_keeps_both_sides_at_their_minimum() {
        assert!((clamp_tabs_fraction(DEFAULT, 1200.0) - DEFAULT).abs() < 1e-6);
        let shared = 1200.0 - DIVIDER_WIDTH;
        let min = MIN_PANE_WIDTH / shared;
        assert!((clamp_tabs_fraction(0.05, 1200.0) - min).abs() < 1e-6);
        assert!((clamp_tabs_fraction(0.95, 1200.0) - (1.0 - min)).abs() < 1e-6);
        // Exactly wide enough leaves one place for the divider.
        let tight = 2.0 * MIN_PANE_WIDTH + DIVIDER_WIDTH;
        assert!((clamp_tabs_fraction(0.1, tight) - 0.5).abs() < 1e-6);
        assert!((clamp_tabs_fraction(0.9, tight) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_nonsense_share_falls_back_to_the_default() {
        for bad in [f32::NAN, f32::INFINITY, -0.5, 0.0, 1.0, 3.0] {
            assert!(
                (clamp_tabs_fraction(bad, 1200.0) - DEFAULT).abs() < 1e-6,
                "{bad}"
            );
        }
    }

    #[test]
    fn side_by_side_gives_the_tabs_a_third_and_umbra_the_rest() {
        let area = area(1210.0, 500.0);
        let panes = dashboard_panes(area, DEFAULT);
        let divider = panes.divider.expect("wide enough for a divider");
        assert_eq!(panes.tabs.width(), 400.0);
        assert_eq!(divider.width(), DIVIDER_WIDTH);
        assert_eq!(panes.umbra.width(), 800.0);
        assert_eq!(panes.tabs.left(), area.left());
        assert_eq!(panes.tabs.right(), divider.left());
        assert_eq!(divider.right(), panes.umbra.left());
        assert_eq!(panes.umbra.right(), area.right());
        for rect in [panes.tabs, divider, panes.umbra] {
            assert_eq!(rect.top(), area.top());
            assert_eq!(rect.bottom(), area.bottom());
        }
    }

    #[test]
    fn side_by_side_never_squeezes_a_side_below_its_minimum() {
        for fraction in [0.0001, 0.05, 0.5, 0.95, 0.9999] {
            for width in [530.0, 700.0, 1000.0, 2400.0] {
                let panes = dashboard_panes(area(width, 400.0), fraction);
                assert!(panes.divider.is_some());
                assert!(
                    panes.tabs.width() >= MIN_PANE_WIDTH - 0.5,
                    "{fraction} {width}"
                );
                assert!(
                    panes.umbra.width() >= MIN_PANE_WIDTH - 0.5,
                    "{fraction} {width}"
                );
            }
        }
    }

    #[test]
    fn narrow_areas_stack_the_tabs_above_umbra() {
        let area = area(400.0, 610.0);
        let panes = dashboard_panes(area, DEFAULT);
        assert_eq!(panes.divider, None);
        assert_eq!(panes.tabs.width(), 400.0);
        assert_eq!(panes.umbra.width(), 400.0);
        assert_eq!(panes.tabs.top(), area.top());
        assert_eq!(panes.umbra.top(), panes.tabs.bottom() + STACK_GAP);
        assert_eq!(panes.umbra.bottom(), area.bottom());
        assert!(panes.umbra.height() > panes.tabs.height());
        assert!(panes.tabs.height() > 0.0);
    }

    #[test]
    fn dragging_puts_the_divider_under_the_pointer_within_limits() {
        let area = area(1210.0, 500.0);
        let middle = area.left() + 400.0 + DIVIDER_WIDTH / 2.0;
        assert!((fraction_at(middle, area) - DEFAULT).abs() < 1e-6);
        let panes = dashboard_panes(area, fraction_at(middle + 200.0, area));
        assert!((panes.divider.unwrap().center().x - (middle + 200.0)).abs() < 1.0);
        // Past either edge the divider stops at the minimum.
        let far_left = dashboard_panes(area, fraction_at(area.left() - 500.0, area));
        assert!((far_left.tabs.width() - MIN_PANE_WIDTH).abs() < 1.0);
        let far_right = dashboard_panes(area, fraction_at(area.right() + 500.0, area));
        assert!((far_right.umbra.width() - MIN_PANE_WIDTH).abs() < 1.0);
    }

    #[test]
    fn the_grip_sits_in_the_middle_of_the_divider() {
        let divider = Rect::from_min_max(pos2(10.0, 0.0), pos2(20.0, 100.0));
        let dots = grip_dots(divider);
        assert!(dots.iter().all(|dot| dot.x == 15.0));
        assert_eq!(dots[1].y, 50.0);

        let under_map = Rect::from_min_max(pos2(0.0, 300.0), pos2(800.0, 310.0));
        let dots = grip_dots(under_map);
        assert!(dots.iter().all(|dot| dot.y == 305.0));
        assert_eq!(dots[1].x, 400.0);
        assert!(dots[0].x < dots[2].x);
    }

    #[test]
    fn the_map_split_stacks_map_divider_and_the_rest_without_gaps() {
        let area = area(900.0, 810.0);
        let split = map_split(area, 0.5);
        assert_eq!(split.map.top(), area.top());
        assert_eq!(split.map.height(), 400.0);
        assert_eq!(split.divider.top(), split.map.bottom());
        assert_eq!(split.divider.height(), DIVIDER_WIDTH);
        assert_eq!(split.below.top(), split.divider.bottom());
        assert_eq!(split.below.bottom(), area.bottom());
        assert_eq!(split.below.height(), 400.0);
        for rect in [split.map, split.divider, split.below] {
            assert_eq!(rect.left(), area.left());
            assert_eq!(rect.right(), area.right());
        }
    }

    #[test]
    fn the_map_and_the_area_under_it_keep_their_minimums() {
        for fraction in [0.0001, 0.1, 0.45, 0.9, 0.9999] {
            for height in [400.0, 600.0, 1000.0, 2000.0] {
                let split = map_split(area(900.0, height), fraction);
                assert!(
                    split.map.height() >= MIN_MAP_HEIGHT - 0.5,
                    "{fraction} {height}"
                );
                assert!(
                    split.below.height() >= MIN_BELOW_MAP_HEIGHT - 0.5,
                    "{fraction} {height}"
                );
            }
        }
        // Too short for both, they shrink together rather than one vanishing.
        let split = map_split(area(900.0, 200.0), 0.9);
        assert!(split.map.height() > 0.0 && split.below.height() > 0.0);
        let ratio = split.map.height() / split.below.height();
        assert!((ratio - MIN_MAP_HEIGHT / MIN_BELOW_MAP_HEIGHT).abs() < 0.05);
        // Nonsense shares fall back to the default.
        for bad in [f32::NAN, -1.0, 0.0, 1.0, 7.0] {
            assert!(
                (clamp_map_fraction(bad, 1000.0) - DashboardLayout::DEFAULT_MAP_FRACTION).abs()
                    < 1e-6,
                "{bad}"
            );
        }
    }

    #[test]
    fn dragging_puts_the_map_divider_under_the_pointer_within_limits() {
        let area = area(900.0, 810.0);
        let target = area.top() + 300.0;
        let split = map_split(area, map_fraction_at(target, area));
        assert!((split.divider.center().y - target).abs() < 1.0);
        let top = map_split(area, map_fraction_at(area.top() - 500.0, area));
        assert!((top.map.height() - MIN_MAP_HEIGHT).abs() < 1.0);
        let bottom = map_split(area, map_fraction_at(area.bottom() + 500.0, area));
        assert!((bottom.below.height() - MIN_BELOW_MAP_HEIGHT).abs() < 1.0);
    }
}
