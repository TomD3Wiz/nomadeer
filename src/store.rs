use crate::model::{
    Account, AppData, AutoIntelSettings, CachedKillmail, Connection, DashboardLayout, EsiSettings,
    KillCounts, KillboardCoverage, Note, Signature, SystemAssessment, SystemNode,
    SystemTrafficHistory, TrafficHour, TrafficReading, TrafficRows, TrafficSample, UmbraBriefing,
    VisitLog,
};
use crate::paths;
use crate::util::{DAY_SECS, hour_start, now};
use rusqlite::{
    Connection as SqliteConnection, OptionalExtension, Row, TransactionBehavior, params,
    types::Type,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Killboard history covers the last week.
pub(crate) const KILLMAIL_RETENTION_SECS: u64 = 7 * DAY_SECS;
/// A week of hourly traffic for baselines plus the current day.
pub(crate) const TRAFFIC_RETENTION_SECS: u64 = 8 * DAY_SECS;
/// Umbra's feed keeps a month of briefings, and never more than this many.
pub(crate) const UMBRA_FEED_RETENTION_SECS: u64 = 30 * DAY_SECS;
pub(crate) const UMBRA_FEED_MAX_KEPT: usize = 500;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const GATHER_ON_ARRIVAL_KEY: &str = "gather_intel_on_arrival";
const UMBRA_BRIEFS_AUTOMATICALLY_KEY: &str = "umbra_briefs_automatically";
const RECON_DEFAULTS_APPLIED_KEY: &str = "recon_defaults_applied";
const DASHBOARD_TABS_FRACTION_KEY: &str = "dashboard_tabs_fraction";

/// Handle to the SQLite database. Clones share schema setup and can be moved to
/// background threads; each operation opens its own connection.
/// Everything kept in `app_settings`.
#[derive(Default)]
struct Settings {
    esi_settings: EsiSettings,
    auto_intel: AutoIntelSettings,
    recon_defaults_applied: bool,
    dashboard_layout: DashboardLayout,
}

#[derive(Clone)]
pub(crate) struct Store {
    path: PathBuf,
    /// Folders searched for data left by older Nomadeer versions.
    legacy_dirs: Arc<[PathBuf]>,
    schema_ready: Arc<AtomicBool>,
}

impl Store {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            legacy_dirs: Arc::new([]),
            schema_ready: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Lets an empty database pick up data older versions kept in `legacy_dirs`.
    pub(crate) fn with_legacy_dirs(mut self, legacy_dirs: Vec<PathBuf>) -> Self {
        self.legacy_dirs = legacy_dirs.into();
        self
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn load(&self) -> Result<AppData, String> {
        let conn = self.open()?;
        self.prune_expired_intel(&conn, now())?;
        let mut data = self.load_all(&conn)?;

        if self.is_empty(&data)
            && let Some(imported) = self.import_legacy_json()?
        {
            self.save(&imported)?;
            data = imported;
        }

        Ok(data)
    }

    /// Rewrites the map, characters and settings. Killmails and traffic history are
    /// stored through their own methods and are left untouched.
    pub(crate) fn save(&self, data: &AppData) -> Result<(), String> {
        let mut conn = self.open()?;
        let tx = Self::write_transaction(&mut conn)?;

        tx.execute_batch(
            "DELETE FROM app_settings;
             DELETE FROM accounts;
             DELETE FROM systems;
             DELETE FROM connections;
             DELETE FROM signatures;
             DELETE FROM visits;
             DELETE FROM notes;
             DELETE FROM system_assessments;",
        )
        .map_err(|error| error.to_string())?;

        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)",
            params!["esi_client_id", data.esi_settings.client_id],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)",
            params![
                "esi_callback_port",
                data.esi_settings.callback_port.to_string()
            ],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)",
            params!["esi_scopes", data.esi_settings.scopes],
        )
        .map_err(|error| error.to_string())?;
        for (key, enabled) in [
            (GATHER_ON_ARRIVAL_KEY, data.auto_intel.gather_on_arrival),
            (
                UMBRA_BRIEFS_AUTOMATICALLY_KEY,
                data.auto_intel.umbra_briefs_automatically,
            ),
            (RECON_DEFAULTS_APPLIED_KEY, data.recon_defaults_applied),
        ] {
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES (?1, ?2)",
                params![key, if enabled { "1" } else { "0" }],
            )
            .map_err(|error| error.to_string())?;
        }
        tx.execute(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)",
            params![
                DASHBOARD_TABS_FRACTION_KEY,
                data.dashboard_layout.tabs_fraction.to_string()
            ],
        )
        .map_err(|error| error.to_string())?;

        for account in &data.accounts {
            tx.execute(
                "INSERT INTO accounts (
                    id, character_id, character_name, owner_hash, auth_status,
                    last_system_id, last_system_name, access_token, refresh_token,
                    token_expires_at, created_at, updated_at, recon
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    account.id,
                    account.character_id,
                    account.character_name,
                    account.owner_hash,
                    account.auth_status,
                    account.last_system_id,
                    account.last_system_name,
                    account.access_token,
                    account.refresh_token,
                    account.token_expires_at.map(|value| value as i64),
                    account.created_at as i64,
                    account.updated_at as i64,
                    account.recon,
                ],
            )
            .map_err(|error| error.to_string())?;
        }

        for system in &data.systems {
            tx.execute(
                "INSERT INTO systems (
                    id, solar_system_id, name, class, security_status, constellation_id,
                    constellation_name, region_id, region_name, planet_count, stargate_count,
                    station_count, jumps_1h, ship_kills_1h, pod_kills_1h, npc_kills_1h,
                    intel_updated_at, effect, status, owner, notes, created_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                    ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22
                 )",
                params![
                    system.id,
                    system.solar_system_id as i64,
                    system.name,
                    system.class,
                    system.security_status,
                    system.constellation_id as i64,
                    system.constellation_name,
                    system.region_id as i64,
                    system.region_name,
                    system.planet_count as i64,
                    system.stargate_count as i64,
                    system.station_count as i64,
                    system.jumps_1h as i64,
                    system.ship_kills_1h as i64,
                    system.pod_kills_1h as i64,
                    system.npc_kills_1h as i64,
                    system.intel_updated_at as i64,
                    system.effect,
                    system.status,
                    system.owner,
                    system.notes,
                    system.created_at as i64,
                ],
            )
            .map_err(|error| error.to_string())?;
        }

        for connection in &data.connections {
            tx.execute(
                "INSERT INTO connections (
                    id, from_system, to_system, signature_from, signature_to,
                    mass_status, life_status, ship_size, notes, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    connection.id,
                    connection.from_system,
                    connection.to_system,
                    connection.signature_from,
                    connection.signature_to,
                    connection.mass_status,
                    connection.life_status,
                    connection.ship_size,
                    connection.notes,
                    connection.created_at as i64,
                ],
            )
            .map_err(|error| error.to_string())?;
        }

        for signature in &data.signatures {
            tx.execute(
                "INSERT INTO signatures (
                    id, system_id, code, sig_type, target, life, status, notes, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    signature.id,
                    signature.system_id,
                    signature.code,
                    signature.sig_type,
                    signature.target,
                    signature.life,
                    signature.status,
                    signature.notes,
                    signature.created_at as i64,
                ],
            )
            .map_err(|error| error.to_string())?;
        }

        for visit in &data.visits {
            tx.execute(
                "INSERT INTO visits (id, system_id, pilot, activity, notes, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    visit.id,
                    visit.system_id,
                    visit.pilot,
                    visit.activity,
                    visit.notes,
                    visit.created_at as i64,
                ],
            )
            .map_err(|error| error.to_string())?;
        }

        for note in &data.notes {
            tx.execute(
                "INSERT INTO notes (id, system_id, title, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    note.id,
                    note.system_id,
                    note.title,
                    note.body,
                    note.created_at as i64,
                ],
            )
            .map_err(|error| error.to_string())?;
        }

        for assessment in &data.system_assessments {
            let evidence =
                serde_json::to_string(&assessment.evidence).map_err(|error| error.to_string())?;
            tx.execute(
                "INSERT INTO system_assessments (
                    system_id, threat_score, activity_score, confidence, evidence, briefing,
                    model_generated, generated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    assessment.system_id,
                    assessment.threat_score as i64,
                    assessment.activity_score as i64,
                    assessment.confidence,
                    evidence,
                    assessment.briefing,
                    i64::from(assessment.model_generated),
                    assessment.generated_at as i64,
                ],
            )
            .map_err(|error| error.to_string())?;
        }

        tx.commit().map_err(|error| error.to_string())
    }

    /// Cached killmails for a system at or after `since`, newest first.
    pub(crate) fn cached_killmails(
        &self,
        solar_system_id: u64,
        since: u64,
    ) -> Result<Vec<CachedKillmail>, String> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT killmail_id, solar_system_id, killmail_time, total_value, solo, npc,
                        attacker_ids, corporation_ids
                 FROM cached_killmails
                 WHERE solar_system_id = ?1 AND killmail_time >= ?2
                 ORDER BY killmail_time DESC",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map(params![solar_system_id as i64, since as i64], |row| {
                let attacker_ids_json = row.get::<_, String>(6)?;
                let corporation_ids_json = row.get::<_, String>(7)?;
                let attacker_ids = serde_json::from_str(&attacker_ids_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(6, Type::Text, Box::new(error))
                })?;
                let corporation_ids =
                    serde_json::from_str(&corporation_ids_json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(7, Type::Text, Box::new(error))
                    })?;
                Ok(CachedKillmail {
                    killmail_id: row.get::<_, i64>(0)? as u64,
                    solar_system_id: row.get::<_, i64>(1)? as u64,
                    killmail_time: row.get::<_, i64>(2)? as u64,
                    total_value: row.get(3)?,
                    solo: row.get::<_, i64>(4)? != 0,
                    npc: row.get::<_, i64>(5)? != 0,
                    attacker_ids,
                    corporation_ids,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    /// How far back a system's cached killmails are known to be complete, if they have
    /// been fetched since coverage was first recorded.
    pub(crate) fn killboard_coverage(
        &self,
        solar_system_id: u64,
    ) -> Result<Option<KillboardCoverage>, String> {
        let conn = self.open()?;
        conn.query_row(
            "SELECT covered_since, covered_until FROM killboard_coverage
             WHERE solar_system_id = ?1",
            params![solar_system_id as i64],
            |row| {
                Ok(KillboardCoverage {
                    solar_system_id,
                    since: row.get::<_, i64>(0)? as u64,
                    until: row.get::<_, i64>(1)? as u64,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
    }

    /// Upserts killmails by id and drops any older than the retention window. The
    /// coverage, when given, is written in the same transaction so it never claims
    /// kills that were not saved.
    pub(crate) fn store_killmails(
        &self,
        kills: &[CachedKillmail],
        coverage: Option<&KillboardCoverage>,
        current_time: u64,
    ) -> Result<(), String> {
        let mut conn = self.open()?;
        let tx = Self::write_transaction(&mut conn)?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR REPLACE INTO cached_killmails (
                        killmail_id, solar_system_id, killmail_time, total_value, solo, npc,
                        attacker_ids, corporation_ids
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )
                .map_err(|error| error.to_string())?;
            for kill in kills {
                let attacker_ids =
                    serde_json::to_string(&kill.attacker_ids).map_err(|error| error.to_string())?;
                let corporation_ids = serde_json::to_string(&kill.corporation_ids)
                    .map_err(|error| error.to_string())?;
                stmt.execute(params![
                    kill.killmail_id as i64,
                    kill.solar_system_id as i64,
                    kill.killmail_time as i64,
                    kill.total_value,
                    i64::from(kill.solo),
                    i64::from(kill.npc),
                    attacker_ids,
                    corporation_ids,
                ])
                .map_err(|error| error.to_string())?;
            }
        }
        if let Some(coverage) = coverage {
            tx.execute(
                "INSERT OR REPLACE INTO killboard_coverage (
                    solar_system_id, covered_since, covered_until
                 ) VALUES (?1, ?2, ?3)",
                params![
                    coverage.solar_system_id as i64,
                    coverage.since as i64,
                    coverage.until as i64,
                ],
            )
            .map_err(|error| error.to_string())?;
        }
        Self::prune_killmails(&tx, current_time)?;
        tx.commit().map_err(|error| error.to_string())
    }

    /// Records one ESI hour of a universe-wide feed in a single transaction. Only systems
    /// with activity get a row; the captured hour itself is what makes the rest count as
    /// zero. A capture older than the one already stored for that hour is ignored.
    pub(crate) fn record_traffic_hour(
        &self,
        traffic: &TrafficHour,
        current_time: u64,
    ) -> Result<(), String> {
        let mut conn = self.open()?;
        let tx = Self::write_transaction(&mut conn)?;
        let (time_column, reset_sql) = match traffic.rows {
            TrafficRows::Jumps(_) => (
                "jumps_data_time",
                "UPDATE system_traffic SET jumps = 0 WHERE hour = ?1",
            ),
            TrafficRows::Kills(_) => (
                "kills_data_time",
                "UPDATE system_traffic SET ship_kills = 0, pod_kills = 0, npc_kills = 0
                 WHERE hour = ?1",
            ),
        };
        let stored_time = tx
            .query_row(
                &format!("SELECT {time_column} FROM traffic_hours WHERE hour = ?1"),
                params![traffic.hour as i64],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .flatten();
        if stored_time.is_some_and(|stored| stored as u64 > traffic.data_time) {
            return Ok(());
        }

        tx.execute(
            &format!(
                "INSERT INTO traffic_hours (hour, {time_column}) VALUES (?1, ?2)
                 ON CONFLICT(hour) DO UPDATE SET {time_column} = excluded.{time_column}"
            ),
            params![traffic.hour as i64, traffic.data_time as i64],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(reset_sql, params![traffic.hour as i64])
            .map_err(|error| error.to_string())?;

        match &traffic.rows {
            TrafficRows::Jumps(rows) => {
                let mut stmt = tx
                    .prepare_cached(
                        "INSERT INTO system_traffic (solar_system_id, hour, jumps)
                         VALUES (?1, ?2, ?3)
                         ON CONFLICT(solar_system_id, hour) DO UPDATE SET jumps = excluded.jumps",
                    )
                    .map_err(|error| error.to_string())?;
                for (solar_system_id, jumps) in rows {
                    stmt.execute(params![
                        *solar_system_id as i64,
                        traffic.hour as i64,
                        i64::from(*jumps)
                    ])
                    .map_err(|error| error.to_string())?;
                }
            }
            TrafficRows::Kills(rows) => {
                let mut stmt = tx
                    .prepare_cached(
                        "INSERT INTO system_traffic (
                            solar_system_id, hour, ship_kills, pod_kills, npc_kills
                         ) VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(solar_system_id, hour) DO UPDATE SET
                            ship_kills = excluded.ship_kills,
                            pod_kills = excluded.pod_kills,
                            npc_kills = excluded.npc_kills",
                    )
                    .map_err(|error| error.to_string())?;
                for (solar_system_id, kills) in rows {
                    stmt.execute(params![
                        *solar_system_id as i64,
                        traffic.hour as i64,
                        i64::from(kills.ship_kills),
                        i64::from(kills.pod_kills),
                        i64::from(kills.npc_kills),
                    ])
                    .map_err(|error| error.to_string())?;
                }
            }
        }

        Self::prune_traffic(&tx, current_time)?;
        tx.commit().map_err(|error| error.to_string())
    }

    /// A system's captured hours from `since` onwards plus its latest reading per feed.
    pub(crate) fn system_traffic(
        &self,
        solar_system_id: u64,
        since: u64,
    ) -> Result<SystemTrafficHistory, String> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT h.hour, h.jumps_data_time, h.kills_data_time,
                        COALESCE(t.jumps, 0), COALESCE(t.ship_kills, 0),
                        COALESCE(t.pod_kills, 0), COALESCE(t.npc_kills, 0)
                 FROM traffic_hours h
                 LEFT JOIN system_traffic t
                    ON t.hour = h.hour AND t.solar_system_id = ?1
                 WHERE h.hour >= ?2
                 ORDER BY h.hour",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map(
                params![solar_system_id as i64, hour_start(since) as i64],
                |row| {
                    let jumps_captured = row.get::<_, Option<i64>>(1)?.is_some();
                    let kills_captured = row.get::<_, Option<i64>>(2)?.is_some();
                    let kills = traffic_kills(row, 4)?;
                    Ok(TrafficSample {
                        hour: row.get::<_, i64>(0)? as u64,
                        jumps: if jumps_captured {
                            Some(row.get::<_, i64>(3)? as u32)
                        } else {
                            None
                        },
                        kills: kills_captured.then_some(kills),
                    })
                },
            )
            .map_err(|error| error.to_string())?;
        let samples = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;

        let latest_jumps = conn
            .query_row(
                "SELECT h.hour, h.jumps_data_time, COALESCE(t.jumps, 0)
                 FROM traffic_hours h
                 LEFT JOIN system_traffic t
                    ON t.hour = h.hour AND t.solar_system_id = ?1
                 WHERE h.jumps_data_time IS NOT NULL
                 ORDER BY h.hour DESC
                 LIMIT 1",
                params![solar_system_id as i64],
                |row| {
                    Ok(TrafficReading {
                        hour: row.get::<_, i64>(0)? as u64,
                        data_time: row.get::<_, i64>(1)? as u64,
                        value: row.get::<_, i64>(2)? as u32,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let latest_kills = conn
            .query_row(
                "SELECT h.hour, h.kills_data_time, COALESCE(t.ship_kills, 0),
                        COALESCE(t.pod_kills, 0), COALESCE(t.npc_kills, 0)
                 FROM traffic_hours h
                 LEFT JOIN system_traffic t
                    ON t.hour = h.hour AND t.solar_system_id = ?1
                 WHERE h.kills_data_time IS NOT NULL
                 ORDER BY h.hour DESC
                 LIMIT 1",
                params![solar_system_id as i64],
                |row| {
                    Ok(TrafficReading {
                        hour: row.get::<_, i64>(0)? as u64,
                        data_time: row.get::<_, i64>(1)? as u64,
                        value: traffic_kills(row, 2)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;

        Ok(SystemTrafficHistory {
            latest_jumps,
            latest_kills,
            samples,
        })
    }

    /// Adds an accepted briefing to Umbra's feed and prunes the feed. Returns the entry
    /// with its id, or None when the feed already holds a briefing of that assessment.
    pub(crate) fn record_umbra_briefing(
        &self,
        entry: &UmbraBriefing,
        current_time: u64,
    ) -> Result<Option<UmbraBriefing>, String> {
        let mut conn = self.open()?;
        let tx = Self::write_transaction(&mut conn)?;
        let inserted = tx
            .execute(
                "INSERT OR IGNORE INTO umbra_briefings (
                    solar_system_id, system_id, system_name, system_class, threat_score,
                    activity_score, confidence, briefing, assessed_at, briefed_at, auto
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    entry.solar_system_id as i64,
                    entry.system_id,
                    entry.system_name,
                    entry.system_class,
                    i64::from(entry.threat_score),
                    i64::from(entry.activity_score),
                    entry.confidence,
                    entry.briefing,
                    entry.assessed_at as i64,
                    entry.briefed_at as i64,
                    i64::from(entry.auto),
                ],
            )
            .map_err(|error| error.to_string())?;
        let saved = (inserted > 0).then(|| UmbraBriefing {
            id: tx.last_insert_rowid(),
            ..entry.clone()
        });
        Self::prune_umbra_briefings(&tx, current_time)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(saved)
    }

    /// The newest `limit` briefings in Umbra's feed, oldest first.
    pub(crate) fn umbra_briefings(&self, limit: usize) -> Result<Vec<UmbraBriefing>, String> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, solar_system_id, system_id, system_name, system_class, threat_score,
                        activity_score, confidence, briefing, assessed_at, briefed_at, auto
                 FROM umbra_briefings
                 ORDER BY briefed_at DESC, id DESC
                 LIMIT ?1",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(UmbraBriefing {
                    id: row.get(0)?,
                    solar_system_id: row.get::<_, i64>(1)? as u64,
                    system_id: row.get(2)?,
                    system_name: row.get(3)?,
                    system_class: row.get(4)?,
                    threat_score: row.get::<_, i64>(5)?.clamp(0, 100) as u8,
                    activity_score: row.get::<_, i64>(6)?.clamp(0, 100) as u8,
                    confidence: row.get(7)?,
                    briefing: row.get(8)?,
                    assessed_at: row.get::<_, i64>(9)? as u64,
                    briefed_at: row.get::<_, i64>(10)? as u64,
                    auto: row.get::<_, i64>(11)? != 0,
                })
            })
            .map_err(|error| error.to_string())?;
        let mut feed = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        feed.reverse();
        Ok(feed)
    }

    /// Forgets every briefing in Umbra's feed.
    pub(crate) fn clear_umbra_briefings(&self) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute("DELETE FROM umbra_briefings", [])
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn prune_umbra_briefings(conn: &SqliteConnection, current_time: u64) -> Result<(), String> {
        let cutoff = current_time.saturating_sub(UMBRA_FEED_RETENTION_SECS);
        conn.execute(
            "DELETE FROM umbra_briefings WHERE briefed_at < ?1",
            params![cutoff as i64],
        )
        .map_err(|error| error.to_string())?;
        conn.execute(
            "DELETE FROM umbra_briefings WHERE id NOT IN (
                SELECT id FROM umbra_briefings ORDER BY briefed_at DESC, id DESC LIMIT ?1
             )",
            params![UMBRA_FEED_MAX_KEPT as i64],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn prune_expired_intel(
        &self,
        conn: &SqliteConnection,
        current_time: u64,
    ) -> Result<(), String> {
        Self::prune_killmails(conn, current_time)?;
        Self::prune_traffic(conn, current_time)?;
        Self::prune_umbra_briefings(conn, current_time)
    }

    fn prune_killmails(conn: &SqliteConnection, current_time: u64) -> Result<(), String> {
        let cutoff = current_time.saturating_sub(KILLMAIL_RETENTION_SECS);
        conn.execute(
            "DELETE FROM cached_killmails WHERE killmail_time < ?1",
            params![cutoff as i64],
        )
        .map_err(|error| error.to_string())?;
        conn.execute(
            "DELETE FROM killboard_coverage WHERE covered_until < ?1",
            params![cutoff as i64],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn prune_traffic(conn: &SqliteConnection, current_time: u64) -> Result<(), String> {
        let cutoff = hour_start(current_time).saturating_sub(TRAFFIC_RETENTION_SECS);
        conn.execute(
            "DELETE FROM system_traffic WHERE hour < ?1",
            params![cutoff as i64],
        )
        .map_err(|error| error.to_string())?;
        conn.execute(
            "DELETE FROM traffic_hours WHERE hour < ?1",
            params![cutoff as i64],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    /// Takes the write lock up front, where the busy timeout applies. A deferred
    /// transaction that reads first cannot wait for another writer when it upgrades.
    fn write_transaction(conn: &mut SqliteConnection) -> Result<rusqlite::Transaction<'_>, String> {
        conn.transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())
    }

    fn open(&self) -> Result<SqliteConnection, String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        self.copy_legacy_sqlite_if_needed()?;
        let conn = SqliteConnection::open(&self.path).map_err(|error| error.to_string())?;
        // Background intel writes and UI saves use separate connections.
        conn.busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| error.to_string())?;
        if !self.schema_ready.load(Ordering::Acquire) {
            self.init_schema(&conn)?;
            self.schema_ready.store(true, Ordering::Release);
        }
        Ok(conn)
    }

    fn copy_legacy_sqlite_if_needed(&self) -> Result<(), String> {
        if self.path.exists() {
            return Ok(());
        }
        let Some(legacy_path) = self
            .legacy_dirs
            .iter()
            .map(|dir| dir.join(paths::LEGACY_DATABASE_FILE))
            .find(|path| *path != self.path && path.is_file())
        else {
            return Ok(());
        };

        fs::copy(&legacy_path, &self.path).map_err(|error| error.to_string())?;

        for suffix in ["-wal", "-shm"] {
            let legacy_sidecar = sidecar_path(&legacy_path, suffix);
            let new_sidecar = sidecar_path(&self.path, suffix);
            if legacy_sidecar.exists() {
                fs::copy(legacy_sidecar, new_sidecar).map_err(|error| error.to_string())?;
            }
        }

        Ok(())
    }

    fn init_schema(&self, conn: &SqliteConnection) -> Result<(), String> {
        // WAL lets the UI keep reading while background intel is written.
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE IF NOT EXISTS accounts (
                id TEXT PRIMARY KEY,
                character_id TEXT NOT NULL DEFAULT '',
                character_name TEXT NOT NULL,
                owner_hash TEXT NOT NULL DEFAULT '',
                auth_status TEXT NOT NULL DEFAULT 'unauthorized',
                last_system_id TEXT NOT NULL DEFAULT '',
                last_system_name TEXT NOT NULL DEFAULT '',
                access_token TEXT,
                refresh_token TEXT,
                token_expires_at INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                recon INTEGER NOT NULL DEFAULT 0
             );
              CREATE TABLE IF NOT EXISTS systems (
                 id TEXT PRIMARY KEY,
                 solar_system_id INTEGER NOT NULL DEFAULT 0,
                 name TEXT NOT NULL,
                 class TEXT NOT NULL DEFAULT '',
                 security_status REAL NOT NULL DEFAULT 0,
                 constellation_id INTEGER NOT NULL DEFAULT 0,
                 constellation_name TEXT NOT NULL DEFAULT '',
                 region_id INTEGER NOT NULL DEFAULT 0,
                 region_name TEXT NOT NULL DEFAULT '',
                 planet_count INTEGER NOT NULL DEFAULT 0,
                 stargate_count INTEGER NOT NULL DEFAULT 0,
                 station_count INTEGER NOT NULL DEFAULT 0,
                 jumps_1h INTEGER NOT NULL DEFAULT 0,
                 ship_kills_1h INTEGER NOT NULL DEFAULT 0,
                 pod_kills_1h INTEGER NOT NULL DEFAULT 0,
                 npc_kills_1h INTEGER NOT NULL DEFAULT 0,
                 intel_updated_at INTEGER NOT NULL DEFAULT 0,
                 effect TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT '',
                owner TEXT NOT NULL DEFAULT '',
                notes TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS connections (
                id TEXT PRIMARY KEY,
                from_system TEXT NOT NULL,
                to_system TEXT NOT NULL,
                signature_from TEXT NOT NULL DEFAULT '',
                signature_to TEXT NOT NULL DEFAULT '',
                mass_status TEXT NOT NULL DEFAULT '',
                life_status TEXT NOT NULL DEFAULT '',
                ship_size TEXT NOT NULL DEFAULT '',
                notes TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS signatures (
                id TEXT PRIMARY KEY,
                system_id TEXT NOT NULL,
                code TEXT NOT NULL,
                sig_type TEXT NOT NULL DEFAULT '',
                target TEXT NOT NULL DEFAULT '',
                life TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT '',
                notes TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS visits (
                id TEXT PRIMARY KEY,
                system_id TEXT NOT NULL,
                pilot TEXT NOT NULL DEFAULT '',
                activity TEXT NOT NULL DEFAULT '',
                notes TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
             );
              CREATE TABLE IF NOT EXISTS notes (
                id TEXT PRIMARY KEY,
                system_id TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL DEFAULT '',
                body TEXT NOT NULL DEFAULT '',
                 created_at INTEGER NOT NULL
              );
              -- Replaced by traffic_hours/system_traffic, keyed by solar system.
              DROP TABLE IF EXISTS system_activity_snapshots;
              CREATE TABLE IF NOT EXISTS traffic_hours (
                 hour INTEGER PRIMARY KEY,
                 jumps_data_time INTEGER,
                 kills_data_time INTEGER
              );
              CREATE TABLE IF NOT EXISTS system_traffic (
                 solar_system_id INTEGER NOT NULL,
                 hour INTEGER NOT NULL,
                 jumps INTEGER NOT NULL DEFAULT 0,
                 ship_kills INTEGER NOT NULL DEFAULT 0,
                 pod_kills INTEGER NOT NULL DEFAULT 0,
                 npc_kills INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (solar_system_id, hour)
              ) WITHOUT ROWID;
              CREATE TABLE IF NOT EXISTS cached_killmails (
                 killmail_id INTEGER PRIMARY KEY,
                 solar_system_id INTEGER NOT NULL,
                 killmail_time INTEGER NOT NULL,
                 total_value REAL NOT NULL DEFAULT 0,
                 solo INTEGER NOT NULL DEFAULT 0,
                 npc INTEGER NOT NULL DEFAULT 0,
                 attacker_ids TEXT NOT NULL DEFAULT '[]',
                 corporation_ids TEXT NOT NULL DEFAULT '[]'
              );
              CREATE TABLE IF NOT EXISTS killboard_coverage (
                 solar_system_id INTEGER PRIMARY KEY,
                 covered_since INTEGER NOT NULL,
                 covered_until INTEGER NOT NULL
              );
              CREATE TABLE IF NOT EXISTS system_assessments (
                 system_id TEXT PRIMARY KEY,
                 threat_score INTEGER NOT NULL,
                 activity_score INTEGER NOT NULL,
                 confidence TEXT NOT NULL,
                 evidence TEXT NOT NULL DEFAULT '[]',
                 briefing TEXT NOT NULL DEFAULT '',
                 model_generated INTEGER NOT NULL DEFAULT 0,
                 generated_at INTEGER NOT NULL
              );
              CREATE TABLE IF NOT EXISTS umbra_briefings (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 solar_system_id INTEGER NOT NULL DEFAULT 0,
                 system_id TEXT NOT NULL,
                 system_name TEXT NOT NULL DEFAULT '',
                 system_class TEXT NOT NULL DEFAULT '',
                 threat_score INTEGER NOT NULL,
                 activity_score INTEGER NOT NULL,
                 confidence TEXT NOT NULL DEFAULT '',
                 briefing TEXT NOT NULL,
                 assessed_at INTEGER NOT NULL,
                 briefed_at INTEGER NOT NULL,
                 auto INTEGER NOT NULL DEFAULT 0,
                 UNIQUE (system_id, assessed_at)
              );
              CREATE INDEX IF NOT EXISTS idx_umbra_briefings_briefed_at
                 ON umbra_briefings(briefed_at);
              CREATE INDEX IF NOT EXISTS idx_signatures_system_id ON signatures(system_id);
              CREATE INDEX IF NOT EXISTS idx_visits_system_id ON visits(system_id);
              CREATE INDEX IF NOT EXISTS idx_notes_system_id ON notes(system_id);
              CREATE INDEX IF NOT EXISTS idx_system_traffic_hour ON system_traffic(hour);
              CREATE INDEX IF NOT EXISTS idx_killmails_system_time
                 ON cached_killmails(solar_system_id, killmail_time);",
        )
        .map_err(|error| error.to_string())?;

        self.add_missing_column(conn, "accounts", "recon", "INTEGER NOT NULL DEFAULT 0")?;
        self.add_missing_column(
            conn,
            "systems",
            "solar_system_id",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.add_missing_column(
            conn,
            "systems",
            "security_status",
            "REAL NOT NULL DEFAULT 0",
        )?;
        self.add_missing_column(
            conn,
            "systems",
            "constellation_id",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.add_missing_column(
            conn,
            "systems",
            "constellation_name",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        self.add_missing_column(conn, "systems", "region_id", "INTEGER NOT NULL DEFAULT 0")?;
        self.add_missing_column(conn, "systems", "region_name", "TEXT NOT NULL DEFAULT ''")?;
        self.add_missing_column(
            conn,
            "systems",
            "planet_count",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.add_missing_column(
            conn,
            "systems",
            "stargate_count",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.add_missing_column(
            conn,
            "systems",
            "station_count",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.add_missing_column(conn, "systems", "jumps_1h", "INTEGER NOT NULL DEFAULT 0")?;
        self.add_missing_column(
            conn,
            "systems",
            "ship_kills_1h",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.add_missing_column(
            conn,
            "systems",
            "pod_kills_1h",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.add_missing_column(
            conn,
            "systems",
            "npc_kills_1h",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.add_missing_column(
            conn,
            "systems",
            "intel_updated_at",
            "INTEGER NOT NULL DEFAULT 0",
        )?;

        Ok(())
    }

    fn add_missing_column(
        &self,
        conn: &SqliteConnection,
        table: &str,
        column: &str,
        definition: &str,
    ) -> Result<(), String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| error.to_string())?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| error.to_string())?;

        for existing in columns {
            if existing.map_err(|error| error.to_string())? == column {
                return Ok(());
            }
        }

        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn load_all(&self, conn: &SqliteConnection) -> Result<AppData, String> {
        let Settings {
            esi_settings,
            auto_intel,
            recon_defaults_applied,
            dashboard_layout,
        } = self.load_settings(conn)?;
        Ok(AppData {
            esi_settings,
            auto_intel,
            recon_defaults_applied,
            dashboard_layout,
            accounts: self.load_accounts(conn)?,
            systems: self.load_systems(conn)?,
            connections: self.load_connections(conn)?,
            signatures: self.load_signatures(conn)?,
            visits: self.load_visits(conn)?,
            notes: self.load_notes(conn)?,
            system_assessments: self.load_system_assessments(conn)?,
        })
    }

    /// Settings missing from the database, e.g. ones added after it was made, keep
    /// their defaults.
    fn load_settings(&self, conn: &SqliteConnection) -> Result<Settings, String> {
        let mut loaded = Settings::default();
        let mut stmt = conn
            .prepare("SELECT key, value FROM app_settings")
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?;

        for row in rows {
            let (key, value) = row.map_err(|error| error.to_string())?;
            match key.as_str() {
                "esi_client_id" => loaded.esi_settings.client_id = value,
                "esi_callback_port" => {
                    loaded.esi_settings.callback_port =
                        value.parse().unwrap_or(loaded.esi_settings.callback_port);
                }
                "esi_scopes" => loaded.esi_settings.scopes = value,
                GATHER_ON_ARRIVAL_KEY => loaded.auto_intel.gather_on_arrival = value != "0",
                UMBRA_BRIEFS_AUTOMATICALLY_KEY => {
                    loaded.auto_intel.umbra_briefs_automatically = value != "0";
                }
                RECON_DEFAULTS_APPLIED_KEY => loaded.recon_defaults_applied = value == "1",
                DASHBOARD_TABS_FRACTION_KEY => {
                    if let Ok(fraction) = value.parse::<f32>()
                        && DashboardLayout::is_valid_fraction(fraction)
                    {
                        loaded.dashboard_layout.tabs_fraction = fraction;
                    }
                }
                _ => {}
            }
        }

        Ok(loaded)
    }

    fn load_accounts(&self, conn: &SqliteConnection) -> Result<Vec<Account>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, character_id, character_name, owner_hash, auth_status,
                    last_system_id, last_system_name, access_token, refresh_token,
                    token_expires_at, created_at, updated_at, recon
             FROM accounts ORDER BY character_name",
            )
            .map_err(|error| error.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                let token_expires_at: Option<i64> = row.get(9)?;
                Ok(Account {
                    id: row.get(0)?,
                    character_id: row.get(1)?,
                    character_name: row.get(2)?,
                    owner_hash: row.get(3)?,
                    auth_status: row.get(4)?,
                    last_system_id: row.get(5)?,
                    last_system_name: row.get(6)?,
                    access_token: row.get(7)?,
                    refresh_token: row.get(8)?,
                    token_expires_at: token_expires_at.map(|value| value as u64),
                    created_at: row.get::<_, i64>(10)? as u64,
                    updated_at: row.get::<_, i64>(11)? as u64,
                    recon: row.get::<_, i64>(12)? != 0,
                })
            })
            .map_err(|error| error.to_string())?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    fn load_systems(&self, conn: &SqliteConnection) -> Result<Vec<SystemNode>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, solar_system_id, name, class, security_status, constellation_id,
                    constellation_name, region_id, region_name, planet_count, stargate_count,
                    station_count, jumps_1h, ship_kills_1h, pod_kills_1h, npc_kills_1h,
                    intel_updated_at, effect, status, owner, notes, created_at
              FROM systems ORDER BY created_at, name",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SystemNode {
                    id: row.get(0)?,
                    solar_system_id: row.get::<_, i64>(1)? as u64,
                    name: row.get(2)?,
                    class: row.get(3)?,
                    security_status: row.get(4)?,
                    constellation_id: row.get::<_, i64>(5)? as u64,
                    constellation_name: row.get(6)?,
                    region_id: row.get::<_, i64>(7)? as u64,
                    region_name: row.get(8)?,
                    planet_count: row.get::<_, i64>(9)? as u32,
                    stargate_count: row.get::<_, i64>(10)? as u32,
                    station_count: row.get::<_, i64>(11)? as u32,
                    jumps_1h: row.get::<_, i64>(12)? as u32,
                    ship_kills_1h: row.get::<_, i64>(13)? as u32,
                    pod_kills_1h: row.get::<_, i64>(14)? as u32,
                    npc_kills_1h: row.get::<_, i64>(15)? as u32,
                    intel_updated_at: row.get::<_, i64>(16)? as u64,
                    effect: row.get(17)?,
                    status: row.get(18)?,
                    owner: row.get(19)?,
                    notes: row.get(20)?,
                    created_at: row.get::<_, i64>(21)? as u64,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    fn load_connections(&self, conn: &SqliteConnection) -> Result<Vec<Connection>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, from_system, to_system, signature_from, signature_to,
                    mass_status, life_status, ship_size, notes, created_at
             FROM connections ORDER BY created_at",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Connection {
                    id: row.get(0)?,
                    from_system: row.get(1)?,
                    to_system: row.get(2)?,
                    signature_from: row.get(3)?,
                    signature_to: row.get(4)?,
                    mass_status: row.get(5)?,
                    life_status: row.get(6)?,
                    ship_size: row.get(7)?,
                    notes: row.get(8)?,
                    created_at: row.get::<_, i64>(9)? as u64,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    fn load_signatures(&self, conn: &SqliteConnection) -> Result<Vec<Signature>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, system_id, code, sig_type, target, life, status, notes, created_at
             FROM signatures ORDER BY system_id, code",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Signature {
                    id: row.get(0)?,
                    system_id: row.get(1)?,
                    code: row.get(2)?,
                    sig_type: row.get(3)?,
                    target: row.get(4)?,
                    life: row.get(5)?,
                    status: row.get(6)?,
                    notes: row.get(7)?,
                    created_at: row.get::<_, i64>(8)? as u64,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    fn load_visits(&self, conn: &SqliteConnection) -> Result<Vec<VisitLog>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, system_id, pilot, activity, notes, created_at
             FROM visits ORDER BY created_at DESC",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(VisitLog {
                    id: row.get(0)?,
                    system_id: row.get(1)?,
                    pilot: row.get(2)?,
                    activity: row.get(3)?,
                    notes: row.get(4)?,
                    created_at: row.get::<_, i64>(5)? as u64,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    fn load_notes(&self, conn: &SqliteConnection) -> Result<Vec<Note>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, system_id, title, body, created_at FROM notes ORDER BY created_at DESC",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    system_id: row.get(1)?,
                    title: row.get(2)?,
                    body: row.get(3)?,
                    created_at: row.get::<_, i64>(4)? as u64,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    fn load_system_assessments(
        &self,
        conn: &SqliteConnection,
    ) -> Result<Vec<SystemAssessment>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT system_id, threat_score, activity_score, confidence, evidence,
                        briefing, model_generated, generated_at
                 FROM system_assessments
                 ORDER BY generated_at DESC",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let evidence_json = row.get::<_, String>(4)?;
                let evidence = serde_json::from_str(&evidence_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(4, Type::Text, Box::new(error))
                })?;
                Ok(SystemAssessment {
                    system_id: row.get(0)?,
                    threat_score: row.get::<_, i64>(1)? as u8,
                    activity_score: row.get::<_, i64>(2)? as u8,
                    confidence: row.get(3)?,
                    evidence,
                    briefing: row.get(5)?,
                    model_generated: row.get::<_, i64>(6)? != 0,
                    generated_at: row.get::<_, i64>(7)? as u64,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    fn is_empty(&self, data: &AppData) -> bool {
        data.accounts.is_empty()
            && data.esi_settings.client_id.is_empty()
            && data.systems.is_empty()
            && data.connections.is_empty()
            && data.signatures.is_empty()
            && data.visits.is_empty()
            && data.notes.is_empty()
            && data.system_assessments.is_empty()
    }

    fn import_legacy_json(&self) -> Result<Option<AppData>, String> {
        for dir in self.legacy_dirs.iter() {
            for name in paths::LEGACY_JSON_FILES {
                let legacy_path = dir.join(name);
                if !legacy_path.is_file() {
                    continue;
                }
                let text = fs::read_to_string(&legacy_path).map_err(|error| error.to_string())?;
                let data =
                    serde_json::from_str::<AppData>(&text).map_err(|error| error.to_string())?;
                return Ok(Some(data));
            }
        }

        Ok(None)
    }
}

/// SQLite's `-wal`/`-shm` companion of `database`, without assuming a UTF-8 path.
fn sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    let mut path = database.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

fn traffic_kills(row: &Row<'_>, first_column: usize) -> rusqlite::Result<KillCounts> {
    Ok(KillCounts {
        ship_kills: row.get::<_, i64>(first_column)? as u32,
        pod_kills: row.get::<_, i64>(first_column + 1)? as u32,
        npc_kills: row.get::<_, i64>(first_column + 2)? as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        KILLMAIL_RETENTION_SECS, Store, TRAFFIC_RETENTION_SECS, UMBRA_FEED_MAX_KEPT,
        UMBRA_FEED_RETENTION_SECS,
    };
    use crate::model::{
        AppData, AutoIntelSettings, CachedKillmail, DashboardLayout, KillCounts, KillboardCoverage,
        TrafficHour, TrafficReading, TrafficRows, TrafficSample, UmbraBriefing,
    };
    use rusqlite::Connection as SqliteConnection;
    use std::path::PathBuf;

    const HOUR: u64 = 3_600;
    const NOW: u64 = 1_760_000_000 / HOUR * HOUR + 1_800;

    struct TestDb {
        path: PathBuf,
    }

    impl TestDb {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "nomadeer-store-{}-{name}.sqlite3",
                std::process::id()
            ));
            let db = Self { path };
            db.remove_files();
            db
        }

        fn store(&self) -> Store {
            Store::new(self.path.clone())
        }

        fn remove_files(&self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
            }
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            self.remove_files();
        }
    }

    /// A scratch folder standing in for data folders, removed afterwards.
    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("nomadeer-store-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("test dir should be created");
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn data_with_client_id(client_id: &str) -> AppData {
        let mut data = AppData::default();
        data.esi_settings.client_id = client_id.to_owned();
        data
    }

    #[test]
    fn a_new_database_copies_the_legacy_database_from_an_older_folder() {
        let dir = TestDir::new("legacy-sqlite");
        let data_dir = dir.path.join("data");
        let old_dir = dir.path.join("old");
        Store::new(old_dir.join("eve_wanderer.sqlite3"))
            .save(&data_with_client_id("legacy"))
            .unwrap();

        let store = Store::new(data_dir.join("nomadeer.sqlite3"))
            .with_legacy_dirs(vec![data_dir.clone(), old_dir.clone()]);
        assert_eq!(store.load().unwrap().esi_settings.client_id, "legacy");
        // The old copy is left for older versions.
        assert!(old_dir.join("eve_wanderer.sqlite3").is_file());

        // Once the database exists, the legacy one is no longer consulted.
        store.save(&data_with_client_id("current")).unwrap();
        let reopened =
            Store::new(data_dir.join("nomadeer.sqlite3")).with_legacy_dirs(vec![data_dir, old_dir]);
        assert_eq!(reopened.load().unwrap().esi_settings.client_id, "current");
    }

    #[test]
    fn an_empty_database_imports_legacy_json_preferring_the_data_folder() {
        let dir = TestDir::new("legacy-json");
        let data_dir = dir.path.join("data");
        let old_dir = dir.path.join("old");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&old_dir).unwrap();
        let write = |path: PathBuf, client_id: &str| {
            let json = serde_json::to_string(&data_with_client_id(client_id)).unwrap();
            std::fs::write(path, json).unwrap();
        };
        write(old_dir.join("nomadeer.json"), "old-folder");
        write(data_dir.join("eve_wanderer.json"), "data-folder");

        let store = Store::new(data_dir.join("nomadeer.sqlite3"))
            .with_legacy_dirs(vec![data_dir.clone(), old_dir.clone()]);
        assert_eq!(store.load().unwrap().esi_settings.client_id, "data-folder");

        // Without legacy folders nothing is imported.
        let isolated = Store::new(old_dir.join("nomadeer.sqlite3"));
        assert_eq!(isolated.load().unwrap().esi_settings.client_id, "");
    }

    fn jumps(hour: u64, rows: &[(u64, u32)]) -> TrafficHour {
        TrafficHour {
            hour,
            data_time: hour + 120,
            rows: TrafficRows::Jumps(rows.to_vec()),
        }
    }

    fn kills(hour: u64, rows: &[(u64, u32)]) -> TrafficHour {
        TrafficHour {
            hour,
            data_time: hour + 300,
            rows: TrafficRows::Kills(
                rows.iter()
                    .map(|(system, ship_kills)| {
                        (
                            *system,
                            KillCounts {
                                ship_kills: *ship_kills,
                                pod_kills: 1,
                                npc_kills: 40,
                            },
                        )
                    })
                    .collect(),
            ),
        }
    }

    fn killmail(killmail_id: u64, solar_system_id: u64, killmail_time: u64) -> CachedKillmail {
        CachedKillmail {
            killmail_id,
            solar_system_id,
            killmail_time,
            total_value: 1_000_000.0,
            solo: false,
            npc: false,
            attacker_ids: vec![7],
            corporation_ids: vec![9],
        }
    }

    #[test]
    fn traffic_history_counts_missing_systems_as_zero() {
        let db = TestDb::new("traffic-zero");
        let store = db.store();
        let first = NOW / HOUR * HOUR - 2 * HOUR;
        let second = first + HOUR;
        store
            .record_traffic_hour(&jumps(first, &[(30_000_142, 900), (30_002_187, 40)]), NOW)
            .unwrap();
        store
            .record_traffic_hour(&jumps(second, &[(30_000_142, 950)]), NOW)
            .unwrap();
        store
            .record_traffic_hour(&kills(second, &[(30_002_187, 3)]), NOW)
            .unwrap();

        let history = store.system_traffic(30_002_187, first).unwrap();
        let counts = KillCounts {
            ship_kills: 3,
            pod_kills: 1,
            npc_kills: 40,
        };
        assert_eq!(
            history.samples,
            vec![
                TrafficSample {
                    hour: first,
                    jumps: Some(40),
                    kills: None,
                },
                TrafficSample {
                    hour: second,
                    jumps: Some(0),
                    kills: Some(counts),
                },
            ]
        );
        assert_eq!(
            history.latest_jumps,
            Some(TrafficReading {
                hour: second,
                data_time: second + 120,
                value: 0,
            })
        );
        assert_eq!(
            history.latest_kills,
            Some(TrafficReading {
                hour: second,
                data_time: second + 300,
                value: counts,
            })
        );

        let jita = store.system_traffic(30_000_142, second).unwrap();
        assert_eq!(jita.samples.len(), 1);
        assert_eq!(jita.samples[0].jumps, Some(950));
        assert_eq!(jita.samples[0].kills, Some(KillCounts::default()));
    }

    #[test]
    fn newer_capture_replaces_an_hour_and_older_one_is_ignored() {
        let db = TestDb::new("traffic-replace");
        let store = db.store();
        let hour = NOW / HOUR * HOUR;
        store
            .record_traffic_hour(&jumps(hour, &[(1, 10), (2, 20)]), NOW)
            .unwrap();
        let mut newer = jumps(hour, &[(1, 15)]);
        newer.data_time += 60;
        store.record_traffic_hour(&newer, NOW).unwrap();
        let mut older = jumps(hour, &[(1, 99), (2, 99)]);
        older.data_time -= 60;
        store.record_traffic_hour(&older, NOW).unwrap();

        let one = store.system_traffic(1, hour).unwrap();
        let two = store.system_traffic(2, hour).unwrap();
        assert_eq!(one.samples[0].jumps, Some(15));
        assert_eq!(two.samples[0].jumps, Some(0));
        assert_eq!(one.latest_jumps.unwrap().data_time, newer.data_time);
    }

    #[test]
    fn traffic_capture_waits_for_another_writer() {
        let db = TestDb::new("traffic-busy");
        let store = db.store();
        let hour = NOW / HOUR * HOUR;
        store
            .record_traffic_hour(&jumps(hour, &[(1, 10)]), NOW)
            .unwrap();

        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let path = db.path.clone();
        let writer = std::thread::spawn(move || {
            let mut conn = SqliteConnection::open(path).unwrap();
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            tx.execute(
                "UPDATE traffic_hours SET jumps_data_time = jumps_data_time",
                [],
            )
            .unwrap();
            locked_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            tx.commit().unwrap();
        });
        locked_rx.recv().unwrap();
        store
            .record_traffic_hour(&kills(hour, &[(1, 2)]), NOW)
            .unwrap();
        writer.join().unwrap();

        let history = store.system_traffic(1, hour).unwrap();
        assert_eq!(history.samples[0].kills.unwrap().ship_kills, 2);
    }

    #[test]
    fn traffic_and_killmails_are_pruned_by_age() {
        let db = TestDb::new("prune");
        let store = db.store();
        let current_hour = NOW / HOUR * HOUR;
        let expired_hour = current_hour - TRAFFIC_RETENTION_SECS - HOUR;
        let kept_hour = current_hour - TRAFFIC_RETENTION_SECS;
        store
            .record_traffic_hour(&jumps(expired_hour, &[(1, 5)]), NOW)
            .unwrap();
        store
            .record_traffic_hour(&jumps(kept_hour, &[(1, 6)]), NOW)
            .unwrap();
        let history = store.system_traffic(1, 0).unwrap();
        assert_eq!(history.samples.len(), 1);
        assert_eq!(history.samples[0].hour, kept_hour);

        store
            .store_killmails(
                &[
                    killmail(1, 1, NOW - KILLMAIL_RETENTION_SECS - 1),
                    killmail(2, 1, NOW - 60),
                ],
                Some(&KillboardCoverage {
                    solar_system_id: 1,
                    since: NOW - KILLMAIL_RETENTION_SECS - 2 * HOUR,
                    until: NOW - KILLMAIL_RETENTION_SECS - HOUR,
                }),
                NOW,
            )
            .unwrap();
        assert_eq!(store.killboard_coverage(1).unwrap(), None);
        let ids = store
            .cached_killmails(1, 0)
            .unwrap()
            .into_iter()
            .map(|kill| kill.killmail_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![2]);
    }

    #[test]
    fn killmails_upsert_by_id_and_filter_by_system() {
        let db = TestDb::new("killmails");
        let store = db.store();
        store
            .store_killmails(
                &[
                    killmail(1, 30_000_142, NOW - 600),
                    killmail(2, 30_000_142, NOW - 60),
                    killmail(3, 30_002_187, NOW - 60),
                ],
                None,
                NOW,
            )
            .unwrap();
        let mut updated = killmail(1, 30_000_142, NOW - 600);
        updated.total_value = 5.0;
        updated.attacker_ids = vec![7, 8];
        store
            .store_killmails(std::slice::from_ref(&updated), None, NOW)
            .unwrap();

        let kills = store.cached_killmails(30_000_142, 0).unwrap();
        assert_eq!(
            kills
                .iter()
                .map(|kill| kill.killmail_id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(kills[1], updated);
        assert_eq!(
            store.cached_killmails(30_000_142, NOW - 300).unwrap().len(),
            1
        );
    }

    #[test]
    fn killboard_coverage_is_saved_with_its_killmails() {
        let db = TestDb::new("coverage");
        let store = db.store();
        assert_eq!(store.killboard_coverage(30_000_142).unwrap(), None);
        let first = KillboardCoverage {
            solar_system_id: 30_000_142,
            since: NOW - 6 * HOUR,
            until: NOW - HOUR,
        };
        store
            .store_killmails(
                &[killmail(1, 30_000_142, NOW - 2 * HOUR)],
                Some(&first),
                NOW,
            )
            .unwrap();
        let extended = KillboardCoverage {
            until: NOW,
            ..first
        };
        store
            .store_killmails(&[killmail(2, 30_000_142, NOW - 60)], Some(&extended), NOW)
            .unwrap();

        assert_eq!(
            store.killboard_coverage(30_000_142).unwrap(),
            Some(extended)
        );
        assert_eq!(store.killboard_coverage(30_002_187).unwrap(), None);
        assert_eq!(store.cached_killmails(30_000_142, 0).unwrap().len(), 2);
    }

    #[test]
    fn saving_app_data_keeps_killmails_and_traffic() {
        let db = TestDb::new("save");
        let store = db.store();
        store.load().unwrap();
        store
            .store_killmails(&[killmail(1, 1, NOW - 60)], None, NOW)
            .unwrap();
        store
            .record_traffic_hour(&jumps(NOW / HOUR * HOUR, &[(1, 5)]), NOW)
            .unwrap();
        store.save(&AppData::default()).unwrap();

        assert_eq!(store.cached_killmails(1, 0).unwrap().len(), 1);
        assert_eq!(store.system_traffic(1, 0).unwrap().samples.len(), 1);
    }

    #[test]
    fn auto_intel_settings_default_on_and_persist() {
        let db = TestDb::new("auto-intel");
        let store = db.store();
        // A database saved before these settings existed has no rows for them.
        store.save(&data_with_client_id("older")).unwrap();
        {
            let conn = SqliteConnection::open(&db.path).unwrap();
            conn.execute("DELETE FROM app_settings WHERE key NOT LIKE 'esi_%'", [])
                .unwrap();
        }
        assert_eq!(
            store.load().unwrap().auto_intel,
            AutoIntelSettings::default()
        );
        assert!(AutoIntelSettings::default().gather_on_arrival);
        assert!(AutoIntelSettings::default().umbra_briefs_automatically);

        let mut data = data_with_client_id("older");
        data.auto_intel.gather_on_arrival = false;
        store.save(&data).unwrap();
        let loaded = store.load().unwrap().auto_intel;
        assert!(!loaded.gather_on_arrival);
        assert!(loaded.umbra_briefs_automatically);

        data.auto_intel = AutoIntelSettings {
            gather_on_arrival: true,
            umbra_briefs_automatically: false,
        };
        store.save(&data).unwrap();
        assert_eq!(store.load().unwrap().auto_intel, data.auto_intel);

        // Legacy JSON without the settings turns both on.
        let legacy: AppData = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.auto_intel, AutoIntelSettings::default());
    }

    #[test]
    fn dashboard_layout_defaults_to_a_third_for_tabs_and_persists() {
        let db = TestDb::new("dashboard-layout");
        let store = db.store();
        // A database saved before the split existed has no row for it.
        store.save(&data_with_client_id("older")).unwrap();
        {
            let conn = SqliteConnection::open(&db.path).unwrap();
            conn.execute(
                "DELETE FROM app_settings WHERE key = 'dashboard_tabs_fraction'",
                [],
            )
            .unwrap();
        }
        assert_eq!(
            store.load().unwrap().dashboard_layout,
            DashboardLayout::default()
        );
        assert!((DashboardLayout::default().tabs_fraction - 1.0 / 3.0).abs() < f32::EPSILON);

        let mut data = data_with_client_id("older");
        data.dashboard_layout.tabs_fraction = 0.45;
        store.save(&data).unwrap();
        assert_eq!(store.load().unwrap().dashboard_layout.tabs_fraction, 0.45);

        // A stored share that leaves one side no room falls back to the default.
        for bad in ["0", "1", "-0.2", "NaN", "inf", "wide"] {
            let conn = SqliteConnection::open(&db.path).unwrap();
            conn.execute(
                "UPDATE app_settings SET value = ?1 WHERE key = 'dashboard_tabs_fraction'",
                [bad],
            )
            .unwrap();
            assert_eq!(
                store.load().unwrap().dashboard_layout,
                DashboardLayout::default(),
                "{bad}"
            );
        }

        // Legacy JSON without the split gets the default.
        let legacy: AppData = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.dashboard_layout, DashboardLayout::default());
    }

    #[test]
    fn recon_pilots_survive_the_accounts_migration_and_persist() {
        let db = TestDb::new("recon");
        {
            // Characters saved before recon pilots existed.
            let conn = SqliteConnection::open(&db.path).unwrap();
            conn.execute_batch(
                "CREATE TABLE accounts (
                    id TEXT PRIMARY KEY,
                    character_id TEXT NOT NULL DEFAULT '',
                    character_name TEXT NOT NULL,
                    owner_hash TEXT NOT NULL DEFAULT '',
                    auth_status TEXT NOT NULL DEFAULT 'unauthorized',
                    last_system_id TEXT NOT NULL DEFAULT '',
                    last_system_name TEXT NOT NULL DEFAULT '',
                    access_token TEXT,
                    refresh_token TEXT,
                    token_expires_at INTEGER,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                 );
                 INSERT INTO accounts (id, character_name, created_at, updated_at)
                    VALUES ('acct_1', 'Scout', 1, 1), ('acct_2', 'Hauler', 1, 1);",
            )
            .unwrap();
        }
        let store = db.store();
        let mut data = store.load().unwrap();
        assert_eq!(data.accounts.len(), 2);
        assert!(data.accounts.iter().all(|account| !account.recon));
        assert!(!data.recon_defaults_applied);
        // A second store over the same file re-runs the migration without errors.
        db.store().load().unwrap();

        let scout = data
            .accounts
            .iter_mut()
            .find(|account| account.id == "acct_1")
            .unwrap();
        scout.recon = true;
        data.recon_defaults_applied = true;
        store.save(&data).unwrap();
        let loaded = store.load().unwrap();
        assert!(loaded.recon_defaults_applied);
        let recon: Vec<_> = loaded
            .accounts
            .iter()
            .filter(|account| account.recon)
            .map(|account| account.id.as_str())
            .collect();
        assert_eq!(recon, ["acct_1"]);

        // Legacy JSON characters are not recon pilots until the defaults apply.
        let legacy: AppData = serde_json::from_str(
            r#"{"accounts": [{
                "id": "acct_1", "character_id": "", "character_name": "Scout",
                "owner_hash": "", "auth_status": "authorized", "last_system_id": "",
                "last_system_name": "", "access_token": null, "refresh_token": null,
                "token_expires_at": null, "created_at": 1, "updated_at": 1
            }]}"#,
        )
        .unwrap();
        assert!(!legacy.accounts[0].recon);
        assert!(!legacy.recon_defaults_applied);
    }

    fn umbra_entry(system_id: &str, assessed_at: u64, briefed_at: u64) -> UmbraBriefing {
        UmbraBriefing {
            id: 0,
            solar_system_id: 31_002_238,
            system_id: system_id.to_owned(),
            system_name: "J123456".to_owned(),
            system_class: "C3".to_owned(),
            threat_score: 64,
            activity_score: 22,
            confidence: "Medium".to_owned(),
            briefing: "Threat is Elevated after 3 kills. Activity is Low.".to_owned(),
            assessed_at,
            briefed_at,
            auto: true,
        }
    }

    #[test]
    fn umbra_feed_appends_and_loads_oldest_first() {
        let db = TestDb::new("umbra-feed");
        let store = db.store();
        assert!(store.umbra_briefings(200).unwrap().is_empty());

        let first = store
            .record_umbra_briefing(&umbra_entry("sys_a", NOW - 600, NOW - 500), NOW)
            .unwrap()
            .expect("a new briefing is recorded");
        assert!(first.id > 0);
        let mut asked = umbra_entry("sys_b", NOW - 300, NOW - 200);
        asked.auto = false;
        asked.system_class = "LS 0.3".to_owned();
        store.record_umbra_briefing(&asked, NOW).unwrap();

        let feed = store.umbra_briefings(200).unwrap();
        let systems = feed
            .iter()
            .map(|entry| entry.system_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(systems, vec!["sys_a", "sys_b"]);
        assert_eq!(feed[0], first);
        assert_eq!(
            feed[1],
            UmbraBriefing {
                id: feed[1].id,
                ..asked
            }
        );

        // Only the newest are loaded, still oldest first.
        let newest = store.umbra_briefings(1).unwrap();
        assert_eq!(newest.len(), 1);
        assert_eq!(newest[0].system_id, "sys_b");

        // Saving the map leaves the feed alone.
        store.save(&AppData::default()).unwrap();
        assert_eq!(store.umbra_briefings(200).unwrap().len(), 2);
    }

    #[test]
    fn briefing_the_same_assessment_again_adds_nothing() {
        let db = TestDb::new("umbra-dedupe");
        let store = db.store();
        let entry = umbra_entry("sys_a", NOW - 600, NOW - 500);
        assert!(store.record_umbra_briefing(&entry, NOW).unwrap().is_some());
        let again = UmbraBriefing {
            briefed_at: NOW - 100,
            auto: false,
            ..entry.clone()
        };
        assert_eq!(store.record_umbra_briefing(&again, NOW).unwrap(), None);
        // A newer assessment of the same system is a new entry.
        assert!(
            store
                .record_umbra_briefing(&umbra_entry("sys_a", NOW - 60, NOW - 30), NOW)
                .unwrap()
                .is_some()
        );
        let feed = store.umbra_briefings(200).unwrap();
        assert_eq!(feed.len(), 2);
        assert!(feed[0].auto);
        assert_eq!(feed[0].briefed_at, NOW - 500);
    }

    #[test]
    fn umbra_feed_is_pruned_by_age_and_count() {
        let db = TestDb::new("umbra-prune");
        let store = db.store();
        let expired = NOW - UMBRA_FEED_RETENTION_SECS - 1;
        store
            .record_umbra_briefing(&umbra_entry("sys_old", expired, expired), NOW)
            .unwrap();
        for index in 0..UMBRA_FEED_MAX_KEPT as u64 + 5 {
            let at = NOW - 10_000 + index;
            store
                .record_umbra_briefing(&umbra_entry(&format!("sys_{index}"), at, at), NOW)
                .unwrap();
        }
        let feed = store.umbra_briefings(10_000).unwrap();
        assert_eq!(feed.len(), UMBRA_FEED_MAX_KEPT);
        assert!(feed.iter().all(|entry| entry.system_id != "sys_old"));
        // The oldest beyond the cap went first.
        assert_eq!(feed[0].system_id, "sys_5");
        assert_eq!(
            feed.last().unwrap().system_id,
            format!("sys_{}", UMBRA_FEED_MAX_KEPT + 4)
        );

        store.clear_umbra_briefings().unwrap();
        assert!(store.umbra_briefings(200).unwrap().is_empty());
    }

    #[test]
    fn schema_drops_map_keyed_activity_snapshots() {
        let db = TestDb::new("migrate");
        {
            let conn = SqliteConnection::open(&db.path).unwrap();
            conn.execute_batch(
                "CREATE TABLE system_activity_snapshots (
                    system_id TEXT NOT NULL,
                    captured_at INTEGER NOT NULL,
                    PRIMARY KEY (system_id, captured_at)
                 );
                 INSERT INTO system_activity_snapshots VALUES ('sys_1', 3600);",
            )
            .unwrap();
        }
        db.store().load().unwrap();
        // A second store over the same file re-runs the schema without errors.
        db.store().load().unwrap();

        let conn = SqliteConnection::open(&db.path).unwrap();
        let old_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'system_activity_snapshots'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_tables, 0);
    }
}
