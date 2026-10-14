use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const USER_AGENT: &str = "Nomadeer/0.1 private wormhole mapper";

#[derive(Debug, Clone)]
pub(crate) struct ZkillReport {
    pub(crate) solar_system_id: u64,
    pub(crate) recent_kills: Vec<ZkillKill>,
    pub(crate) total_value: f64,
    pub(crate) solo_kills: usize,
    pub(crate) npc_kills: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ZkillKill {
    pub(crate) killmail_id: u64,
    pub(crate) killmail_time: String,
    pub(crate) victim_ship_type_id: Option<u64>,
    pub(crate) victim_ship_type_name: Option<String>,
    pub(crate) victim_character_name: Option<String>,
    pub(crate) victim_corporation_name: Option<String>,
    pub(crate) victim_alliance_name: Option<String>,
    pub(crate) attacker_count: usize,
    pub(crate) total_value: f64,
    pub(crate) solo: bool,
    pub(crate) npc: bool,
}

#[derive(Debug, Deserialize)]
struct ApiKillmail {
    killmail_id: u64,
    #[serde(default)]
    killmail_time: String,
    #[serde(default)]
    victim: Option<ApiVictim>,
    #[serde(default)]
    attackers: Vec<ApiAttacker>,
    #[serde(default)]
    zkb: Option<ApiZkb>,
}

#[derive(Debug, Deserialize)]
struct ApiVictim {
    character_id: Option<u64>,
    corporation_id: Option<u64>,
    alliance_id: Option<u64>,
    ship_type_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ApiAttacker {}

#[derive(Debug, Deserialize)]
struct ApiZkb {
    #[serde(default, rename = "totalValue")]
    total_value: f64,
    #[serde(default)]
    solo: bool,
    #[serde(default)]
    npc: bool,
}

#[derive(Debug, Deserialize)]
struct ApiName {
    id: u64,
    name: String,
}

pub(crate) fn fetch_system_report(solar_system_id: u64) -> Result<ZkillReport, String> {
    let url = format!("https://zkillboard.com/api/kills/solarSystemID/{solar_system_id}/");
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("Could not start zKill request: {error}"))?;
    let response = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .map_err(|error| format!("Could not reach zKillboard: {error}"))?;

    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("Could not read zKillboard response: {error}"))?;

    if !status.is_success() {
        return Err(format!("zKillboard returned {status}: {body}"));
    }

    let json = serde_json::from_str::<Value>(&body)
        .map_err(|error| format!("Could not parse zKillboard response: {error}"))?;
    if let Some(message) = json.get("error").and_then(Value::as_str) {
        return Err(format!("zKillboard says: {message}"));
    }

    let kills = serde_json::from_value::<Vec<ApiKillmail>>(json)
        .map_err(|error| format!("Could not read zKillboard kills: {error}"))?;

    let kills = kills.into_iter().take(10).collect::<Vec<_>>();
    let names = fetch_eve_names(&client, zkill_name_ids(&kills)).unwrap_or_default();

    let recent_kills = kills
        .into_iter()
        .map(|kill| {
            let zkb = kill.zkb.unwrap_or(ApiZkb {
                total_value: 0.0,
                solo: false,
                npc: false,
            });
            let victim = kill.victim;
            let victim_ship_type_id = victim.as_ref().and_then(|victim| victim.ship_type_id);
            let victim_ship_type_name = victim_ship_type_id.and_then(|id| names.get(&id).cloned());
            let victim_character_name = victim
                .as_ref()
                .and_then(|victim| victim.character_id)
                .and_then(|id| names.get(&id).cloned());
            let victim_corporation_name = victim
                .as_ref()
                .and_then(|victim| victim.corporation_id)
                .and_then(|id| names.get(&id).cloned());
            let victim_alliance_name = victim
                .as_ref()
                .and_then(|victim| victim.alliance_id)
                .and_then(|id| names.get(&id).cloned());

            ZkillKill {
                killmail_id: kill.killmail_id,
                killmail_time: kill.killmail_time,
                victim_ship_type_id,
                victim_ship_type_name,
                victim_character_name,
                victim_corporation_name,
                victim_alliance_name,
                attacker_count: kill.attackers.len(),
                total_value: zkb.total_value,
                solo: zkb.solo,
                npc: zkb.npc,
            }
        })
        .collect::<Vec<_>>();

    let total_value = recent_kills.iter().map(|kill| kill.total_value).sum();
    let solo_kills = recent_kills.iter().filter(|kill| kill.solo).count();
    let npc_kills = recent_kills.iter().filter(|kill| kill.npc).count();

    Ok(ZkillReport {
        solar_system_id,
        recent_kills,
        total_value,
        solo_kills,
        npc_kills,
    })
}

fn zkill_name_ids(kills: &[ApiKillmail]) -> Vec<u64> {
    let mut ids = HashSet::new();
    for kill in kills {
        let Some(victim) = &kill.victim else {
            continue;
        };
        if let Some(id) = victim.character_id {
            ids.insert(id);
        }
        if let Some(id) = victim.corporation_id {
            ids.insert(id);
        }
        if let Some(id) = victim.alliance_id {
            ids.insert(id);
        }
        if let Some(id) = victim.ship_type_id {
            ids.insert(id);
        }
    }

    ids.into_iter().collect()
}

fn fetch_eve_names(client: &Client, ids: Vec<u64>) -> Result<HashMap<u64, String>, String> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let response = client
        .post("https://esi.evetech.net/latest/universe/names/?datasource=tranquility")
        .header("User-Agent", USER_AGENT)
        .json(&ids)
        .send()
        .map_err(|error| format!("Could not resolve EVE names: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("Could not read EVE name response: {error}"))?;

    if !status.is_success() {
        return Err(format!("EVE names returned {status}: {body}"));
    }

    let names = serde_json::from_str::<Vec<ApiName>>(&body)
        .map_err(|error| format!("Could not parse EVE names: {error}"))?;
    Ok(names
        .into_iter()
        .map(|entry| (entry.id, entry.name))
        .collect())
}
