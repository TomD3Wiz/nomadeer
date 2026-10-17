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
    pub(crate) victim_ship_type_name: Option<String>,
    pub(crate) victim_character_name: Option<String>,
    pub(crate) victim_corporation_name: Option<String>,
    pub(crate) victim_alliance_name: Option<String>,
    pub(crate) victim_damage_taken: u64,
    pub(crate) attacker_count: usize,
    pub(crate) attackers: Vec<ZkillAttacker>,
    pub(crate) items: Vec<ZkillItem>,
    pub(crate) fitted_value: f64,
    pub(crate) dropped_value: f64,
    pub(crate) destroyed_value: f64,
    pub(crate) total_value: f64,
    pub(crate) points: u64,
    pub(crate) solo: bool,
    pub(crate) npc: bool,
    pub(crate) awox: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ZkillAttacker {
    pub(crate) character_name: Option<String>,
    pub(crate) corporation_name: Option<String>,
    pub(crate) alliance_name: Option<String>,
    pub(crate) faction_name: Option<String>,
    pub(crate) ship_type_name: Option<String>,
    pub(crate) weapon_type_name: Option<String>,
    pub(crate) damage_done: u64,
    pub(crate) final_blow: bool,
    pub(crate) security_status: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct ZkillItem {
    pub(crate) type_name: String,
    pub(crate) quantity_destroyed: u64,
    pub(crate) quantity_dropped: u64,
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
    #[serde(default)]
    damage_taken: u64,
    #[serde(default)]
    items: Vec<ApiItem>,
}

#[derive(Debug, Deserialize)]
struct ApiAttacker {
    character_id: Option<u64>,
    corporation_id: Option<u64>,
    alliance_id: Option<u64>,
    faction_id: Option<u64>,
    ship_type_id: Option<u64>,
    weapon_type_id: Option<u64>,
    #[serde(default)]
    damage_done: u64,
    #[serde(default)]
    final_blow: bool,
    #[serde(default)]
    security_status: f32,
}

#[derive(Debug, Deserialize)]
struct ApiItem {
    item_type_id: u64,
    #[serde(default)]
    quantity_destroyed: u64,
    #[serde(default)]
    quantity_dropped: u64,
    #[serde(default)]
    items: Vec<ApiItem>,
}

#[derive(Debug, Deserialize)]
struct ApiZkb {
    #[serde(default, rename = "totalValue")]
    total_value: f64,
    #[serde(default, rename = "fittedValue")]
    fitted_value: f64,
    #[serde(default, rename = "droppedValue")]
    dropped_value: f64,
    #[serde(default, rename = "destroyedValue")]
    destroyed_value: f64,
    #[serde(default)]
    points: u64,
    #[serde(default)]
    solo: bool,
    #[serde(default)]
    npc: bool,
    #[serde(default)]
    awox: bool,
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
                fitted_value: 0.0,
                dropped_value: 0.0,
                destroyed_value: 0.0,
                points: 0,
                solo: false,
                npc: false,
                awox: false,
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
            let victim_damage_taken = victim.as_ref().map_or(0, |victim| victim.damage_taken);
            let items = victim
                .as_ref()
                .map(|victim| resolved_items(&victim.items, &names))
                .unwrap_or_default();
            let attackers = kill
                .attackers
                .iter()
                .map(|attacker| resolved_attacker(attacker, &names))
                .collect::<Vec<_>>();

            ZkillKill {
                killmail_id: kill.killmail_id,
                killmail_time: kill.killmail_time,
                victim_ship_type_name,
                victim_character_name,
                victim_corporation_name,
                victim_alliance_name,
                victim_damage_taken,
                attacker_count: attackers.len(),
                attackers,
                items,
                fitted_value: zkb.fitted_value,
                dropped_value: zkb.dropped_value,
                destroyed_value: zkb.destroyed_value,
                total_value: zkb.total_value,
                points: zkb.points,
                solo: zkb.solo,
                npc: zkb.npc,
                awox: zkb.awox,
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
        collect_item_name_ids(&victim.items, &mut ids);
        for attacker in &kill.attackers {
            for id in [
                attacker.character_id,
                attacker.corporation_id,
                attacker.alliance_id,
                attacker.faction_id,
                attacker.ship_type_id,
                attacker.weapon_type_id,
            ]
            .into_iter()
            .flatten()
            {
                ids.insert(id);
            }
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

fn collect_item_name_ids(items: &[ApiItem], ids: &mut HashSet<u64>) {
    for item in items {
        ids.insert(item.item_type_id);
        collect_item_name_ids(&item.items, ids);
    }
}

fn resolved_attacker(attacker: &ApiAttacker, names: &HashMap<u64, String>) -> ZkillAttacker {
    ZkillAttacker {
        character_name: resolved_name(attacker.character_id, names),
        corporation_name: resolved_name(attacker.corporation_id, names),
        alliance_name: resolved_name(attacker.alliance_id, names),
        faction_name: resolved_name(attacker.faction_id, names),
        ship_type_name: resolved_name(attacker.ship_type_id, names),
        weapon_type_name: resolved_name(attacker.weapon_type_id, names),
        damage_done: attacker.damage_done,
        final_blow: attacker.final_blow,
        security_status: attacker.security_status,
    }
}

fn resolved_items(items: &[ApiItem], names: &HashMap<u64, String>) -> Vec<ZkillItem> {
    let mut resolved = Vec::new();
    for item in items {
        resolved.push(ZkillItem {
            type_name: names
                .get(&item.item_type_id)
                .cloned()
                .unwrap_or_else(|| format!("Type {}", item.item_type_id)),
            quantity_destroyed: item.quantity_destroyed,
            quantity_dropped: item.quantity_dropped,
        });
        resolved.extend(resolved_items(&item.items, names));
    }
    resolved
}

fn resolved_name(id: Option<u64>, names: &HashMap<u64, String>) -> Option<String> {
    id.and_then(|id| names.get(&id).cloned())
}
