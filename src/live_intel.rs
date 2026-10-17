//! Live intel pasted from the game: the local member list and the directional
//! scanner. Each paste is summarised for the system it was taken in and, for
//! [`LIVE_INTEL_FRESH_SECS`] after that, adds plain-English evidence and a modest
//! threat and activity contribution to the system's assessment. Older pastes stay
//! on show as earlier intel but no longer count.
//!
//! Everything here is pure; `eve_lookup` resolves names and types through ESI.

use crate::local_intel::{count_label, join_phrases};
use crate::model::{CachedKillmail, EveType};
use crate::probe_scan;
use crate::util::DAY_SECS;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// How long a paste shapes the assessment; after that it is earlier intel.
pub(crate) const LIVE_INTEL_FRESH_SECS: u64 = 15 * 60;
/// Earlier intel is kept this long, for the sidebar only.
pub(crate) const LIVE_INTEL_RETENTION_SECS: u64 = DAY_SECS;
/// Grids are usually a few thousand km across, so anything this close is taken to
/// be on grid with the scanning ship.
pub(crate) const ON_GRID_KM: f64 = 10_000.0;
/// NPC corporations, such as the starter corporations many pilots sit in, say
/// nothing about who someone flies with.
const NPC_CORPORATION_IDS: std::ops::RangeInclusive<u64> = 1_000_000..=1_999_999;

// Live intel never decides an assessment on its own: its threat and activity are
// capped at these, on top of the killboard and traffic.
const LIVE_THREAT_CAP: f32 = 50.0;
const LIVE_ACTIVITY_CAP: f32 = 40.0;

/// Whether a paste captured at `captured_at` still counts towards the assessment.
pub(crate) fn is_fresh(captured_at: u64, current_time: u64) -> bool {
    current_time.saturating_sub(captured_at) < LIVE_INTEL_FRESH_SECS
}

// ---------------------------------------------------------------------------------
// Local

/// Pilot names from a local paste, trimmed and in order, each once.
pub(crate) fn local_names(lines: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && seen.insert(line.to_lowercase()))
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct LocalPilot {
    pub(crate) character_id: u64,
    pub(crate) name: String,
    /// 0 when ESI could not say.
    pub(crate) corporation_id: u64,
    pub(crate) corporation: String,
    /// 0 when the pilot's corporation is in no alliance.
    pub(crate) alliance_id: u64,
    pub(crate) alliance: String,
}

/// What a pasted local member list showed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct LocalReport {
    pub(crate) captured_at: u64,
    /// Every pilot in local other than the player's own characters.
    pub(crate) pilot_count: usize,
    /// The player's own characters seen in local, left out of every count.
    pub(crate) own: Vec<String>,
    /// Pilots ESI identified, with their corporation and alliance where known.
    pub(crate) pilots: Vec<LocalPilot>,
    /// Names ESI did not recognise.
    pub(crate) unidentified: Vec<String>,
    /// Why some pilots could not be looked up; empty when every lookup worked.
    pub(crate) lookup_error: String,
}

/// An alliance, or a corporation outside any alliance, and how many of its pilots
/// are in local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalGroup {
    pub(crate) name: String,
    pub(crate) alliance: bool,
    pub(crate) count: usize,
}

/// Local's pilots by alliance, or by corporation for pilots in none; largest first.
pub(crate) fn local_groups(report: &LocalReport) -> Vec<LocalGroup> {
    let mut groups = HashMap::<(bool, u64), LocalGroup>::new();
    for pilot in &report.pilots {
        let (alliance, id, name) = if pilot.alliance_id != 0 {
            (true, pilot.alliance_id, &pilot.alliance)
        } else if pilot.corporation_id != 0 {
            (false, pilot.corporation_id, &pilot.corporation)
        } else {
            continue;
        };
        let name = if name.is_empty() {
            if alliance {
                "an unnamed alliance"
            } else {
                "an unnamed corporation"
            }
        } else {
            name
        };
        groups
            .entry((alliance, id))
            .or_insert_with(|| LocalGroup {
                name: name.to_owned(),
                alliance,
                count: 0,
            })
            .count += 1;
    }
    let mut groups = groups.into_values().collect::<Vec<_>>();
    groups.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    groups
}

/// How many of local's pilots, and of their player corporations, were attackers on
/// kills in the system.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct KillerOverlap {
    pub(crate) pilots_24h: usize,
    pub(crate) pilots_7d: usize,
    pub(crate) corporations_24h: usize,
    pub(crate) corporations_7d: usize,
}

