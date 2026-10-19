//! Looks up what a pasted local list or d-scan names: pilots with their
//! corporations and alliances, and item types with their groups. Runs on worker
//! threads. Answers are cached in SQLite, so a paste only asks ESI about what
//! Nomadeer has not seen before, and a failed lookup still leaves a usable report.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::esi;
use crate::live_intel::{self, DscanReport, DscanRow, LocalPilot, LocalReport};
use crate::model::{CharacterAffiliation, EveType};
use crate::store::Store;

const CHARACTER: &str = "character";
const INVENTORY_TYPE: &str = "inventory_type";
/// Type and group lookups are one request each, so a first d-scan full of new
/// types runs a few at a time rather than one after another.
const LOOKUP_THREADS: usize = 4;

/// Reads a pasted local list. `own` are the player's characters (name and
/// character ID), which are noted but left out of every count.
pub(crate) fn read_local(
    store: &Store,
    lines: &[String],
    own: &[(String, u64)],
    current_time: u64,
) -> LocalReport {
    let own_names = own
        .iter()
        .map(|(name, _)| name.to_lowercase())
        .collect::<HashSet<_>>();
    let own_ids = own.iter().map(|(_, id)| *id).collect::<HashSet<_>>();
    let (own_here, others): (Vec<_>, Vec<_>) = live_intel::local_names(lines)
        .into_iter()
        .partition(|name| own_names.contains(&name.to_lowercase()));
    let mut report = LocalReport {
        captured_at: current_time,
        pilot_count: others.len(),
        own: own_here,
        ..LocalReport::default()
    };
    let mut errors = Vec::new();

    let mut ids = store
        .cached_ids_by_name(CHARACTER, &others)
        .unwrap_or_default();
    let missing = others
        .iter()
        .filter(|name| !ids.contains_key(&name.to_lowercase()))
        .cloned()
        .collect::<Vec<_>>();
    for batch in missing.chunks(esi::MAX_NAMES_PER_ID_LOOKUP) {
        match esi::universe_ids(batch) {
            Ok(found) => {
                let named = found
                    .characters
                    .into_iter()
                    .map(|entry| (entry.id, entry.name, CHARACTER.to_owned()))
                    .collect::<Vec<_>>();
                let _ = store.store_names(&named, current_time);
                for (id, name, _) in named {
                    ids.insert(name.to_lowercase(), (id, name));
                }
            }
            Err(error) => {
                errors.push(error);
                break;
            }
        }
    }

    let mut pilots = Vec::new();
    for name in &others {
        match ids.get(&name.to_lowercase()) {
            // A character renamed in local's spelling is still the player's own.
            Some((id, _)) if own_ids.contains(id) => {
                report.pilot_count -= 1;
                report.own.push(name.clone());
            }
            Some((id, spelled)) => pilots.push(LocalPilot {
                character_id: *id,
                name: spelled.clone(),
                ..LocalPilot::default()
            }),
            None if errors.is_empty() => report.unidentified.push(name.clone()),
            None => {}
        }
    }

    let character_ids = pilots
        .iter()
        .map(|pilot| pilot.character_id)
        .collect::<Vec<_>>();
    let mut affiliations = store
        .cached_affiliations(&character_ids, current_time)
        .unwrap_or_default();
    let missing = character_ids
        .iter()
        .copied()
        .filter(|id| !affiliations.contains_key(id))
        .collect::<Vec<_>>();
    for batch in missing.chunks(esi::MAX_IDS_PER_AFFILIATION) {
        match esi::character_affiliations(batch) {
            Ok((found, expires_at)) => {
                let _ = store.store_affiliations(&found, expires_at);
                affiliations.extend(found.into_iter().map(|found| (found.character_id, found)));
            }
            Err(error) => {
                errors.push(error);
                break;
            }
        }
    }

    let group_ids = affiliations
        .values()
        .flat_map(|affiliation| [affiliation.corporation_id, affiliation.alliance_id])
        .filter(|id| *id != 0)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut names = store.cached_names(&group_ids).unwrap_or_default();
    let missing = group_ids
        .iter()
        .copied()
        .filter(|id| !names.contains_key(id))
        .collect::<Vec<_>>();
    for batch in missing.chunks(esi::MAX_IDS_PER_NAME_LOOKUP) {
        match esi::universe_names(batch) {
            Ok(found) => {
                let named = found
                    .into_iter()
                    .map(|entry| (entry.id, entry.name, entry.category))
                    .collect::<Vec<_>>();
                let _ = store.store_names(&named, current_time);
                names.extend(named.into_iter().map(|(id, name, _)| (id, name)));
            }
            Err(error) => {
                errors.push(error);
                break;
            }
        }
    }

    for pilot in &mut pilots {
        if let Some(affiliation) = affiliations.get(&pilot.character_id) {
            affiliate(pilot, affiliation, &names);
        }
    }
    report.pilots = pilots;
    report.lookup_error = errors.join("; ");
    report
}

fn affiliate(
    pilot: &mut LocalPilot,
    affiliation: &CharacterAffiliation,
    names: &HashMap<u64, String>,
) {
    let name = |id: u64| names.get(&id).cloned().unwrap_or_default();
    pilot.corporation_id = affiliation.corporation_id;
    pilot.corporation = name(affiliation.corporation_id);
    pilot.alliance_id = affiliation.alliance_id;
    pilot.alliance = if affiliation.alliance_id == 0 {
        String::new()
    } else {
        name(affiliation.alliance_id)
    };
}

