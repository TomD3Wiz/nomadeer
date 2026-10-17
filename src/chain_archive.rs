use crate::model::{Connection, Note, Signature, SystemNode, VisitLog};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChainSnapshot {
    pub(crate) name: String,
    pub(crate) created_at: u64,
    #[serde(default)]
    pub(crate) selected_system_id: String,
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

#[derive(Debug, Clone)]
pub(crate) struct ChainSnapshotMeta {
    pub(crate) file_name: String,
    pub(crate) name: String,
    pub(crate) created_at: u64,
    pub(crate) system_count: usize,
    pub(crate) connection_count: usize,
}

pub(crate) fn list_snapshots(directory: &Path) -> Result<Vec<ChainSnapshotMeta>, String> {
    if !directory.exists() {
        return Ok(Vec::new());
    }

    let mut snapshots = Vec::new();
    let entries = fs::read_dir(directory).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }

        let Ok(snapshot) = load_snapshot_path(&path) else {
            continue;
        };
        let Some(file_name) = path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
        else {
            continue;
        };

        snapshots.push(ChainSnapshotMeta {
            file_name,
            name: snapshot.name,
            created_at: snapshot.created_at,
            system_count: snapshot.systems.len(),
            connection_count: snapshot.connections.len(),
        });
    }

    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.created_at));
    Ok(snapshots)
}

pub(crate) fn save_snapshot(directory: &Path, snapshot: &ChainSnapshot) -> Result<String, String> {
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let file_name = format!(
        "{}_{}.json",
        snapshot.created_at,
        slugify_snapshot_name(&snapshot.name)
    );
    let path = directory.join(&file_name);
    let json = serde_json::to_string_pretty(snapshot).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())?;
    Ok(file_name)
}

pub(crate) fn load_snapshot(directory: &Path, file_name: &str) -> Result<ChainSnapshot, String> {
    let path = safe_snapshot_path(directory, file_name)?;
    load_snapshot_path(&path)
}

pub(crate) fn delete_snapshot(directory: &Path, file_name: &str) -> Result<(), String> {
    let path = safe_snapshot_path(directory, file_name)?;
    fs::remove_file(path).map_err(|error| error.to_string())
}

fn load_snapshot_path(path: &Path) -> Result<ChainSnapshot, String> {
    let json = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&json).map_err(|error| error.to_string())
}

fn safe_snapshot_path(directory: &Path, file_name: &str) -> Result<PathBuf, String> {
    // A Windows drive prefix such as "C:" would make `join` leave the archive.
    if file_name.is_empty() || file_name.contains(['/', '\\', ':']) || file_name.contains("..") {
        return Err("Invalid chain snapshot file".to_owned());
    }
    Ok(directory.join(file_name))
}

fn slugify_snapshot_name(name: &str) -> String {
    let mut slug = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();

    while slug.contains("__") {
        slug = slug.replace("__", "_");
    }
    slug = slug.trim_matches('_').to_owned();

    if slug.is_empty() {
        "chain".to_owned()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::{ChainSnapshot, safe_snapshot_path};
    use crate::model::MapPosition;
    use std::path::Path;

    #[test]
    fn snapshots_keep_hand_placed_systems_and_older_ones_still_load() {
        // Saved before systems could be placed by hand.
        let older: ChainSnapshot = serde_json::from_str(
            r#"{"name": "Home", "created_at": 1, "systems": [{
                "id": "sys_a", "name": "J123456", "class": "C3", "effect": "",
                "status": "home", "owner": "", "notes": "", "created_at": 1
            }]}"#,
        )
        .unwrap();
        assert_eq!(older.systems[0].map_position, None);

        let mut placed = older.clone();
        placed.systems[0].map_position = Some(MapPosition { x: 40.0, y: -12.5 });
        let json = serde_json::to_string(&placed).unwrap();
        let loaded: ChainSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(
            loaded.systems[0].map_position,
            placed.systems[0].map_position
        );
        // Systems without a place write nothing for it, as older versions did.
        assert!(
            !serde_json::to_string(&older)
                .unwrap()
                .contains("map_position")
        );
    }

    #[test]
    fn snapshot_names_stay_inside_the_archive() {
        let archive = Path::new("archive");
        assert_eq!(
            safe_snapshot_path(archive, "1760000000_home_chain.json").unwrap(),
            archive.join("1760000000_home_chain.json")
        );
        for name in [
            "",
            "../outside.json",
            "..",
            "nested/chain.json",
            r"nested\chain.json",
            "C:chain.json",
            r"C:\chain.json",
            "/etc/chain.json",
        ] {
            assert!(safe_snapshot_path(archive, name).is_err(), "{name:?}");
        }
    }
}