/// Lines local up against the system's cached killmails. NPC kills and NPC
/// corporations are left out.
pub(crate) fn killer_overlap(
    report: &LocalReport,
    killmails: &[CachedKillmail],
    current_time: u64,
) -> KillerOverlap {
    let pilots = report
        .pilots
        .iter()
        .map(|pilot| pilot.character_id)
        .filter(|id| *id != 0)
        .collect::<HashSet<_>>();
    let corporations = report
        .pilots
        .iter()
        .map(|pilot| pilot.corporation_id)
        .filter(|id| *id != 0 && !NPC_CORPORATION_IDS.contains(id))
        .collect::<HashSet<_>>();
    let mut pilots_24h = HashSet::new();
    let mut pilots_7d = HashSet::new();
    let mut corporations_24h = HashSet::new();
    let mut corporations_7d = HashSet::new();
    for kill in killmails.iter().filter(|kill| !kill.npc) {
        let age = current_time.saturating_sub(kill.killmail_time);
        if age > 7 * DAY_SECS {
            continue;
        }
        for id in kill.attacker_ids.iter().filter(|id| pilots.contains(id)) {
            pilots_7d.insert(*id);
            if age <= DAY_SECS {
                pilots_24h.insert(*id);
            }
        }
        for id in kill
            .corporation_ids
            .iter()
            .filter(|id| corporations.contains(id))
        {
            corporations_7d.insert(*id);
            if age <= DAY_SECS {
                corporations_24h.insert(*id);
            }
        }
    }
    KillerOverlap {
        pilots_24h: pilots_24h.len(),
        pilots_7d: pilots_7d.len(),
        corporations_24h: corporations_24h.len(),
        corporations_7d: corporations_7d.len(),
    }
}

// ---------------------------------------------------------------------------------
// Directional scanner

/// One row of a d-scan. The client copies "type ID, name, type, distance", e.g.
/// "11182\tCheetah\tCheetah\t1,234 km"; older clients copied the last three only.
/// The distance is "-" when the scanner gives none.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DscanRow {
    pub(crate) type_id: Option<u32>,
    pub(crate) name: String,
    pub(crate) type_name: String,
    /// None for "-".
    pub(crate) distance_km: Option<f64>,
}

impl DscanRow {
    pub(crate) fn on_grid(&self) -> bool {
        self.distance_km.is_some_and(|km| km <= ON_GRID_KM)
    }
}

/// Reads a d-scan row, or None for anything else. A row starting with a type ID
/// has three or four cells; one without has exactly three, the last a distance.
pub(crate) fn parse_dscan_row(line: &str) -> Option<DscanRow> {
    let cells = line.trim().split('\t').map(str::trim).collect::<Vec<_>>();
    let first = cells[0];
    if !first.is_empty() && first.chars().all(|character| character.is_ascii_digit()) {
        if !(3..=4).contains(&cells.len()) {
            return None;
        }
        let distance_km = match cells.get(3) {
            Some(cell) if !cell.is_empty() => distance(cell)?,
            _ => None,
        };
        return Some(DscanRow {
            type_id: Some(first.parse().ok()?),
            name: cells[1].to_owned(),
            type_name: cells[2].to_owned(),
            distance_km,
        });
    }
    if cells.len() != 3 || cells[1].is_empty() {
        return None;
    }
    Some(DscanRow {
        type_id: None,
        name: cells[0].to_owned(),
        type_name: cells[1].to_owned(),
        distance_km: distance(cells[2])?,
    })
}

/// "-" (no distance) or a distance in any unit the client writes; None for neither.
fn distance(cell: &str) -> Option<Option<f64>> {
    if cell == "-" {
        return Some(None);
    }
    probe_scan::parse_distance_km(cell).map(Some)
}

// Group IDs, checked against ESI's /universe/groups/ (2026-10).
const GROUP_CAPSULE: u32 = 29;
const GROUP_INTERDICTOR: u32 = 541;
const GROUP_HEAVY_INTERDICTOR: u32 = 894;
const GROUP_COMBAT_RECON: u32 = 906;
const GROUP_FORCE_RECON: u32 = 833;
const GROUP_COVERT_OPS: u32 = 830;
const GROUP_STEALTH_BOMBER: u32 = 834;
const GROUP_STRATEGIC_CRUISER: u32 = 963;
const GROUP_TACTICAL_DESTROYER: u32 = 1305;
/// Titan, Dreadnought, Carrier, Supercarrier, Capital Industrial Ship, Force
/// Auxiliary and Lancer Dreadnought.
const CAPITAL_GROUPS: &[u32] = &[30, 485, 547, 659, 883, 1538, 4594];
const GROUP_MOBILE_WARP_DISRUPTOR: u32 = 361;
const GROUP_SCANNER_PROBE: u32 = 479;
/// Combat Scanner Probe I, Sisters Combat Scanner Probe and Satori-Horigu Combat
/// Scanner Probe; core probes share their group.
const COMBAT_PROBE_TYPES: &[u32] = &[30028, 30486, 85747];
const CATEGORY_SHIP: u32 = 6;

/// What on a d-scan is worth calling out to a pilot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThreatClass {
    HeavyInterdictor,
    Interdictor,
    Bubble,
    CombatProbe,
    CombatRecon,
    ForceRecon,
    StealthBomber,
    CovertOps,
    StrategicCruiser,
    TacticalDestroyer,
    Capital,
}

