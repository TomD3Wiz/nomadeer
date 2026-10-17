//! What the player knows about each wormhole connection: its type, how much mass
//! it has left and whether it is dying, and what that means in plain words.

use crate::data::wormhole_types::{self, TargetClass, WORMHOLE_TYPES, WormholeType};
use crate::model::{Connection, SystemNode};

const HOUR_SECS: u64 = 3_600;
/// A wormhole marked end of life has less than this left.
pub(crate) const EOL_SECS: u64 = 4 * HOUR_SECS;
/// Every chain fact starts with this, so a refresh can find and replace them.
const CHAIN_FACT_PREFIX: &str = "Wormhole ";

/// How much mass a wormhole has left, as the player last saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MassState {
    Unknown,
    /// Over half its mass left.
    Stable,
    /// Under half: "destabilised" in game.
    Reduced,
    /// Under a tenth.
    Critical,
}

impl MassState {
    pub(crate) fn parse(status: &str) -> Self {
        match normalized(status).as_str() {
            "stable" | "fresh" => Self::Stable,
            "reduced" | "destab" | "destabilized" | "destabilised" | "half" => Self::Reduced,
            "critical" | "crit" => Self::Critical,
            _ => Self::Unknown,
        }
    }

    /// How it is stored.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Stable => "stable",
            Self::Reduced => "reduced",
            Self::Critical => "critical",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Mass unknown",
            Self::Stable => "Stable",
            Self::Reduced => "Mass reduced",
            Self::Critical => "Mass critical",
        }
    }
}

/// How long a wormhole has left, as the player last saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifeState {
    Unknown,
    Fresh,
    /// Under four hours left.
    EndOfLife,
}

impl LifeState {
    pub(crate) fn parse(status: &str) -> Self {
        match normalized(status).as_str() {
            "eol" | "end of life" | "dying" => Self::EndOfLife,
            "fresh" | "stable" | "new" => Self::Fresh,
            _ => Self::Unknown,
        }
    }

    /// How it is stored.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Fresh => "fresh",
            Self::EndOfLife => "eol",
        }
    }
}

fn normalized(text: &str) -> String {
    text.trim().to_lowercase().replace(['_', '-'], " ")
}

pub(crate) fn mass(connection: &Connection) -> MassState {
    MassState::parse(&connection.mass_status)
}

pub(crate) fn life(connection: &Connection) -> LifeState {
    LifeState::parse(&connection.life_status)
}

pub(crate) fn set_mass(connection: &mut Connection, mass: MassState) {
    connection.mass_status = mass.as_str().to_owned();
}

/// Marks it end of life from `now`. Marking it again keeps the first time, which is
/// the one the four hours count from.
pub(crate) fn mark_eol(connection: &mut Connection, now: u64) {
    if life(connection) != LifeState::EndOfLife || connection.eol_marked_at == 0 {
        connection.eol_marked_at = now;
    }
    connection.life_status = LifeState::EndOfLife.as_str().to_owned();
}

/// Forgets its mass and age, e.g. after the player rolled it and found a new one:
/// its lifetime then counts from `now`, as for a hole just mapped.
pub(crate) fn reset_state(connection: &mut Connection, now: u64) {
    connection.mass_status = MassState::Unknown.as_str().to_owned();
    connection.life_status = LifeState::Unknown.as_str().to_owned();
    connection.eol_marked_at = 0;
    connection.created_at = now;
}

/// The wormhole's own type, on the side it spawned.
pub(crate) fn wormhole_type(connection: &Connection) -> Option<&'static WormholeType> {
    wormhole_types::by_code(&connection.wormhole_code).filter(|wormhole| !wormhole.is_exit())
}

/// The other end of a connection from `system_id`, if it touches it.
pub(crate) fn other_side<'a>(connection: &'a Connection, system_id: &str) -> Option<&'a str> {
    if connection.from_system == system_id {
        Some(&connection.to_system)
    } else if connection.to_system == system_id {
        Some(&connection.from_system)
    } else {
        None
    }
}

/// The type a pilot sees on the signature in `system_id`: the wormhole's own code on
/// the side it spawned, K162 on the other.
pub(crate) fn type_on_side(
    connection: &Connection,
    system_id: &str,
) -> Option<&'static WormholeType> {
    let origin = connection.wormhole_origin.as_str();
    if origin.is_empty() || other_side(connection, system_id).is_none() {
        None
    } else if origin == system_id {
        wormhole_type(connection)
    } else {
        wormhole_types::by_code("K162")
    }
}

