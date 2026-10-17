use serde::{Deserialize, Serialize};

/// Nomadeer's registered EVE application. It uses PKCE, so there is no secret.
pub(crate) const DEFAULT_ESI_CLIENT_ID: &str = "93a6ff589aad4d09b2ed0003e742509b";
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
    pub(crate) system_assessments: Vec<SystemAssessment>,
    #[serde(default)]
    pub(crate) auto_intel: AutoIntelSettings,
    /// The first character has been made a recon pilot once; after that the player
    /// decides, even if they unmark everyone.
    #[serde(default)]
    pub(crate) recon_defaults_applied: bool,
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

/// What Nomadeer does on its own as characters move. Both are on unless turned off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AutoIntelSettings {
    /// Refresh killboard intel when a character arrives in a system without fresh intel.
    pub(crate) gather_on_arrival: bool,
    /// Have Umbra brief each fresh assessment without being asked.
    pub(crate) umbra_briefs_automatically: bool,
}

impl Default for AutoIntelSettings {
    fn default() -> Self {
        Self {
            gather_on_arrival: true,
            umbra_briefs_automatically: true,
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
    /// Recon pilots' arrivals gather intel and get Umbra's briefings.
    #[serde(default)]
    pub(crate) recon: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct KillCounts {
    pub(crate) ship_kills: u32,
    pub(crate) pod_kills: u32,
    pub(crate) npc_kills: u32,
}

/// The two universe-wide ESI activity feeds. Each publishes its own hourly data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrafficFeed {
    Jumps,
    Kills,
}

impl TrafficFeed {
    pub(crate) const ALL: [Self; 2] = [Self::Jumps, Self::Kills];
}

/// One ESI data hour of a universe-wide feed. Systems missing from `rows` had no activity.
#[derive(Debug, Clone)]
pub(crate) struct TrafficHour {
    /// Start of the UTC hour ESI published the data in.
    pub(crate) hour: u64,
    /// ESI's Last-Modified time for the data.
    pub(crate) data_time: u64,
    pub(crate) rows: TrafficRows,
}

#[derive(Debug, Clone)]
pub(crate) enum TrafficRows {
    Jumps(Vec<(u64, u32)>),
    Kills(Vec<(u64, KillCounts)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrafficReading<T> {
    pub(crate) hour: u64,
    pub(crate) data_time: u64,
    pub(crate) value: T,
}

/// A system's activity for one captured ESI hour. A feed that was not captured that
/// hour is `None`; a captured feed that did not list the system is zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrafficSample {
    pub(crate) hour: u64,
    pub(crate) jumps: Option<u32>,
    pub(crate) kills: Option<KillCounts>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SystemTrafficHistory {
    pub(crate) latest_jumps: Option<TrafficReading<u32>>,
    pub(crate) latest_kills: Option<TrafficReading<KillCounts>>,
    /// Captured hours in ascending order.
    pub(crate) samples: Vec<TrafficSample>,
}

#[derive(Debug, Clone, PartialEq)]
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

/// The span, `since` to `until` in Unix seconds, over which a system's cached
/// killmails hold every kill zKillboard listed when it was last fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KillboardCoverage {
    pub(crate) solar_system_id: u64,
    pub(crate) since: u64,
    pub(crate) until: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