/// Reads a pasted d-scan, looking up each type's group. Rows without a type ID are
/// matched by type name.
pub(crate) fn read_dscan(store: &Store, rows: &[DscanRow], current_time: u64) -> DscanReport {
    let mut errors = Vec::new();

    let unnamed = rows
        .iter()
        .filter(|row| row.type_id.is_none() && !row.type_name.is_empty())
        .map(|row| row.type_name.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut ids_by_name = store
        .cached_ids_by_name(INVENTORY_TYPE, &unnamed)
        .unwrap_or_default()
        .into_iter()
        .map(|(name, (id, _))| (name, id as u32))
        .collect::<HashMap<_, _>>();
    let missing = unnamed
        .iter()
        .filter(|name| !ids_by_name.contains_key(&name.to_lowercase()))
        .cloned()
        .collect::<Vec<_>>();
    for batch in missing.chunks(esi::MAX_NAMES_PER_ID_LOOKUP) {
        match esi::universe_ids(batch) {
            Ok(found) => {
                let named = found
                    .inventory_types
                    .into_iter()
                    .map(|entry| (entry.id, entry.name, INVENTORY_TYPE.to_owned()))
                    .collect::<Vec<_>>();
                let _ = store.store_names(&named, current_time);
                for (id, name, _) in named {
                    ids_by_name.insert(name.to_lowercase(), id as u32);
                }
            }
            Err(error) => {
                errors.push(error);
                break;
            }
        }
    }

    let type_ids = rows
        .iter()
        .filter_map(|row| {
            row.type_id
                .or_else(|| ids_by_name.get(&row.type_name.to_lowercase()).copied())
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut types = store.cached_types(&type_ids).unwrap_or_default();
    let missing = type_ids
        .iter()
        .copied()
        .filter(|id| !types.contains_key(id))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let (found, error) = look_up_types(store, &missing);
        let _ = store.store_types(&found, current_time);
        types.extend(found.into_iter().map(|info| (info.type_id, info)));
        errors.extend(error);
    }

    let mut report = live_intel::summarise_dscan(rows, &types, &ids_by_name, current_time);
    report.lookup_error = errors.join("; ");
    report
}

/// Looks up types and then any groups not yet cached, a few at a time. Returns what
/// was found and the first error, if any.
fn look_up_types(store: &Store, type_ids: &[u32]) -> (Vec<EveType>, Option<String>) {
    let type_results = in_parallel(type_ids, |type_id| esi::type_and_group(*type_id));
    let mut first_error = None;
    let mut named_types = Vec::new();
    for (type_id, result) in type_ids.iter().zip(type_results) {
        match result {
            Ok((name, group_id)) => named_types.push((*type_id, name, group_id)),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }

    let group_ids = named_types
        .iter()
        .map(|(_, _, group_id)| *group_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut groups = store.cached_groups(&group_ids).unwrap_or_default();
    let missing = group_ids
        .iter()
        .copied()
        .filter(|id| !groups.contains_key(id))
        .collect::<Vec<_>>();
    let group_results = in_parallel(&missing, |group_id| esi::group_and_category(*group_id));
    for (group_id, result) in missing.iter().zip(group_results) {
        match result {
            Ok(group) => {
                groups.insert(*group_id, group);
            }
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }

    let found = named_types
        .into_iter()
        .filter_map(|(type_id, name, group_id)| {
            let (group_name, category_id) = groups.get(&group_id)?.clone();
            Some(EveType {
                type_id,
                name,
                group_id,
                group_name,
                category_id,
            })
        })
        .collect();
    (found, first_error)
}

/// Runs `lookup` on every item with a few threads, keeping the results in order.
fn in_parallel<T, R>(items: &[T], lookup: impl Fn(&T) -> R + Sync) -> Vec<R>
where
    T: Sync,
    R: Send,
{
    let next = AtomicUsize::new(0);
    let results = Mutex::new((0..items.len()).map(|_| None).collect::<Vec<Option<R>>>());
    thread::scope(|scope| {
        for _ in 0..LOOKUP_THREADS.min(items.len()) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else {
                        break;
                    };
                    let result = lookup(item);
                    if let Ok(mut results) = results.lock() {
                        results[index] = Some(result);
                    }
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .map(|result| result.expect("every item is looked up once"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{affiliate, in_parallel};
    use crate::live_intel::LocalPilot;
    use crate::model::CharacterAffiliation;

    #[test]
    fn parallel_lookups_keep_their_order() {
        let items = (0..37).collect::<Vec<u32>>();
        assert_eq!(
            in_parallel(&items, |item| item * 2),
            (0..37).map(|item| item * 2).collect::<Vec<_>>()
        );
        assert!(in_parallel(&[] as &[u32], |item| *item).is_empty());
    }

    #[test]
    fn pilots_take_their_corporation_and_alliance_names() {
        let names = HashMap::from([
            (98_000_001, "Some Corp".to_owned()),
            (99_000_001, "Some Alliance".to_owned()),
        ]);
        let mut pilot = LocalPilot {
            character_id: 1,
            name: "Some Pilot".to_owned(),
            ..LocalPilot::default()
        };
        affiliate(
            &mut pilot,
            &CharacterAffiliation {
                character_id: 1,
                corporation_id: 98_000_001,
                alliance_id: 99_000_001,
            },
            &names,
        );
        assert_eq!(pilot.corporation, "Some Corp");
        assert_eq!(pilot.alliance, "Some Alliance");
        affiliate(
            &mut pilot,
            &CharacterAffiliation {
                character_id: 1,
                corporation_id: 98_000_002,
                alliance_id: 0,
            },
            &names,
        );
        assert_eq!(pilot.corporation_id, 98_000_002);
        assert_eq!(pilot.corporation, "");
        assert_eq!(pilot.alliance_id, 0);
        assert_eq!(pilot.alliance, "");
    }
}