/// The type to label the connection with: its own code when known, else K162 when
/// only the exit side is known.
pub(crate) fn label_type(connection: &Connection) -> Option<&'static WormholeType> {
    wormhole_type(connection).or_else(|| {
        (!connection.wormhole_origin.is_empty())
            .then(|| wormhole_types::by_code("K162"))
            .flatten()
    })
}

/// Records the type read off the signature in `system_id`. A K162 there means the
/// wormhole spawned on the other side, whose code stays as it was if it was known.
pub(crate) fn set_type_on_side(
    connection: &mut Connection,
    system_id: &str,
    wormhole: &WormholeType,
) {
    let Some(other) = other_side(connection, system_id).map(str::to_owned) else {
        return;
    };
    if wormhole.is_exit() {
        if connection.wormhole_origin != other {
            connection.wormhole_code.clear();
        }
        connection.wormhole_origin = other;
    } else {
        connection.wormhole_code = wormhole.code.to_owned();
        connection.wormhole_origin = system_id.to_owned();
    }
}

pub(crate) fn clear_type(connection: &mut Connection) {
    connection.wormhole_code.clear();
    connection.wormhole_origin.clear();
}

/// Hours left at most, rounded up, from what the player marked and what the type
/// allows; None when nothing bounds it. Zero means it is overdue.
pub(crate) fn hours_left(connection: &Connection, now: u64) -> Option<u64> {
    let eol = (life(connection) == LifeState::EndOfLife && connection.eol_marked_at != 0)
        .then(|| (connection.eol_marked_at + EOL_SECS).saturating_sub(now));
    // It spawned before it was mapped, so it has at most its lifetime less the time
    // since then.
    let lifetime = wormhole_type(connection)
        .and_then(|wormhole| wormhole.stats)
        .filter(|_| connection.created_at != 0)
        .map(|stats| {
            let lifetime = (stats.max_stable_hours * HOUR_SECS as f32) as u64;
            (connection.created_at + lifetime).saturating_sub(now)
        });
    let left = match (eol, lifetime) {
        (Some(a), Some(b)) => a.min(b),
        (a, b) => a.or(b)?,
    };
    Some(left.div_ceil(HOUR_SECS))
}

/// "expires within ~3h", "at most ~14h left", or a warning once it is overdue.
pub(crate) fn expiry_text(connection: &Connection, now: u64) -> Option<String> {
    let hours = hours_left(connection, now)?;
    let dying = life(connection) == LifeState::EndOfLife;
    Some(match hours {
        0 => "may already have collapsed".to_owned(),
        hours if dying => format!("expires within ~{hours}h"),
        hours => format!("at most ~{hours}h left"),
    })
}

/// "Marked EOL 40m ago" for a connection marked end of life.
pub(crate) fn eol_marked_text(connection: &Connection, now: u64) -> Option<String> {
    (life(connection) == LifeState::EndOfLife && connection.eol_marked_at != 0).then(|| {
        format!(
            "marked EOL {} ago",
            short_duration(now.saturating_sub(connection.eol_marked_at))
        )
    })
}

