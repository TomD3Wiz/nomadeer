use crate::model::CachedKillmail;
use crate::util::{DAY_SECS, HOUR_SECS, hour_start};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const USER_AGENT: &str = "Nomadeer/0.1 private wormhole mapper";
const ZKILL_PAGE_SIZE: usize = 200;
const MAX_ZKILL_PAGES: usize = 10;

#[derive(Debug, Clone)]
pub(crate) struct ZkillReport {
    pub(crate) solar_system_id: u64,
    pub(crate) recent_kills: Vec<ZkillKill>,
    pub(crate) history: ZkillHistory,
    pub(crate) cached_killmails: Vec<CachedKillmail>,
    pub(crate) total_value: f64,
    pub(crate) solo_kills: usize,
    pub(crate) npc_kills: usize,
}

/// Kill counts and attacker figures cover kills by players. Ships lost only to NPCs
/// are counted separately in `npc_kills_*`.
#[derive(Debug, Clone, Default)]
pub(crate) struct ZkillHistory {
    pub(crate) kills_1h: usize,
    pub(crate) kills_4h: usize,
    pub(crate) kills_8h: usize,
    pub(crate) kills_24h: usize,
    pub(crate) kills_7d: usize,
    /// Kills in this UTC hour of day on the previous 7 days, leaving out today's.
    pub(crate) kills_current_hour_7d: usize,
    pub(crate) solo_24h: usize,
    pub(crate) npc_kills_24h: usize,
    pub(crate) npc_kills_7d: usize,
    pub(crate) isk_destroyed_24h: f64,
    pub(crate) unique_attackers_24h: usize,
    pub(crate) busiest_attacker_24h: usize,
    pub(crate) busiest_corporation_24h: usize,
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

pub(crate) fn fetch_system_report(
    solar_system_id: u64,
    cached_killmails: Vec<CachedKillmail>,
) -> Result<ZkillReport, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("Could not start zKill request: {error}"))?;
    let oldest_cached = cached_killmails.iter().map(|kill| kill.killmail_time).min();
    let mut kills = Vec::new();
    for page in 1..=MAX_ZKILL_PAGES {
        let page_kills = fetch_kill_page(&client, solar_system_id, page)?;
        let page_len = page_kills.len();
        let page_oldest = page_kills
            .iter()
            .filter_map(|kill| parse_killmail_time(&kill.killmail_time))
            .min();
        kills.extend(page_kills);

        let reached_cache = oldest_cached
            .zip(page_oldest)
            .is_some_and(|(cached, fetched)| fetched <= cached);
        if page_len < ZKILL_PAGE_SIZE || reached_cache {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }

    let cached_killmails = merge_cached_killmails(solar_system_id, cached_killmails, &kills);
    let history = summarize_history(&cached_killmails, crate::util::now());
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
        history,
        cached_killmails,
        total_value,
        solo_kills,
        npc_kills,
    })
}

fn fetch_kill_page(
    client: &Client,
    solar_system_id: u64,
    page: usize,
) -> Result<Vec<ApiKillmail>, String> {
    let page_suffix = if page == 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    let url = format!(
        "https://zkillboard.com/api/kills/solarSystemID/{solar_system_id}/pastSeconds/604800/{page_suffix}"
    );
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
    serde_json::from_value::<Vec<ApiKillmail>>(json)
        .map_err(|error| format!("Could not read zKillboard kills: {error}"))
}

