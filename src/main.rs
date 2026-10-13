mod app;
mod esi;
mod forms;
mod model;
mod store;
mod util;
mod widgets;

use app::NomadeerApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
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
