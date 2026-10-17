// Release builds on Windows are GUI apps, so no console window opens beside them.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod chain_archive;
mod config;
mod esi;
mod forms;
mod local_intel;
mod model;
mod paths;
mod portrait;
mod store;
mod util;
mod widgets;
mod zkill;

use app::NomadeerApp;
use eframe::egui;

/// Matches the desktop entry and AppStream metainfo under packaging/linux, so
/// desktops group the window with its launcher.
const APP_ID: &str = "io.github.TomD3Wiz.Nomadeer";

const ICON_PNG: &[u8] = include_bytes!("../assets/icons/nomadeer-256.png");

fn main() -> eframe::Result<()> {
    ensure_cpu_supports_build();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Nomadeer")
        .with_app_id(APP_ID)
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([960.0, 640.0]);
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Nomadeer",
        options,
        Box::new(|cc| Ok(Box::new(NomadeerApp::new(cc)))),
    )
}

fn app_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

/// Shown when the optimized build is started on a CPU it cannot run on.
#[cfg(any(all(target_arch = "x86_64", target_feature = "avx2"), test))]
const CPU_UNSUPPORTED_MESSAGE: &str = "This optimized Nomadeer build needs a CPU with AVX2 and FMA. \
     Please use the compatible build instead.";

/// The optimized release is compiled for AVX2/FMA CPUs. Exit with a clear message
/// on older CPUs instead of crashing on the first unsupported instruction.
#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
fn ensure_cpu_supports_build() {
    if !std::arch::is_x86_feature_detected!("avx2") || !std::arch::is_x86_feature_detected!("fma") {
        eprintln!("{CPU_UNSUPPORTED_MESSAGE}");
        // Windows release builds have no console, so the player would see nothing.
        // The text is encoded at compile time to run as little AVX-built code as
        // possible on a CPU without it.
        #[cfg(windows)]
        {
            const TITLE: [u16; 9] = windows_dialog::wide("Nomadeer");
            const TEXT: [u16; CPU_UNSUPPORTED_MESSAGE.len() + 1] =
                windows_dialog::wide(CPU_UNSUPPORTED_MESSAGE);
            windows_dialog::show_error(&TITLE, &TEXT);
        }
        std::process::exit(1);
    }
}

/// A native error dialog for problems that stop Nomadeer before its window opens.
#[cfg(any(windows, test))]
#[allow(dead_code)]
mod windows_dialog {
    /// NUL-terminated UTF-16 for an ASCII `text`; `N` must be its length plus one.
    pub(crate) const fn wide<const N: usize>(text: &str) -> [u16; N] {
        let bytes = text.as_bytes();
        assert!(bytes.len() + 1 == N, "N must leave room for the NUL");
        let mut wide = [0_u16; N];
        let mut index = 0;
        while index < bytes.len() {
            assert!(bytes[index].is_ascii(), "only ASCII text is supported");
            wide[index] = bytes[index] as u16;
            index += 1;
        }
        wide
    }

    #[cfg(windows)]
    pub(crate) fn show_error(title: &[u16], text: &[u16]) {
        use std::ffi::c_void;

        const MB_OK: u32 = 0x0000_0000;
        const MB_ICONERROR: u32 = 0x0000_0010;

        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBoxW(
                hwnd: *mut c_void,
                text: *const u16,
                caption: *const u16,
                kind: u32,
            ) -> i32;
        }

        assert!(title.last() == Some(&0) && text.last() == Some(&0));
        // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call, and a
        // null owner window is allowed.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

#[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
fn ensure_cpu_supports_build() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_decodes_embedded_png() {
        let icon = app_icon().expect("embedded icon should decode");
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }

    #[test]
    fn dialog_text_is_nul_terminated_utf16() {
        const TEXT: [u16; CPU_UNSUPPORTED_MESSAGE.len() + 1] =
            windows_dialog::wide(CPU_UNSUPPORTED_MESSAGE);
        let expected = CPU_UNSUPPORTED_MESSAGE
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        assert_eq!(TEXT.as_slice(), expected.as_slice());
        assert_eq!(windows_dialog::wide::<9>("Nomadeer").last(), Some(&0));
    }

    #[test]
    fn app_id_matches_linux_packaging() {
        let desktop = include_str!("../packaging/linux/io.github.TomD3Wiz.Nomadeer.desktop");
        // Compare whole lines so a CRLF checkout on Windows still matches.
        let has_line = |line: String| desktop.lines().any(|candidate| candidate == line);
        assert!(has_line(format!("StartupWMClass={APP_ID}")));
        assert!(has_line(format!("Icon={APP_ID}")));

        let metainfo = include_str!("../packaging/linux/io.github.TomD3Wiz.Nomadeer.metainfo.xml");
        assert!(metainfo.contains(&format!("<id>{APP_ID}</id>")));
        assert!(metainfo.contains(&format!(
            "<launchable type=\"desktop-id\">{APP_ID}.desktop</launchable>"
        )));
    }
}
