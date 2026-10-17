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
    #[serde(default)]
    pub(crate) activity_snapshots: Vec<SystemActivitySnapshot>,
    #[serde(default)]
    pub(crate) cached_killmails: Vec<CachedKillmail>,
    #[serde(default)]
    pub(crate) system_assessments: Vec<SystemAssessment>,
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
    #[serde(default)]
    pub(crate) solar_system_id: u64,
    pub(crate) name: String,
    pub(crate) class: String,
    #[serde(default)]
    pub(crate) security_status: f32,
    #[serde(default)]
    pub(crate) constellation_id: u64,
    #[serde(default)]
    pub(crate) constellation_name: String,
    #[serde(default)]
    pub(crate) region_id: u64,
    #[serde(default)]
    pub(crate) region_name: String,
    #[serde(default)]
    pub(crate) planet_count: u32,
    #[serde(default)]
    pub(crate) stargate_count: u32,
    #[serde(default)]
    pub(crate) station_count: u32,
    #[serde(default)]
    pub(crate) jumps_1h: u32,
    #[serde(default)]
    pub(crate) ship_kills_1h: u32,
    #[serde(default)]
    pub(crate) pod_kills_1h: u32,
    #[serde(default)]
    pub(crate) npc_kills_1h: u32,
    #[serde(default)]
    pub(crate) intel_updated_at: u64,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SystemActivitySnapshot {
    pub(crate) system_id: String,
    pub(crate) captured_at: u64,
    pub(crate) jumps_1h: u32,
    pub(crate) ship_kills_1h: u32,
    pub(crate) pod_kills_1h: u32,
    pub(crate) npc_kills_1h: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedKillmail {
    pub(crate) killmail_id: u64,
    pub(crate) solar_system_id: u64,
    pub(crate) killmail_time: u64,
    pub(crate) total_value: f64,
    pub(crate) solo: bool,
    pub(crate) npc: bool,
    pub(crate) attacker_ids: Vec<u64>,
    pub(crate) corporation_ids: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SystemAssessment {
    pub(crate) system_id: String,
    pub(crate) threat_score: u8,
    pub(crate) activity_score: u8,
    pub(crate) confidence: String,
    pub(crate) evidence: Vec<String>,
    pub(crate) briefing: String,
    pub(crate) model_generated: bool,
    pub(crate) generated_at: u64,
}