impl ThreatClass {
    pub(crate) fn of(info: &EveType) -> Option<Self> {
        Some(match info.group_id {
            GROUP_HEAVY_INTERDICTOR => Self::HeavyInterdictor,
            GROUP_INTERDICTOR => Self::Interdictor,
            GROUP_MOBILE_WARP_DISRUPTOR => Self::Bubble,
            GROUP_COMBAT_RECON => Self::CombatRecon,
            GROUP_FORCE_RECON => Self::ForceRecon,
            GROUP_STEALTH_BOMBER => Self::StealthBomber,
            GROUP_COVERT_OPS => Self::CovertOps,
            GROUP_STRATEGIC_CRUISER => Self::StrategicCruiser,
            GROUP_TACTICAL_DESTROYER => Self::TacticalDestroyer,
            group if CAPITAL_GROUPS.contains(&group) => Self::Capital,
            GROUP_SCANNER_PROBE
                if COMBAT_PROBE_TYPES.contains(&info.type_id)
                    || info.name.to_lowercase().contains("combat") =>
            {
                Self::CombatProbe
            }
            _ => return None,
        })
    }

    /// "2 interdictors", "1 heavy interdictor".
    pub(crate) fn count_text(self, count: usize) -> String {
        let (one, many) = match self {
            Self::HeavyInterdictor => ("heavy interdictor", "heavy interdictors"),
            Self::Interdictor => ("interdictor", "interdictors"),
            Self::Bubble => ("warp disruption bubble", "warp disruption bubbles"),
            Self::CombatProbe => ("combat scanner probe", "combat scanner probes"),
            Self::CombatRecon => ("combat recon", "combat recons"),
            Self::ForceRecon => ("force recon", "force recons"),
            Self::StealthBomber => ("stealth bomber", "stealth bombers"),
            Self::CovertOps => ("covert ops frigate", "covert ops frigates"),
            Self::StrategicCruiser => ("strategic cruiser", "strategic cruisers"),
            Self::TacticalDestroyer => ("tactical destroyer", "tactical destroyers"),
            Self::Capital => ("capital", "capitals"),
        };
        format!("{count} {}", if count == 1 { one } else { many })
    }

    /// Threat each one adds, and the most the class can add together. Ships that
    /// hold a pilot in place or hunt from cloak weigh most; probes count once.
    fn weight(self) -> (f32, f32) {
        match self {
            Self::HeavyInterdictor => (10.0, 20.0),
            Self::Interdictor | Self::Bubble => (8.0, 16.0),
            Self::CombatProbe => (10.0, 10.0),
            Self::CombatRecon | Self::ForceRecon => (6.0, 12.0),
            Self::Capital => (6.0, 18.0),
            Self::StealthBomber | Self::StrategicCruiser => (5.0, 15.0),
            Self::CovertOps | Self::TacticalDestroyer => (3.0, 9.0),
        }
    }
}

/// One kind of thing on a d-scan: a group such as "Heavy Assault Cruiser", or
/// combat probes apart from core probes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DscanEntry {
    pub(crate) label: String,
    /// A ship with a pilot in it, capsules aside.
    pub(crate) ship: bool,
    pub(crate) threat: Option<ThreatClass>,
    pub(crate) count: usize,
    pub(crate) on_grid: usize,
}

/// What a pasted d-scan showed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DscanReport {
    pub(crate) captured_at: u64,
    pub(crate) rows: usize,
    /// Ships first, then the most numerous.
    pub(crate) entries: Vec<DscanEntry>,
    /// Rows whose type could not be looked up.
    pub(crate) unidentified: usize,
    /// Why some types could not be looked up; empty when every lookup worked.
    pub(crate) lookup_error: String,
}

impl DscanReport {
    pub(crate) fn ships(&self) -> usize {
        self.ship_entries().map(|entry| entry.count).sum()
    }

    pub(crate) fn ships_on_grid(&self) -> usize {
        self.ship_entries().map(|entry| entry.on_grid).sum()
    }

    pub(crate) fn ship_entries(&self) -> impl Iterator<Item = &DscanEntry> {
        self.entries.iter().filter(|entry| entry.ship)
    }

    /// Each threat class seen, with how many and how many on grid, worst first.
    pub(crate) fn threats(&self) -> Vec<(ThreatClass, usize, usize)> {
        let mut threats = Vec::<(ThreatClass, usize, usize)>::new();
        for entry in &self.entries {
            let Some(class) = entry.threat else {
                continue;
            };
            match threats.iter_mut().find(|(known, _, _)| *known == class) {
                Some((_, count, on_grid)) => {
                    *count += entry.count;
                    *on_grid += entry.on_grid;
                }
                None => threats.push((class, entry.count, entry.on_grid)),
            }
        }
        threats.sort_by_key(|(class, _, _)| *class);
        threats
    }
}

