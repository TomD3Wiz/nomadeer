use crate::model::{
    Account, AppData, Connection, EsiSettings, Note, Signature, SystemNode, VisitLog,
};
use rusqlite::{Connection as SqliteConnection, params};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct Store {
    path: PathBuf,
}

impl Store {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn load(&self) -> Result<AppData, String> {
        let conn = self.open()?;
        self.init_schema(&conn)?;
        let mut data = self.load_all(&conn)?;

        if self.is_empty(&data)
            && let Some(imported) = self.import_legacy_json()?
        {
            self.save(&imported)?;
            data = imported;
        }

        Ok(data)
    }

    pub(crate) fn save(&self, data: &AppData) -> Result<(), String> {
        let mut conn = self.open()?;
        self.init_schema(&conn)?;
        let tx = conn.transaction().map_err(|error| error.to_string())?;

        tx.execute_batch(
            "DELETE FROM app_settings;
             DELETE FROM accounts;
             DELETE FROM systems;
             DELETE FROM connections;
             DELETE FROM signatures;
             DELETE FROM visits;
             DELETE FROM notes;",
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

        for account in &data.accounts {
            tx.execute(
                "INSERT INTO accounts (
                    id, character_id, character_name, owner_hash, auth_status,
                    last_system_id, last_system_name, access_token, refresh_token,
                    token_expires_at, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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

        tx.commit().map_err(|error| error.to_string())
    }

    fn open(&self) -> Result<SqliteConnection, String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        self.copy_legacy_sqlite_if_needed()?;
        SqliteConnection::open(&self.path).map_err(|error| error.to_string())
    }

    fn copy_legacy_sqlite_if_needed(&self) -> Result<(), String> {
        let legacy_path = PathBuf::from("data/eve_wanderer.sqlite3");
        if self.path.exists() || self.path == legacy_path || !legacy_path.exists() {
            return Ok(());
        }

        fs::copy(&legacy_path, &self.path).map_err(|error| error.to_string())?;

        for suffix in ["-wal", "-shm"] {
            let legacy_sidecar = PathBuf::from(format!("{}{}", legacy_path.display(), suffix));
            let new_sidecar = PathBuf::from(format!("{}{}", self.path.display(), suffix));
            if legacy_sidecar.exists() {
                fs::copy(legacy_sidecar, new_sidecar).map_err(|error| error.to_string())?;
            }
        }

        Ok(())
    }

    fn init_schema(&self, conn: &SqliteConnection) -> Result<(), String> {
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
                updated_at INTEGER NOT NULL
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
             CREATE INDEX IF NOT EXISTS idx_signatures_system_id ON signatures(system_id);
             CREATE INDEX IF NOT EXISTS idx_visits_system_id ON visits(system_id);
             CREATE INDEX IF NOT EXISTS idx_notes_system_id ON notes(system_id);",
        )
        .map_err(|error| error.to_string())?;

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
        Ok(AppData {
            esi_settings: self.load_esi_settings(conn)?,
            accounts: self.load_accounts(conn)?,
            systems: self.load_systems(conn)?,
            connections: self.load_connections(conn)?,
            signatures: self.load_signatures(conn)?,
            visits: self.load_visits(conn)?,
            notes: self.load_notes(conn)?,
        })
    }

    fn load_esi_settings(&self, conn: &SqliteConnection) -> Result<EsiSettings, String> {
        let mut settings = EsiSettings::default();
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
                "esi_client_id" => settings.client_id = value,
                "esi_callback_port" => {
                    settings.callback_port = value.parse().unwrap_or(settings.callback_port);
                }
                "esi_scopes" => settings.scopes = value,
                _ => {}
            }
        }

        Ok(settings)
    }

    fn load_accounts(&self, conn: &SqliteConnection) -> Result<Vec<Account>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, character_id, character_name, owner_hash, auth_status,
                    last_system_id, last_system_name, access_token, refresh_token,
                    token_expires_at, created_at, updated_at
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

    fn is_empty(&self, data: &AppData) -> bool {
        data.accounts.is_empty()
            && data.esi_settings.client_id.is_empty()
            && data.systems.is_empty()
            && data.connections.is_empty()
            && data.signatures.is_empty()
            && data.visits.is_empty()
            && data.notes.is_empty()
    }

    fn import_legacy_json(&self) -> Result<Option<AppData>, String> {
        for legacy_path in [
            PathBuf::from("data/nomadeer.json"),
            PathBuf::from("data/eve_wanderer.json"),
        ] {
            if legacy_path.exists() {
                let text = fs::read_to_string(&legacy_path).map_err(|error| error.to_string())?;
                let data =
                    serde_json::from_str::<AppData>(&text).map_err(|error| error.to_string())?;
                return Ok(Some(data));
            }
        }

        Ok(None)
    }
}
