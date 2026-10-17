use eframe::egui::Color32;

pub(crate) mod theme {
    use super::Color32;

    pub(crate) fn accent() -> Color32 {
        Color32::from_rgb(93, 242, 255)
    }

    pub(crate) fn character_text() -> Color32 {
        Color32::from_rgb(255, 218, 121)
    }

    pub(crate) fn system_text() -> Color32 {
        Color32::from_rgb(126, 231, 135)
    }

    /// A character who is logged in.
    pub(crate) fn online() -> Color32 {
        Color32::from_rgb(110, 224, 160)
    }

    pub(crate) fn muted_text() -> Color32 {
        Color32::from_rgb(150, 180, 188)
    }

    pub(crate) fn panel_background() -> Color32 {
        Color32::from_rgb(6, 17, 22)
    }

    pub(crate) fn panel_stroke() -> Color32 {
        Color32::from_rgb(34, 78, 90)
    }
}