/// Sorts a d-scan's rows by group. `types` holds what is known about each type ID,
/// and `ids_by_name` the type IDs of type names, lowercased, for rows without one.
pub(crate) fn summarise_dscan(
    rows: &[DscanRow],
    types: &HashMap<u32, EveType>,
    ids_by_name: &HashMap<String, u32>,
    captured_at: u64,
) -> DscanReport {
    let mut report = DscanReport {
        captured_at,
        rows: rows.len(),
        ..DscanReport::default()
    };
    for row in rows {
        let info = row
            .type_id
            .or_else(|| ids_by_name.get(&row.type_name.to_lowercase()).copied())
            .and_then(|type_id| types.get(&type_id));
        let Some(info) = info else {
            report.unidentified += 1;
            continue;
        };
        let threat = ThreatClass::of(info);
        let label = if threat == Some(ThreatClass::CombatProbe) {
            "Combat Scanner Probe"
        } else {
            &info.group_name
        };
        let ship = info.category_id == CATEGORY_SHIP && info.group_id != GROUP_CAPSULE;
        let index = match report
            .entries
            .iter()
            .position(|entry| entry.label == label && entry.threat == threat)
        {
            Some(index) => index,
            None => {
                report.entries.push(DscanEntry {
                    label: label.to_owned(),
                    ship,
                    threat,
                    ..DscanEntry::default()
                });
                report.entries.len() - 1
            }
        };
        let entry = &mut report.entries[index];
        entry.count += 1;
        entry.on_grid += usize::from(row.on_grid());
    }
    report.entries.sort_by(|a, b| {
        b.ship
            .cmp(&a.ship)
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| a.label.cmp(&b.label))
    });
    report
}

/// "3 Strategic Cruisers", "2 Covert Ops", "1 Force Auxiliary", "4 Force Auxiliaries".
pub(crate) fn group_count_text(label: &str, count: usize) -> String {
    let plural = if count == 1 || label.ends_with("Ops") || label.ends_with('s') {
        label.to_owned()
    } else if let Some(stem) = label.strip_suffix('y')
        && !stem.ends_with(['a', 'e', 'i', 'o', 'u'])
    {
        format!("{stem}ies")
    } else {
        format!("{label}s")
    };
    format!("{count} {plural}")
}

// ---------------------------------------------------------------------------------
// The assessment

/// A system's latest local and d-scan pastes.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SystemLiveIntel {
    pub(crate) local: Option<LocalReport>,
    pub(crate) dscan: Option<DscanReport>,
}

impl SystemLiveIntel {
    pub(crate) fn fresh_local(&self, current_time: u64) -> Option<&LocalReport> {
        self.local
            .as_ref()
            .filter(|report| is_fresh(report.captured_at, current_time))
    }

    pub(crate) fn fresh_dscan(&self, current_time: u64) -> Option<&DscanReport> {
        self.dscan
            .as_ref()
            .filter(|report| is_fresh(report.captured_at, current_time))
    }

    /// When the fresh intel stops counting: the earliest paste to expire.
    pub(crate) fn expires_at(&self, current_time: u64) -> Option<u64> {
        let local = self
            .fresh_local(current_time)
            .map(|report| report.captured_at);
        let dscan = self
            .fresh_dscan(current_time)
            .map(|report| report.captured_at);
        local
            .into_iter()
            .chain(dscan)
            .min()
            .map(|captured_at| captured_at + LIVE_INTEL_FRESH_SECS)
    }
}

/// What fresh live intel adds to a system's assessment.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LiveContribution {
    pub(crate) threat: f32,
    pub(crate) activity: f32,
    pub(crate) evidence: Vec<String>,
}

impl LiveContribution {
    pub(crate) fn is_live(&self) -> bool {
        !self.evidence.is_empty()
    }
}

/// Evidence, threat and activity from a system's fresh pastes. `killmails` are the
/// system's cached kills, None when its killboard has not been read.
///
/// Scoring, all of it capped at 50 threat and 40 activity together:
/// - Pilots in local besides the player's: threat 3 × √pilots up to 12, activity
///   6 × √pilots up to 25.
/// - Local pilots who killed here in the last day: threat 8 each up to 24; earlier
///   in the week: 3 each up to 9. Their corporations' kills here in the last day:
///   2 each up to 6.
/// - Ships on d-scan: threat 2 × √ships up to 10, activity 6 × √ships up to 30, and
///   ships on grid 2 threat each up to 10.
/// - Each threat class on d-scan by [`ThreatClass::weight`]: heavy interdictors 10
///   each up to 20; interdictors and bubbles 8 up to 16; combat probes 10 once;
///   recons 6 up to 12; capitals 6 up to 18; bombers and strategic cruisers 5 up to
///   15; covert ops and tactical destroyers 3 up to 9.
pub(crate) fn contribution(
    intel: &SystemLiveIntel,
    killmails: Option<&[CachedKillmail]>,
    current_time: u64,
) -> LiveContribution {
    let mut live = LiveContribution::default();
    if let Some(local) = intel.fresh_local(current_time) {
        add_local(&mut live, local, killmails, current_time);
    }
    if let Some(dscan) = intel.fresh_dscan(current_time) {
        add_dscan(&mut live, dscan);
    }
    live.threat = live.threat.min(LIVE_THREAT_CAP);
    live.activity = live.activity.min(LIVE_ACTIVITY_CAP);
    live
}

