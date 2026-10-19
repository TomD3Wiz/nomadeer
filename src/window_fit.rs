//! Fitting Nomadeer's window to the screen it opens on.
//!
//! The window opens at a comfortable default size. On a screen too small for it
//! (a 1280x800 or 1366x768 laptop, or a larger one at a high scaling setting) it
//! would open partly off-screen, so it starts maximized there instead, and its
//! minimum size never asks for more than the screen can show.

use eframe::egui::Vec2;

/// The window's size on a screen with room for it, in points.
pub(crate) const DEFAULT_WINDOW_SIZE: Vec2 = Vec2::new(1280.0, 820.0);
/// The smallest the window can be made where the screen allows it.
pub(crate) const MIN_WINDOW_SIZE: Vec2 = Vec2::new(960.0, 640.0);
/// Room the desktop keeps around a window: its title bar and borders, and a
/// taskbar or panel along one edge.
const DESKTOP_CHROME: Vec2 = Vec2::new(16.0, 96.0);
/// The minimum size never drops below this, however small the screen says it is.
const SMALLEST_MIN_SIZE: Vec2 = Vec2::new(640.0, 400.0);

/// How the window should open on a monitor of the given size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WindowFit {
    /// The default size does not fit, so the window fills the screen instead.
    pub(crate) maximize: bool,
    pub(crate) min_size: Vec2,
}

/// The fit for a monitor of `monitor` points.
pub(crate) fn fit_to_monitor(monitor: Vec2) -> WindowFit {
    let usable = monitor - DESKTOP_CHROME;
    WindowFit {
        maximize: DEFAULT_WINDOW_SIZE.x > usable.x || DEFAULT_WINDOW_SIZE.y > usable.y,
        min_size: MIN_WINDOW_SIZE.min(usable).max(SMALLEST_MIN_SIZE),
    }
}

/// Whether a reported monitor size is usable; some platforms report zero before
/// the window is placed.
pub(crate) fn is_known_monitor(monitor: Vec2) -> bool {
    monitor.x >= 1.0 && monitor.y >= 1.0
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_WINDOW_SIZE, MIN_WINDOW_SIZE, SMALLEST_MIN_SIZE, WindowFit, fit_to_monitor,
        is_known_monitor,
    };
    use eframe::egui::Vec2;

    fn fit(width: f32, height: f32) -> WindowFit {
        fit_to_monitor(Vec2::new(width, height))
    }

    #[test]
    fn small_screens_open_maximized() {
        // The player's 1280x800 laptop: the default 1280x820 window would hang off it.
        assert!(fit(1280.0, 800.0).maximize);
        assert!(fit(1366.0, 768.0).maximize);
        assert!(fit(1440.0, 900.0).maximize);
        // 1920x1080 at 150% scaling is 1280x720 points.
        assert!(fit(1280.0, 720.0).maximize);
        // At 125% it is 1536x864 points, short of room for the window's 820
        // points plus a title bar and taskbar.
        assert!(fit(1536.0, 864.0).maximize);
    }

    #[test]
    fn roomy_screens_keep_the_default_size() {
        assert!(!fit(1920.0, 1080.0).maximize);
        assert!(!fit(2560.0, 1440.0).maximize);
        assert!(!fit(1680.0, 1050.0).maximize);
        assert!(!fit(DEFAULT_WINDOW_SIZE.x + 16.0, DEFAULT_WINDOW_SIZE.y + 96.0).maximize);
    }

    #[test]
    fn the_minimum_size_fits_common_small_screens() {
        // 1366x768 with a taskbar still allows the usual minimum.
        assert_eq!(fit(1366.0, 768.0).min_size, MIN_WINDOW_SIZE);
        assert_eq!(fit(1280.0, 800.0).min_size, MIN_WINDOW_SIZE);
        assert_eq!(fit(1920.0, 1080.0).min_size, MIN_WINDOW_SIZE);
        // 1366x768 at 125% scaling is about 1093x614 points: the minimum shrinks
        // so the whole window can stay on screen.
        let scaled = fit(1366.0 / 1.25, 768.0 / 1.25);
        assert!(scaled.maximize);
        assert_eq!(scaled.min_size.x, MIN_WINDOW_SIZE.x);
        assert!(scaled.min_size.y <= 768.0 / 1.25 - 96.0);
        // Never shrunk to nothing by a nonsense report.
        assert_eq!(fit(200.0, 100.0).min_size, SMALLEST_MIN_SIZE);
    }

    #[test]
    fn unknown_monitors_are_ignored() {
        assert!(!is_known_monitor(Vec2::ZERO));
        assert!(!is_known_monitor(Vec2::new(1920.0, 0.0)));
        assert!(is_known_monitor(Vec2::new(1280.0, 800.0)));
    }
}