/// "40m", "3h", "2d".
pub(crate) fn short_duration(seconds: u64) -> String {
    match seconds {
        ..60 => "<1m".to_owned(),
        60..HOUR_SECS => format!("{}m", seconds / 60),
        HOUR_SECS..86_400 => format!("{}h", seconds / HOUR_SECS),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// The kind of space a system is, as a wormhole type's target, from its class pill
/// ("C3", "HS", "Thera", ...). None when it is not known.
pub(crate) fn target_for_class(class: &str, pochven: bool) -> Option<TargetClass> {
    if pochven {
        return Some(TargetClass::Pochven);
    }
    Some(match class {
        "C1" => TargetClass::C1,
        "C2" => TargetClass::C2,
        "C3" => TargetClass::C3,
        "C4" => TargetClass::C4,
        "C5" => TargetClass::C5,
        "C6" => TargetClass::C6,
        "Thera" => TargetClass::Thera,
        "C13" => TargetClass::C13,
        "C14" => TargetClass::C14,
        "C15" => TargetClass::C15,
        "C16" => TargetClass::C16,
        "C17" => TargetClass::C17,
        "C18" => TargetClass::C18,
        "HS" => TargetClass::HighSec,
        "LS" => TargetClass::LowSec,
        "NS" => TargetClass::NullSec,
        _ => return None,
    })
}

/// Types the player could mean: those starting with what they typed and, when the
/// far side's class is known, leading there. K162 is always offered first.
pub(crate) fn candidate_types(
    typed: &str,
    leads_to: Option<TargetClass>,
) -> Vec<&'static WormholeType> {
    let typed = typed.trim().to_ascii_uppercase();
    let mut types = WORMHOLE_TYPES
        .iter()
        .filter(|wormhole| wormhole.code.starts_with(&typed))
        .filter(|wormhole| {
            wormhole.is_exit()
                || leads_to.is_none_or(|target| {
                    wormhole.target == target || wormhole.target == TargetClass::Varies
                })
        })
        .collect::<Vec<_>>();
    types.sort_by_key(|wormhole| (!wormhole.is_exit(), wormhole.code));
    types
}

/// "2B kg", "375M kg", "5M kg".
pub(crate) fn format_mass(kg: u64) -> String {
    const MILLION: u64 = 1_000_000;
    const BILLION: u64 = 1_000 * MILLION;
    if kg >= BILLION {
        let billions = kg as f64 / BILLION as f64;
        let text = format!("{billions:.2}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        format!("{text}B kg")
    } else if kg >= MILLION {
        format!("{}M kg", (kg as f64 / MILLION as f64).round())
    } else {
        format!("{kg} kg")
    }
}

/// One line on a type: where it leads, what fits, how much it takes and how long
/// it lives.
pub(crate) fn type_summary(wormhole: &WormholeType) -> String {
    match (wormhole.stats, wormhole.jump_mass_category()) {
        (Some(stats), Some(size)) => format!(
            "{} → {} · {} ships, {} a jump · {} total · lives up to {}h",
            wormhole.code,
            wormhole.target.label(),
            size.label(),
            format_mass(stats.max_jump_mass_kg),
            format_mass(stats.total_mass_kg),
            stats.max_stable_hours.round() as u64
        ),
        _ => format!(
            "{}: the exit side; the far side's type decides where it leads",
            wormhole.code
        ),
    }
}

/// A short state line, e.g. "EOL · Mass reduced · expires within ~3h"; empty when
/// nothing is known.
pub(crate) fn state_text(connection: &Connection, now: u64) -> String {
    let mut parts = Vec::new();
    match life(connection) {
        LifeState::EndOfLife => parts.push("EOL".to_owned()),
        LifeState::Fresh => parts.push("Fresh".to_owned()),
        LifeState::Unknown => {}
    }
    let mass = mass(connection);
    if mass != MassState::Unknown {
        parts.push(mass.label().to_owned());
    }
    if let Some(expiry) = expiry_text(connection, now) {
        parts.push(expiry);
    }
    parts.join(" · ")
}

/// Facts about the connections out of `system_id` for Umbra's evidence, e.g.
/// "Wormhole B274 to Jita (HS 0.9) is at end of life, expiring within about 3
/// hours". Connections the player has said nothing about are left out.
pub(crate) fn chain_facts(
    system_id: &str,
    systems: &[SystemNode],
    connections: &[Connection],
    now: u64,
    class_label: impl Fn(&SystemNode) -> String,
) -> Vec<String> {
    let mut facts = Vec::new();
    for connection in connections {
        let Some(other_id) = other_side(connection, system_id) else {
            continue;
        };
        let mass = mass(connection);
        let life = life(connection);
        let seen_type = type_on_side(connection, system_id);
        if seen_type.is_none() && mass == MassState::Unknown && life == LifeState::Unknown {
            continue;
        }
        let destination = systems
            .iter()
            .find(|system| system.id == other_id)
            .map(|system| format!("{} ({})", system.name, class_label(system)))
            .unwrap_or_else(|| "an unknown system".to_owned());
        let mut fact = match seen_type {
            Some(wormhole) => format!("{CHAIN_FACT_PREFIX}{} to {destination}", wormhole.code),
            None => format!("{CHAIN_FACT_PREFIX}to {destination}"),
        };
        let mut states = Vec::new();
        match life {
            LifeState::EndOfLife => states.push("at end of life"),
            LifeState::Fresh => states.push("fresh"),
            LifeState::Unknown => {}
        }
        match mass {
            MassState::Stable => states.push("stable in mass"),
            MassState::Reduced => states.push("reduced in mass"),
            MassState::Critical => states.push("mass critical"),
            MassState::Unknown => {}
        }
        if !states.is_empty() {
            fact.push_str(" is ");
            fact.push_str(&join_and(&states));
        }
        match hours_left(connection, now) {
            Some(0) => fact.push_str(", and may already have collapsed"),
            Some(hours) if life == LifeState::EndOfLife => fact.push_str(&format!(
                ", expiring within about {}",
                count_label(hours, "hour")
            )),
            _ => {}
        }
        facts.push(fact);
    }
    facts
}

/// Whether an evidence line came from `chain_facts`.
pub(crate) fn is_chain_fact(line: &str) -> bool {
    line.starts_with(CHAIN_FACT_PREFIX)
}

/// Swaps the chain facts at the end of some evidence for `facts`; true if that
/// changed anything.
pub(crate) fn replace_chain_facts(evidence: &mut Vec<String>, facts: Vec<String>) -> bool {
    let kept = evidence
        .iter()
        .filter(|line| !is_chain_fact(line))
        .cloned()
        .collect::<Vec<_>>();
    let updated = kept.into_iter().chain(facts).collect::<Vec<_>>();
    if *evidence == updated {
        return false;
    }
    *evidence = updated;
    true
}

fn join_and(parts: &[&str]) -> String {
    match parts {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn count_label(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SystemNode;

    const HOUR: u64 = 3_600;
    const NOW: u64 = 1_760_000_000;

    fn connection() -> Connection {
        Connection {
            id: "conn_1".to_owned(),
            from_system: "home".to_owned(),
            to_system: "jita".to_owned(),
            mass_status: "unknown".to_owned(),
            life_status: "unknown".to_owned(),
            created_at: NOW - HOUR,
            ..Connection::default()
        }
    }

    fn system(id: &str, name: &str) -> SystemNode {
        SystemNode {
            id: id.to_owned(),
            solar_system_id: 0,
            name: name.to_owned(),
            class: String::new(),
            security_status: 0.0,
            constellation_id: 0,
            constellation_name: String::new(),
            region_id: 0,
            region_name: String::new(),
            planet_count: 0,
            stargate_count: 0,
            station_count: 0,
            jumps_1h: 0,
            ship_kills_1h: 0,
            pod_kills_1h: 0,
            npc_kills_1h: 0,
            intel_updated_at: 0,
            effect: String::new(),
            status: String::new(),
            owner: String::new(),
            notes: String::new(),
            created_at: 0,
            map_position: None,
        }
    }

    #[test]
    fn states_read_older_words_and_store_their_own() {
        assert_eq!(MassState::parse("destab"), MassState::Reduced);
        assert_eq!(MassState::parse("Critical"), MassState::Critical);
        assert_eq!(MassState::parse(""), MassState::Unknown);
        assert_eq!(LifeState::parse("end_of_life"), LifeState::EndOfLife);
        assert_eq!(LifeState::parse("EOL"), LifeState::EndOfLife);
        assert_eq!(LifeState::parse("unknown"), LifeState::Unknown);
        for mass in [
            MassState::Unknown,
            MassState::Stable,
            MassState::Reduced,
            MassState::Critical,
        ] {
            assert_eq!(MassState::parse(mass.as_str()), mass);
        }
        for life in [LifeState::Unknown, LifeState::Fresh, LifeState::EndOfLife] {
            assert_eq!(LifeState::parse(life.as_str()), life);
        }
    }

    #[test]
    fn marking_eol_counts_four_hours_from_the_first_mark() {
        let mut connection = connection();
        assert_eq!(hours_left(&connection, NOW), None);
        mark_eol(&mut connection, NOW);
        assert_eq!(life(&connection), LifeState::EndOfLife);
        assert_eq!(connection.eol_marked_at, NOW);
        assert_eq!(hours_left(&connection, NOW), Some(4));
        assert_eq!(
            expiry_text(&connection, NOW + HOUR / 2).as_deref(),
            Some("expires within ~4h")
        );
        // Marking again later does not restart the clock.
        mark_eol(&mut connection, NOW + 2 * HOUR);
        assert_eq!(connection.eol_marked_at, NOW);
        assert_eq!(hours_left(&connection, NOW + 2 * HOUR + 60), Some(2));
        assert_eq!(
            expiry_text(&connection, NOW + 5 * HOUR).as_deref(),
            Some("may already have collapsed")
        );
        assert_eq!(
            eol_marked_text(&connection, NOW + 40 * 60).as_deref(),
            Some("marked EOL 40m ago")
        );

        set_mass(&mut connection, MassState::Critical);
        assert_eq!(mass(&connection), MassState::Critical);
        reset_state(&mut connection, NOW);
        assert_eq!(mass(&connection), MassState::Unknown);
        assert_eq!(life(&connection), LifeState::Unknown);
        assert_eq!(connection.eol_marked_at, 0);
        assert_eq!(hours_left(&connection, NOW), None);
    }

    #[test]
    fn an_eol_mark_from_older_data_has_no_clock() {
        let mut connection = connection();
        connection.life_status = "eol".to_owned();
        assert_eq!(hours_left(&connection, NOW), None);
        assert_eq!(eol_marked_text(&connection, NOW), None);
        assert_eq!(state_text(&connection, NOW), "EOL");
    }

    #[test]
    fn a_known_type_bounds_the_time_left() {
        let mut connection = connection();
        // B274 lives up to 24 hours and was mapped an hour ago.
        set_type_on_side(
            &mut connection,
            "home",
            wormhole_types::by_code("B274").unwrap(),
        );
        assert_eq!(hours_left(&connection, NOW), Some(23));
        assert_eq!(
            expiry_text(&connection, NOW).as_deref(),
            Some("at most ~23h left")
        );
        // EOL is the tighter bound.
        mark_eol(&mut connection, NOW);
        assert_eq!(hours_left(&connection, NOW), Some(4));
        // Unless the type's lifetime runs out first.
        connection.created_at = NOW - 22 * HOUR;
        assert_eq!(hours_left(&connection, NOW), Some(2));

        // Rolled and found again, it has its whole lifetime ahead.
        reset_state(&mut connection, NOW);
        assert_eq!(hours_left(&connection, NOW), Some(24));
        assert_eq!(
            expiry_text(&connection, NOW + 23 * HOUR).as_deref(),
            Some("at most ~1h left")
        );
    }

    #[test]
    fn the_code_belongs_to_the_side_it_spawned() {
        let mut connection = connection();
        assert_eq!(type_on_side(&connection, "home"), None);
        assert_eq!(label_type(&connection), None);

        set_type_on_side(
            &mut connection,
            "home",
            wormhole_types::by_code("b274").unwrap(),
        );
        assert_eq!(connection.wormhole_code, "B274");
        assert_eq!(connection.wormhole_origin, "home");
        assert_eq!(type_on_side(&connection, "home").unwrap().code, "B274");
        assert_eq!(type_on_side(&connection, "jita").unwrap().code, "K162");
        assert_eq!(type_on_side(&connection, "elsewhere"), None);
        assert_eq!(label_type(&connection).unwrap().code, "B274");

        // Reading K162 on the far side agrees and keeps the code.
        set_type_on_side(
            &mut connection,
            "jita",
            wormhole_types::by_code("K162").unwrap(),
        );
        assert_eq!(connection.wormhole_code, "B274");
        assert_eq!(connection.wormhole_origin, "home");

        // A K162 at home means it spawned in Jita, whose code is not known.
        set_type_on_side(
            &mut connection,
            "home",
            wormhole_types::by_code("K162").unwrap(),
        );
        assert_eq!(connection.wormhole_code, "");
        assert_eq!(connection.wormhole_origin, "jita");
        assert_eq!(type_on_side(&connection, "home").unwrap().code, "K162");
        assert_eq!(type_on_side(&connection, "jita"), None);
        assert_eq!(label_type(&connection).unwrap().code, "K162");

        clear_type(&mut connection);
        assert_eq!(label_type(&connection), None);
    }

    #[test]
    fn candidates_follow_the_far_side_and_what_was_typed() {
        let high_sec = candidate_types("", Some(TargetClass::HighSec));
        assert_eq!(high_sec[0].code, "K162");
        assert!(high_sec.iter().any(|wormhole| wormhole.code == "B274"));
        assert!(high_sec.iter().all(|wormhole| {
            wormhole.is_exit()
                || matches!(wormhole.target, TargetClass::HighSec | TargetClass::Varies)
        }));
        assert!(!high_sec.iter().any(|wormhole| wormhole.code == "H296"));

        let typed = candidate_types("b2", None);
        assert!(typed.iter().any(|wormhole| wormhole.code == "B274"));
        assert!(typed.iter().all(|wormhole| wormhole.code.starts_with("B2")));
        assert_eq!(candidate_types("", None).len(), WORMHOLE_TYPES.len());

        assert_eq!(target_for_class("HS", false), Some(TargetClass::HighSec));
        assert_eq!(target_for_class("C5", false), Some(TargetClass::C5));
        assert_eq!(target_for_class("NS", true), Some(TargetClass::Pochven));
        assert_eq!(target_for_class("WH", false), None);
        assert_eq!(target_for_class("Unknown", false), None);
    }

    #[test]
    fn type_lines_read_like_the_game() {
        assert_eq!(format_mass(2_000_000_000), "2B kg");
        assert_eq!(format_mass(1_350_000_000), "1.35B kg");
        assert_eq!(format_mass(375_000_000), "375M kg");
        assert_eq!(format_mass(5_000_000), "5M kg");
        let b274 = wormhole_types::by_code("B274").unwrap();
        let summary = type_summary(b274);
        assert!(summary.starts_with("B274 → High-sec · "), "{summary}");
        assert!(summary.contains("lives up to 24h"), "{summary}");
        assert!(type_summary(wormhole_types::by_code("K162").unwrap()).starts_with("K162: "));
        assert_eq!(short_duration(30), "<1m");
        assert_eq!(short_duration(5 * 60), "5m");
        assert_eq!(short_duration(3 * HOUR + 59), "3h");
        assert_eq!(short_duration(50 * HOUR), "2d");
    }

    #[test]
    fn chain_facts_name_what_the_player_marked() {
        let systems = [system("home", "J123456"), system("jita", "Jita")];
        let class = |system: &SystemNode| {
            if system.id == "jita" {
                "HS 0.9".to_owned()
            } else {
                "C2".to_owned()
            }
        };
        let mut connection = connection();
        assert!(chain_facts("home", &systems, &[connection.clone()], NOW, class).is_empty());

        set_type_on_side(
            &mut connection,
            "home",
            wormhole_types::by_code("B274").unwrap(),
        );
        mark_eol(&mut connection, NOW - HOUR);
        set_mass(&mut connection, MassState::Reduced);
        let facts = chain_facts("home", &systems, &[connection.clone()], NOW, class);
        assert_eq!(
            facts,
            vec![
                "Wormhole B274 to Jita (HS 0.9) is at end of life and reduced in mass, expiring within about 3 hours"
            ]
        );
        assert!(facts.iter().all(|fact| is_chain_fact(fact)));

        // From the far side it is the K162 back.
        let facts = chain_facts("jita", &systems, &[connection.clone()], NOW, class);
        assert_eq!(
            facts,
            vec![
                "Wormhole K162 to J123456 (C2) is at end of life and reduced in mass, expiring within about 3 hours"
            ]
        );

        connection.eol_marked_at = NOW - 5 * HOUR;
        let facts = chain_facts("home", &systems, &[connection], NOW, class);
        assert!(facts[0].ends_with(", and may already have collapsed"));
        assert!(!is_chain_fact("No kills in the last 24 hours"));
    }

    #[test]
    fn chain_facts_are_replaced_and_kill_evidence_kept() {
        let mut evidence = vec![
            "No kills in the last 24 hours".to_owned(),
            "Wormhole B274 to Jita (HS 0.9) is fresh".to_owned(),
        ];
        assert!(!replace_chain_facts(
            &mut evidence,
            vec!["Wormhole B274 to Jita (HS 0.9) is fresh".to_owned()]
        ));
        assert!(replace_chain_facts(
            &mut evidence,
            vec!["Wormhole B274 to Jita (HS 0.9) is at end of life".to_owned()]
        ));
        assert_eq!(
            evidence,
            vec![
                "No kills in the last 24 hours",
                "Wormhole B274 to Jita (HS 0.9) is at end of life"
            ]
        );
        assert!(replace_chain_facts(&mut evidence, Vec::new()));
        assert_eq!(evidence, vec!["No kills in the last 24 hours"]);
    }
}
