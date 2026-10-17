//! How the area under the chain map is shared: the dashboard tabs on the left, Umbra
//! on the right, with a divider the player can drag between them.

use crate::model::DashboardLayout;
use eframe::egui::{Pos2, Rect, pos2};

/// Neither side is squeezed narrower than this; below twice this the two stack.
pub(super) const MIN_PANE_WIDTH: f32 = 260.0;
/// The divider's grab area between the two sides.
pub(super) const DIVIDER_WIDTH: f32 = 10.0;
/// The gap between the tabs and Umbra when they are stacked.
pub(super) const STACK_GAP: f32 = 8.0;
/// The tabs' share of the height when stacked; Umbra has the rest.
const STACKED_TABS_SHARE: f32 = 0.4;

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

/// The grip's dots, centred in the divider.
pub(super) fn grip_dots(divider: Rect) -> [Pos2; 3] {
    let center = divider.center();
    [-6.0, 0.0, 6.0].map(|offset| pos2(center.x, center.y + offset))
}

#[cfg(test)]
mod tests {
    use super::{
        DIVIDER_WIDTH, MIN_PANE_WIDTH, STACK_GAP, clamp_tabs_fraction, dashboard_panes,
        fraction_at, grip_dots, stacks,
    };
    use crate::model::DashboardLayout;
    use eframe::egui::{Rect, pos2};

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
    }
}
