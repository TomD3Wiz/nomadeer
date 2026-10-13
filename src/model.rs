use serde::{Deserialize, Serialize};

pub(crate) const SYSTEM_CLASSES: &[&str] = &[
    "C1", "C2", "C3", "C4", "C5", "C6", "Thera", "Drifter", "HS", "LS", "NS", "Pochven",
];
pub(crate) const SYSTEM_STATUSES: &[&str] = &[
    "scouted", "home", "hostile", "farmable", "rolled", "unknown",
];
pub(crate) const SIG_TYPES: &[&str] = &[
    "wormhole", "relic", "data", "gas", "combat", "ore", "unknown",
];
pub(crate) const LIFE_STATES: &[&str] = &["fresh", "eol", "unknown"];
pub(crate) const MASS_STATES: &[&str] = &["fresh", "destab", "critical", "unknown"];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AppData {
    pub(crate) accounts: Vec<Account>,
    pub(crate) systems: Vec<SystemNode>,
    pub(crate) connections: Vec<Connection>,
    pub(crate) signatures: Vec<Signature>,
    pub(crate) visits: Vec<VisitLog>,
    pub(crate) notes: Vec<Note>,
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
