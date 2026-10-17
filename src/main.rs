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

fn main() -> eframe::Result<()> {
    ensure_cpu_supports_build();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Nomadeer")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([960.0, 640.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Nomadeer",
        options,
        Box::new(|cc| Ok(Box::new(NomadeerApp::new(cc)))),
    )
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
