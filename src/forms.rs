#[derive(Default)]
pub(crate) struct EsiSettingsForm {
    pub(crate) client_id: String,
    pub(crate) callback_port: String,
    pub(crate) scopes: String,
}

#[derive(Default)]
pub(crate) struct SystemForm {
    pub(crate) name: String,
    pub(crate) class: String,
    pub(crate) effect: String,
    pub(crate) status: String,
    pub(crate) owner: String,
    pub(crate) notes: String,
}

#[derive(Default)]
pub(crate) struct ConnectionForm {
    pub(crate) from_system: String,
    pub(crate) to_system: String,
    pub(crate) signature_from: String,
    pub(crate) signature_to: String,
    pub(crate) mass_status: String,
    pub(crate) life_status: String,
    pub(crate) ship_size: String,
    pub(crate) notes: String,
}

#[derive(Default)]
pub(crate) struct SignatureForm {
    pub(crate) system_id: String,
    pub(crate) code: String,
    pub(crate) sig_type: String,
    pub(crate) target: String,
    pub(crate) life: String,
    pub(crate) status: String,
    pub(crate) notes: String,
}

#[derive(Default)]
pub(crate) struct VisitForm {
    pub(crate) system_id: String,
    pub(crate) pilot: String,
    pub(crate) activity: String,
    pub(crate) notes: String,
}

#[derive(Default)]
pub(crate) struct NoteForm {
    pub(crate) system_id: String,
    pub(crate) title: String,
    pub(crate) body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelTab {
    Accounts,
    Systems,
    Connections,
    Signatures,
    Visits,
    Notes,
}
