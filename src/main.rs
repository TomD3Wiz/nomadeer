mod app;
mod chain_archive;
mod config;
mod esi;
mod forms;
mod local_intel;
mod model;
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

/// The optimized release is compiled for AVX2/FMA CPUs. Exit with a clear message
/// on older CPUs instead of crashing on the first unsupported instruction.
#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
fn ensure_cpu_supports_build() {
    if !std::arch::is_x86_feature_detected!("avx2") || !std::arch::is_x86_feature_detected!("fma") {
        eprintln!(
            "This optimized Nomadeer build needs a CPU with AVX2 and FMA. \
             Please use the compatible build instead."
        );
        std::process::exit(1);
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
    fn app_id_matches_linux_packaging() {
        let desktop = include_str!("../packaging/linux/io.github.TomD3Wiz.Nomadeer.desktop");
        assert!(desktop.contains(&format!("StartupWMClass={APP_ID}\n")));
        assert!(desktop.contains(&format!("Icon={APP_ID}\n")));

        let metainfo = include_str!("../packaging/linux/io.github.TomD3Wiz.Nomadeer.metainfo.xml");
        assert!(metainfo.contains(&format!("<id>{APP_ID}</id>")));
        assert!(metainfo.contains(&format!(
            "<launchable type=\"desktop-id\">{APP_ID}.desktop</launchable>"
        )));
    }
}
