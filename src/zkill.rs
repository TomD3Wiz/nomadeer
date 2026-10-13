use reqwest::blocking::Client;
use serde::Deserialize;
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

pub(crate) fn fetch_system_report(solar_system_id: u64) -> Result<ZkillReport, String> {
    let url = format!("https://zkillboard.com/api/kills/solarSystemID/{solar_system_id}/limit/10/");
    let kills = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("Could not start zKill request: {error}"))?
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .map_err(|error| format!("Could not reach zKillboard: {error}"))?;

    if !kills.status().is_success() {
        let status = kills.status();
        let body = kills
            .text()
            .unwrap_or_else(|_| "<unreadable body>".to_owned());
        return Err(format!("zKillboard returned {status}: {body}"));
    }

    let kills = kills
        .json::<Vec<ApiKillmail>>()
        .map_err(|error| format!("Could not parse zKillboard response: {error}"))?;

    let recent_kills = kills
        .into_iter()
        .map(|kill| {
            let zkb = kill.zkb.unwrap_or(ApiZkb {
                total_value: 0.0,
                solo: false,
                npc: false,
            });
            ZkillKill {
                killmail_id: kill.killmail_id,
                killmail_time: kill.killmail_time,
                victim_ship_type_id: kill.victim.and_then(|victim| victim.ship_type_id),
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
