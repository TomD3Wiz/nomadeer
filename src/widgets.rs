use crate::config::theme;
use crate::forms::PanelTab;
use crate::model::SystemNode;
use eframe::egui;

pub(crate) fn combo(ui: &mut egui::Ui, label: &str, value: &mut String, options: &[&str]) {
    egui::ComboBox::from_label(label)
        .selected_text(if value.is_empty() {
            "Select"
        } else {
            value.as_str()
        })
        .show_ui(ui, |ui| {
            for option in options {
                ui.selectable_value(value, (*option).to_owned(), *option);
            }
        });
}

pub(crate) fn system_combo(
    ui: &mut egui::Ui,
    label: &str,
    systems: &[SystemNode],
    selected: &mut String,
) {
    egui::ComboBox::from_label(label)
        .selected_text(
            systems
                .iter()
                .find(|system| system.id == *selected)
                .map(|system| system.name.as_str())
                .unwrap_or("Select system"),
        )
        .show_ui(ui, |ui| {
            for system in systems {
                ui.selectable_value(
                    selected,
                    system.id.clone(),
                    format!("{} ({})", system.name, system.class),
                );
            }
        });
}

pub(crate) fn tab_button(ui: &mut egui::Ui, active: &mut PanelTab, tab: PanelTab, label: &str) {
    if ui.selectable_label(*active == tab, label).clicked() {
        *active = tab;
    }
}

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
