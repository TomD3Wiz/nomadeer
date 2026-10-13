use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_ESI_SCOPES: &str = "publicData esi-location.read_location.v1 esi-location.read_ship_type.v1 esi-search.search_structures.v1 esi-universe.read_structures.v1 esi-ui.open_window.v1 esi-ui.write_waypoint.v1 esi-location.read_online.v1 esi-planets.read_customs_offices.v1 esi-structures.read_corporation.v1 esi-structures.read_character.v1 esi-activities.read_character.v1 esi.activity.char:read esi.cosmetic.char:read";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AppData {
    #[serde(default)]
    pub(crate) esi_settings: EsiSettings,
    #[serde(default)]
    pub(crate) accounts: Vec<Account>,
    #[serde(default)]
    pub(crate) systems: Vec<SystemNode>,
    #[serde(default)]
    pub(crate) connections: Vec<Connection>,
    #[serde(default)]
    pub(crate) signatures: Vec<Signature>,
    #[serde(default)]
    pub(crate) visits: Vec<VisitLog>,
    #[serde(default)]
    pub(crate) notes: Vec<Note>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EsiSettings {
    pub(crate) client_id: String,
    pub(crate) callback_port: u16,
    pub(crate) scopes: String,
}

impl Default for EsiSettings {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            callback_port: 8788,
            scopes: DEFAULT_ESI_SCOPES.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Account {
    pub(crate) id: String,
    pub(crate) character_id: String,
    pub(crate) character_name: String,
    pub(crate) owner_hash: String,
    pub(crate) auth_status: String,
    pub(crate) last_system_id: String,
    pub(crate) last_system_name: String,
    pub(crate) access_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) token_expires_at: Option<u64>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SystemNode {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) class: String,
    pub(crate) effect: String,
    pub(crate) status: String,
    pub(crate) owner: String,
    pub(crate) notes: String,
    pub(crate) created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Connection {
    pub(crate) id: String,
    pub(crate) from_system: String,
    pub(crate) to_system: String,
    pub(crate) signature_from: String,
    pub(crate) signature_to: String,
    pub(crate) mass_status: String,
    pub(crate) life_status: String,
    pub(crate) ship_size: String,
    pub(crate) notes: String,
    pub(crate) created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Signature {
    pub(crate) id: String,
    pub(crate) system_id: String,
    pub(crate) code: String,
    pub(crate) sig_type: String,
    pub(crate) target: String,
    pub(crate) life: String,
    pub(crate) status: String,
    pub(crate) notes: String,
    pub(crate) created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VisitLog {
    pub(crate) id: String,
    pub(crate) system_id: String,
    pub(crate) pilot: String,
    pub(crate) activity: String,
    pub(crate) notes: String,
    pub(crate) created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Note {
    pub(crate) id: String,
    pub(crate) system_id: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) created_at: u64,
}
