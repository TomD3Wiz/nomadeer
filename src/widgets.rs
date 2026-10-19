use eframe::egui;

use crate::config::theme;

pub(crate) fn record_frame(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .inner_margin(10.0)
        .show(ui, add_contents);
    ui.add_space(6.0);
}

pub(crate) fn pill(ui: &mut egui::Ui, text: &str) {
    if !text.trim().is_empty() {
        ui.label(egui::RichText::new(text).color(theme::accent()));
    }
}