fn merge_cached_killmails(
    solar_system_id: u64,
    cached_killmails: Vec<CachedKillmail>,
    fetched_kills: &[ApiKillmail],
) -> Vec<CachedKillmail> {
    let cutoff = crate::util::now().saturating_sub(7 * 24 * 60 * 60);
    let mut merged = cached_killmails
        .into_iter()
        .filter(|kill| kill.solar_system_id == solar_system_id && kill.killmail_time >= cutoff)
        .map(|kill| (kill.killmail_id, kill))
        .collect::<HashMap<_, _>>();

    for kill in fetched_kills {
        let Some(killmail_time) = parse_killmail_time(&kill.killmail_time) else {
            continue;
        };
        let attacker_ids = kill
            .attackers
            .iter()
            .filter_map(|attacker| attacker.character_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let corporation_ids = kill
            .attackers
            .iter()
            .filter_map(|attacker| attacker.corporation_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let (total_value, solo, npc) = kill
            .zkb
            .as_ref()
            .map(|zkb| (zkb.total_value, zkb.solo, zkb.npc))
            .unwrap_or_default();
        merged.insert(
            kill.killmail_id,
            CachedKillmail {
                killmail_id: kill.killmail_id,
                solar_system_id,
                killmail_time,
                total_value,
                solo,
                npc,
                attacker_ids,
                corporation_ids,
            },
        );
    }

    let mut kills = merged.into_values().collect::<Vec<_>>();
    kills.sort_by_key(|kill| std::cmp::Reverse(kill.killmail_time));
    kills
}

fn summarize_history(kills: &[CachedKillmail], current_time: u64) -> ZkillHistory {
    let current_hour_start = hour_start(current_time);
    let current_utc_hour = current_hour_start / HOUR_SECS % 24;
    let pattern_start = current_hour_start.saturating_sub(7 * DAY_SECS);
    let mut history = ZkillHistory::default();
    let mut attackers = HashSet::new();
    let mut attacker_kills = HashMap::<u64, usize>::new();
    let mut corporation_kills = HashMap::<u64, usize>::new();

    for kill in kills {
        let age = current_time.saturating_sub(kill.killmail_time);
        if kill.npc {
            // Sleepers, rats and CONCORD are activity, not hostile pilots.
            history.npc_kills_7d += usize::from(age <= 7 * DAY_SECS);
            history.npc_kills_24h += usize::from(age <= DAY_SECS);
            continue;
        }
        if age <= 7 * DAY_SECS {
            history.kills_7d += 1;
        }
        // Earlier days only: this hour's kills already count in the last-hour window.
        if (pattern_start..current_hour_start).contains(&kill.killmail_time)
            && kill.killmail_time / HOUR_SECS % 24 == current_utc_hour
        {
            history.kills_current_hour_7d += 1;
        }
        if age <= DAY_SECS {
            history.kills_24h += 1;
            history.isk_destroyed_24h += kill.total_value;
            history.solo_24h += usize::from(kill.solo);
            for id in &kill.attacker_ids {
                attackers.insert(*id);
                *attacker_kills.entry(*id).or_default() += 1;
            }
            for id in &kill.corporation_ids {
                *corporation_kills.entry(*id).or_default() += 1;
            }
        }
        if age <= 8 * HOUR_SECS {
            history.kills_8h += 1;
        }
        if age <= 4 * HOUR_SECS {
            history.kills_4h += 1;
        }
        if age <= HOUR_SECS {
            history.kills_1h += 1;
        }
    }

    history.unique_attackers_24h = attackers.len();
    history.busiest_attacker_24h = attacker_kills.values().copied().max().unwrap_or(0);
    history.busiest_corporation_24h = corporation_kills.values().copied().max().unwrap_or(0);
    history
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

fn parse_killmail_time(value: &str) -> Option<u64> {
    if value.len() < 20 {
        return None;
    }

    let year = value.get(0..4)?.parse::<i64>().ok()?;
    let month = value.get(5..7)?.parse::<i64>().ok()?;
    let day = value.get(8..10)?.parse::<i64>().ok()?;
    let hour = value.get(11..13)?.parse::<i64>().ok()?;
    let minute = value.get(14..16)?.parse::<i64>().ok()?;
    let second = value.get(17..19)?.parse::<i64>().ok()?;

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }

    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let timestamp = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(timestamp).ok()
}

#[cfg(test)]
mod tests {
    use super::summarize_history;
    use crate::model::CachedKillmail;
    use crate::util::{DAY_SECS, HOUR_SECS};

    /// Thirty minutes into an hour.
    const NOW: u64 = 20_000 * HOUR_SECS + 1_800;

    fn killmail(killmail_id: u64, killmail_time: u64, npc: bool) -> CachedKillmail {
        CachedKillmail {
            killmail_id,
            solar_system_id: 31_002_238,
            killmail_time,
            total_value: 100_000_000.0,
            solo: false,
            npc,
            attacker_ids: if npc { Vec::new() } else { vec![90_000_001] },
            corporation_ids: vec![if npc { 500_021 } else { 98_000_001 }],
        }
    }

    #[test]
    fn npc_losses_are_kept_out_of_player_kills() {
        let history = summarize_history(
            &[
                killmail(1, NOW - 600, true),
                killmail(2, NOW - 900, true),
                killmail(3, NOW - 2 * DAY_SECS, true),
                killmail(4, NOW - 1_200, false),
            ],
            NOW,
        );
        assert_eq!(history.kills_1h, 1);
        assert_eq!(history.kills_7d, 1);
        assert_eq!(history.npc_kills_24h, 2);
        assert_eq!(history.npc_kills_7d, 3);
        assert_eq!(history.busiest_corporation_24h, 1);
        assert_eq!(history.isk_destroyed_24h, 100_000_000.0);
    }

    #[test]
    fn time_of_day_pattern_leaves_out_this_hour() {
        let history = summarize_history(
            &[
                killmail(1, NOW - 600, false),
                killmail(2, NOW - DAY_SECS, false),
                killmail(3, NOW - 3 * DAY_SECS - 600, false),
                killmail(4, NOW - DAY_SECS - 2 * HOUR_SECS, false),
                killmail(5, NOW - 8 * DAY_SECS, false),
            ],
            NOW,
        );
        assert_eq!(history.kills_1h, 1);
        assert_eq!(history.kills_current_hour_7d, 2);
    }
}