fn add_local(
    live: &mut LiveContribution,
    report: &LocalReport,
    killmails: Option<&[CachedKillmail]>,
    current_time: u64,
) {
    let pilots = report.pilot_count;
    let own = report.own.len();
    live.evidence.push(match (pilots, own) {
        (0, _) => format!(
            "Local shows no pilots besides your {}",
            count_label(own, "character")
        ),
        (pilots, 0) => format!("Local shows {}", count_label(pilots, "pilot")),
        (pilots, own) => format!(
            "Local shows {} besides your {}",
            count_label(pilots, "pilot"),
            count_label(own, "character")
        ),
    });
    let root = (pilots as f32).sqrt();
    live.threat += (root * 3.0).min(12.0);
    live.activity += (root * 6.0).min(25.0);

    let groups = local_groups(report);
    if !groups.is_empty() {
        let shown = groups
            .iter()
            .take(3)
            .map(|group| format!("{} from {}", group.count, group.name))
            .collect::<Vec<_>>();
        live.evidence.push(format!(
            "{} in local: {}",
            if shown.len() == 1 {
                "Largest group"
            } else {
                "Largest groups"
            },
            join_phrases(&shown)
        ));
    }
    if !report.unidentified.is_empty() {
        live.evidence.push(format!(
            "{} in local could not be identified",
            count_label(report.unidentified.len(), "name")
        ));
    }

    let Some(killmails) = killmails else {
        return;
    };
    if report.pilots.is_empty() {
        return;
    }
    let overlap = killer_overlap(report, killmails, current_time);
    if overlap.pilots_24h > 0 {
        live.evidence.push(format!(
            "{} in local got kills here in the last 24 hours",
            count_label(overlap.pilots_24h, "pilot")
        ));
    }
    if overlap.pilots_7d > overlap.pilots_24h {
        live.evidence.push(format!(
            "{} in local got kills here in the last 7 days",
            count_label(overlap.pilots_7d, "pilot")
        ));
    }
    if overlap.pilots_7d == 0 {
        if overlap.corporations_24h > 0 {
            live.evidence.push(format!(
                "{} in local got kills here in the last 24 hours",
                count_label(overlap.corporations_24h, "corporation")
            ));
        } else if overlap.corporations_7d > 0 {
            live.evidence.push(format!(
                "{} in local got kills here in the last 7 days",
                count_label(overlap.corporations_7d, "corporation")
            ));
        } else {
            live.evidence
                .push("No one in local got kills here in the last 7 days".to_owned());
        }
    }
    live.threat += (overlap.pilots_24h as f32 * 8.0).min(24.0)
        + ((overlap.pilots_7d - overlap.pilots_24h) as f32 * 3.0).min(9.0)
        + (overlap.corporations_24h as f32 * 2.0).min(6.0);
}

fn add_dscan(live: &mut LiveContribution, report: &DscanReport) {
    let ships = report.ships();
    let on_grid = report.ships_on_grid();
    live.evidence.push(match (ships, on_grid) {
        (0, _) => "D-scan shows no ships".to_owned(),
        (ships, 0) => format!("D-scan shows {}, none on grid", count_label(ships, "ship")),
        (ships, on_grid) => format!(
            "D-scan shows {}, {on_grid} on grid",
            count_label(ships, "ship")
        ),
    });
    let root = (ships as f32).sqrt();
    live.threat += (root * 2.0).min(10.0) + (on_grid as f32 * 2.0).min(10.0);
    live.activity += (root * 6.0).min(30.0);

    let classes = report
        .ship_entries()
        .take(4)
        .map(|entry| group_count_text(&entry.label, entry.count))
        .collect::<Vec<_>>();
    if !classes.is_empty() {
        live.evidence
            .push(format!("Ships on d-scan: {}", join_phrases(&classes)));
    }

    let threats = report.threats();
    if !threats.is_empty() {
        let callouts = threats
            .iter()
            .map(|(class, count, on_grid)| {
                let text = class.count_text(*count);
                if *on_grid > 0 {
                    format!("{text} ({on_grid} on grid)")
                } else {
                    text
                }
            })
            .collect::<Vec<_>>();
        live.evidence
            .push(format!("D-scan picked up {}", join_phrases(&callouts)));
    }
    for (class, count, _) in threats {
        let (each, most) = class.weight();
        live.threat += (count as f32 * each).min(most);
    }
    if report.unidentified > 0 {
        live.evidence.push(format!(
            "{} on d-scan could not be identified",
            count_label(report.unidentified, "object")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DscanRow, KillerOverlap, LIVE_INTEL_FRESH_SECS, LocalPilot, LocalReport, SystemLiveIntel,
        ThreatClass, contribution, group_count_text, killer_overlap, local_groups, local_names,
        parse_dscan_row, summarise_dscan,
    };
    use crate::model::{CachedKillmail, EveType};
    use crate::util::{DAY_SECS, HOUR_SECS};
    use std::collections::HashMap;

    const NOW: u64 = 1_000 * DAY_SECS;

    fn pilot(character_id: u64, corporation_id: u64, alliance_id: u64) -> LocalPilot {
        LocalPilot {
            character_id,
            name: format!("Pilot {character_id}"),
            corporation_id,
            corporation: format!("Corp {corporation_id}"),
            alliance_id,
            alliance: if alliance_id == 0 {
                String::new()
            } else {
                format!("Alliance {alliance_id}")
            },
        }
    }

    fn local(pilots: Vec<LocalPilot>) -> LocalReport {
        LocalReport {
            captured_at: NOW - 60,
            pilot_count: pilots.len(),
            pilots,
            ..LocalReport::default()
        }
    }

    fn kill(age: u64, attackers: &[u64], corporations: &[u64], npc: bool) -> CachedKillmail {
        CachedKillmail {
            killmail_id: age,
            solar_system_id: 31_000_001,
            killmail_time: NOW - age,
            total_value: 0.0,
            solo: false,
            npc,
            attacker_ids: attackers.to_vec(),
            corporation_ids: corporations.to_vec(),
        }
    }

    fn eve_type(type_id: u32, name: &str, group_id: u32, group_name: &str) -> EveType {
        EveType {
            type_id,
            name: name.to_owned(),
            group_id,
            group_name: group_name.to_owned(),
            category_id: match group_id {
                361 => 22,
                479 => 8,
                8 => 7,
                _ => 6,
            },
        }
    }

    fn types() -> HashMap<u32, EveType> {
        [
            eve_type(22456, "Sabre", 541, "Interdictor"),
            eve_type(12013, "Broadsword", 894, "Heavy Interdiction Cruiser"),
            eve_type(29984, "Tengu", 963, "Strategic Cruiser"),
            eve_type(11182, "Cheetah", 830, "Covert Ops"),
            eve_type(670, "Capsule", 29, "Capsule"),
            eve_type(30028, "Combat Scanner Probe I", 479, "Scanner Probe"),
            eve_type(30013, "Core Scanner Probe I", 479, "Scanner Probe"),
            eve_type(
                12199,
                "Mobile Large Warp Disruptor I",
                361,
                "Mobile Warp Disruptor",
            ),
            eve_type(37604, "Apostle", 1538, "Force Auxiliary"),
            eve_type(14, "Moon", 8, "Moon"),
        ]
        .into_iter()
        .map(|info| (info.type_id, info))
        .collect()
    }

    fn row(type_id: u32, distance_km: Option<f64>) -> DscanRow {
        DscanRow {
            type_id: Some(type_id),
            name: String::new(),
            type_name: String::new(),
            distance_km,
        }
    }

    #[test]
    fn local_names_are_trimmed_and_kept_once() {
        let lines = ["  Some Pilot ", "some pilot", "", "Other Pilot"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(local_names(&lines), vec!["Some Pilot", "Other Pilot"]);
    }

    #[test]
    fn dscan_rows_read_with_and_without_a_type_id() {
        let modern = parse_dscan_row("22456\tSomeone's Sabre\tSabre\t1,234 km").unwrap();
        assert_eq!(modern.type_id, Some(22456));
        assert_eq!(modern.name, "Someone's Sabre");
        assert_eq!(modern.type_name, "Sabre");
        assert_eq!(modern.distance_km, Some(1_234.0));
        assert!(modern.on_grid());

        let off_grid = parse_dscan_row("29984\tTengu\tTengu\t-").unwrap();
        assert_eq!(off_grid.distance_km, None);
        assert!(!off_grid.on_grid());
        let far = parse_dscan_row("14\tJ123456 VII - Moon 2\tMoon\t12.3 AU").unwrap();
        assert!(!far.on_grid());
        // German and Russian clients group and mark decimals differently.
        assert_eq!(
            parse_dscan_row("670\tKapsel\tKapsel\t1.234 km")
                .unwrap()
                .distance_km,
            Some(1_234.0)
        );
        assert_eq!(
            parse_dscan_row("670\tКапсула\tКапсула\t12 345 м")
                .unwrap()
                .distance_km,
            Some(12.345)
        );
        // Without a distance column at all.
        assert_eq!(
            parse_dscan_row("670\tCapsule\tCapsule")
                .unwrap()
                .distance_km,
            None
        );

        let legacy = parse_dscan_row("Wreck\tTayra\t82 km").unwrap();
        assert_eq!(legacy.type_id, None);
        assert_eq!(legacy.type_name, "Tayra");
        assert_eq!(legacy.distance_km, Some(82.0));
        assert!(parse_dscan_row("some dude's Stabber\tStabber Fleet Issue\t-").is_some());

        // Not d-scan rows.
        assert!(parse_dscan_row("Some Pilot").is_none());
        assert!(parse_dscan_row("Name\tType\tnot a distance").is_none());
        assert!(parse_dscan_row("670\tCapsule").is_none());
        assert!(parse_dscan_row("670\tCapsule\tCapsule\tfar away").is_none());
        assert!(
            parse_dscan_row("ABC-123\tCosmic Signature\tWormhole\tUnstable\t100.0%\t4.32 AU")
                .is_none()
        );
    }

    #[test]
    fn dscan_is_summarised_by_group_with_threats_called_out() {
        let rows = vec![
            row(22456, Some(500.0)),
            row(22456, None),
            row(12013, Some(2_000_000.0)),
            row(29984, None),
            row(29984, None),
            row(29984, Some(20.0)),
            row(670, Some(5.0)),
            row(30028, None),
            row(30013, None),
            row(12199, Some(100.0)),
            row(14, Some(1.0e9)),
            row(99_999, None),
            DscanRow {
                type_id: None,
                name: "Wreck".to_owned(),
                type_name: "Cheetah".to_owned(),
                distance_km: Some(80.0),
            },
        ];
        let ids_by_name = HashMap::from([("cheetah".to_owned(), 11182)]);
        let report = summarise_dscan(&rows, &types(), &ids_by_name, NOW);
        assert_eq!(report.rows, 13);
        assert_eq!(report.unidentified, 1);
        // Ships, capsules aside: 2 Sabres, a Broadsword, 3 Tengus and a Cheetah.
        assert_eq!(report.ships(), 7);
        assert_eq!(report.ships_on_grid(), 3);
        assert_eq!(report.entries[0].label, "Strategic Cruiser");
        assert_eq!(report.entries[0].count, 3);
        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.label == "Capsule" && !entry.ship)
        );
        assert_eq!(
            report.threats(),
            vec![
                (ThreatClass::HeavyInterdictor, 1, 0),
                (ThreatClass::Interdictor, 2, 1),
                (ThreatClass::Bubble, 1, 1),
                (ThreatClass::CombatProbe, 1, 0),
                (ThreatClass::CovertOps, 1, 1),
                (ThreatClass::StrategicCruiser, 3, 1),
            ]
        );
    }

    #[test]
    fn group_names_take_their_plurals() {
        assert_eq!(
            group_count_text("Strategic Cruiser", 3),
            "3 Strategic Cruisers"
        );
        assert_eq!(group_count_text("Covert Ops", 2), "2 Covert Ops");
        assert_eq!(group_count_text("Black Ops", 2), "2 Black Ops");
        assert_eq!(group_count_text("Force Auxiliary", 1), "1 Force Auxiliary");
        assert_eq!(
            group_count_text("Force Auxiliary", 4),
            "4 Force Auxiliaries"
        );
        assert_eq!(group_count_text("Capsule", 2), "2 Capsules");
        assert_eq!(ThreatClass::Interdictor.count_text(1), "1 interdictor");
        assert_eq!(
            ThreatClass::Bubble.count_text(2),
            "2 warp disruption bubbles"
        );
    }

    #[test]
    fn local_is_grouped_by_alliance_then_corporation() {
        let report = local(vec![
            pilot(1, 98_000_001, 99_000_001),
            pilot(2, 98_000_002, 99_000_001),
            pilot(3, 98_000_003, 0),
            pilot(4, 98_000_003, 0),
            pilot(5, 98_000_003, 0),
            pilot(6, 0, 0),
        ]);
        let groups = local_groups(&report);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Corp 98000003");
        assert_eq!(groups[0].count, 3);
        assert!(!groups[0].alliance);
        assert_eq!(groups[1].name, "Alliance 99000001");
        assert_eq!(groups[1].count, 2);
    }

    #[test]
    fn local_is_matched_against_killers_here() {
        let report = local(vec![
            pilot(1, 98_000_001, 0),
            pilot(2, 98_000_002, 0),
            pilot(3, 1_000_125, 0),
        ]);
        let kills = [
            kill(HOUR_SECS, &[1, 50], &[98_000_001], false),
            kill(3 * DAY_SECS, &[2], &[98_000_002], false),
            // Older than a week, an NPC kill, and a starter corporation: none count.
            kill(8 * DAY_SECS, &[3], &[1_000_125], false),
            kill(HOUR_SECS, &[2], &[98_000_002], true),
            kill(HOUR_SECS, &[77], &[1_000_125], false),
        ];
        assert_eq!(
            killer_overlap(&report, &kills, NOW),
            KillerOverlap {
                pilots_24h: 1,
                pilots_7d: 2,
                corporations_24h: 1,
                corporations_7d: 2,
            }
        );
    }

    #[test]
    fn fresh_intel_adds_evidence_and_threat() {
        let report = LocalReport {
            own: vec!["My Scout".to_owned()],
            unidentified: vec!["Typo Name".to_owned()],
            ..local(vec![
                pilot(1, 98_000_001, 99_000_001),
                pilot(2, 98_000_001, 99_000_001),
            ])
        };
        let report = LocalReport {
            pilot_count: 3,
            ..report
        };
        let kills = [kill(HOUR_SECS, &[1], &[98_000_001], false)];
        let intel = SystemLiveIntel {
            local: Some(report),
            dscan: None,
        };
        let live = contribution(&intel, Some(&kills[..]), NOW);
        assert_eq!(
            live.evidence,
            vec![
                "Local shows 3 pilots besides your 1 character",
                "Largest group in local: 2 from Alliance 99000001",
                "1 name in local could not be identified",
                "1 pilot in local got kills here in the last 24 hours",
            ]
        );
        // 3 × √3 for presence and 8 for the pilot who killed here today, plus 2 for
        // their corporation.
        assert!((live.threat - (3.0 * 3f32.sqrt() + 8.0 + 2.0)).abs() < 0.01);
        assert!((live.activity - 6.0 * 3f32.sqrt()).abs() < 0.01);

        // Without the killboard, local is not checked against it.
        let unchecked = contribution(&intel, None, NOW);
        assert_eq!(unchecked.evidence.len(), 3);
        assert!(unchecked.threat < live.threat);

        // A quiet killboard is said plainly.
        let quiet = contribution(&intel, Some(&[][..]), NOW);
        assert_eq!(
            quiet.evidence.last().map(String::as_str),
            Some("No one in local got kills here in the last 7 days")
        );
    }

    #[test]
    fn dictors_and_bubbles_raise_threat_more_than_plain_ships() {
        let dscan = |rows: Vec<DscanRow>| SystemLiveIntel {
            local: None,
            dscan: Some(summarise_dscan(&rows, &types(), &HashMap::new(), NOW - 30)),
        };
        let tengus = contribution(&dscan(vec![row(29984, None), row(29984, None)]), None, NOW);
        let dictors = contribution(&dscan(vec![row(22456, None), row(12199, None)]), None, NOW);
        assert!(dictors.threat > tengus.threat);
        assert_eq!(
            dictors.evidence,
            vec![
                "D-scan shows 1 ship, none on grid",
                "Ships on d-scan: 1 Interdictor",
                "D-scan picked up 1 interdictor and 1 warp disruption bubble",
            ]
        );
        let on_grid = contribution(
            &dscan(vec![row(29984, Some(10.0)), row(29984, None)]),
            None,
            NOW,
        );
        assert_eq!(on_grid.evidence[0], "D-scan shows 2 ships, 1 on grid");
        assert!(on_grid.threat > tengus.threat);
        assert_eq!(
            contribution(&dscan(vec![row(14, None)]), None, NOW).evidence,
            vec!["D-scan shows no ships"]
        );

        // However much is on scan, live intel stays a modest share.
        let fleet = (0..200).map(|_| row(22456, Some(10.0))).collect();
        let crowd = SystemLiveIntel {
            local: Some(local(
                (1..=400).map(|id| pilot(id, 98_000_001, 0)).collect(),
            )),
            ..dscan(fleet)
        };
        let kills = [kill(HOUR_SECS, &(1..=400).collect::<Vec<_>>(), &[], false)];
        let most = contribution(&crowd, Some(&kills[..]), NOW);
        assert_eq!(most.threat, 50.0);
        assert_eq!(most.activity, 40.0);
    }

    #[test]
    fn live_intel_expires_after_fifteen_minutes() {
        let mut intel = SystemLiveIntel {
            local: Some(LocalReport {
                captured_at: NOW,
                pilot_count: 4,
                ..LocalReport::default()
            }),
            dscan: Some(summarise_dscan(
                &[row(22456, None)],
                &types(),
                &HashMap::new(),
                NOW + 120,
            )),
        };
        assert_eq!(
            intel.expires_at(NOW + 120),
            Some(NOW + LIVE_INTEL_FRESH_SECS)
        );
        assert!(contribution(&intel, None, NOW + LIVE_INTEL_FRESH_SECS - 1).is_live());
        // Local has expired; the d-scan still counts for two more minutes.
        let later = NOW + LIVE_INTEL_FRESH_SECS;
        assert!(intel.fresh_local(later).is_none());
        assert_eq!(intel.expires_at(later), Some(later + 120));
        assert_eq!(
            contribution(&intel, None, later).evidence[0],
            "D-scan shows 1 ship, none on grid"
        );
        let gone = later + 120;
        assert_eq!(intel.expires_at(gone), None);
        assert!(!contribution(&intel, None, gone).is_live());
        assert_eq!(contribution(&intel, None, gone).threat, 0.0);
        // The pastes themselves are kept, as earlier intel.
        assert!(intel.local.is_some() && intel.dscan.is_some());
        intel.dscan = None;
        assert_eq!(intel.expires_at(NOW), Some(NOW + LIVE_INTEL_FRESH_SECS));
    }
}
