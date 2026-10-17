//! Every wormhole type: where it leads, how long it lives and what fits through.
//! The table is generated from ESI by tools/data/gen_wormhole_types.py; rerun it
//! after a patch adds or changes holes rather than editing it.

// Not shown on signatures or connections yet.
#![allow(dead_code)]

/// The kind of space a wormhole leads to, from its type's target system class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TargetClass {
    C1,
    C2,
    C3,
    C4,
    C5,
    C6,
    HighSec,
    LowSec,
    NullSec,
    Thera,
    /// Shattered frigate-only systems.
    C13,
    /// The five Drifter systems.
    C14,
    C15,
    C16,
    C17,
    C18,
    Pochven,
    /// The type names no class; each spawn picks its destination instead (C729).
    Varies,
    /// K162: the far side's type decides where it leads.
    Unknown,
}

impl TargetClass {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::C1 => "C1",
            Self::C2 => "C2",
            Self::C3 => "C3",
            Self::C4 => "C4",
            Self::C5 => "C5",
            Self::C6 => "C6",
            Self::HighSec => "High-sec",
            Self::LowSec => "Low-sec",
            Self::NullSec => "Null-sec",
            Self::Thera => "Thera",
            Self::C13 => "Shattered C13",
            Self::C14 => "Drifter C14",
            Self::C15 => "Drifter C15",
            Self::C16 => "Drifter C16",
            Self::C17 => "Drifter C17",
            Self::C18 => "Drifter C18",
            Self::Pochven => "Pochven",
            Self::Varies => "Varies",
            Self::Unknown => "Unknown",
        }
    }
}

/// The biggest hulls a wormhole lets through, from its maximum jump mass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum JumpMassCategory {
    /// Up to 5,000,000 kg: frigates and destroyers.
    Frigate,
    /// Up to 62,000,000 kg: cruisers and battlecruisers.
    Medium,
    /// Up to 500,000,000 kg (375M and 410M holes): battleships and industrial command ships.
    Large,
    /// Up to 1,000,000,000 kg: freighters and jump freighters.
    Freighter,
    /// Over 1,000,000,000 kg: capitals.
    Capital,
}

impl JumpMassCategory {
    pub(crate) fn from_max_jump_mass(kg: u64) -> Self {
        match kg {
            ..=5_000_000 => Self::Frigate,
            5_000_001..=62_000_000 => Self::Medium,
            62_000_001..=500_000_000 => Self::Large,
            500_000_001..=1_000_000_000 => Self::Freighter,
            _ => Self::Capital,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Frigate => "Frigate",
            Self::Medium => "Medium",
            Self::Large => "Large",
            Self::Freighter => "Freighter",
            Self::Capital => "Capital",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WormholeStats {
    pub(crate) max_stable_hours: f32,
    /// Mass the hole takes before it collapses.
    pub(crate) total_mass_kg: u64,
    /// Heaviest ship that can jump through in one go.
    pub(crate) max_jump_mass_kg: u64,
    /// Mass restored each regeneration cycle; zero for most holes.
    pub(crate) mass_regeneration_kg: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WormholeType {
    /// The signature's type code, e.g. "B274".
    pub(crate) code: &'static str,
    pub(crate) type_id: u32,
    /// Further type IDs sharing this code and stats, e.g. the C729 variants.
    pub(crate) alt_type_ids: &'static [u32],
    pub(crate) target: TargetClass,
    /// None for K162, whose stats are those of the hole on the far side.
    pub(crate) stats: Option<WormholeStats>,
}

impl WormholeType {
    pub(crate) fn is_exit(&self) -> bool {
        self.stats.is_none()
    }

    pub(crate) fn jump_mass_category(&self) -> Option<JumpMassCategory> {
        self.stats
            .map(|stats| JumpMassCategory::from_max_jump_mass(stats.max_jump_mass_kg))
    }
}

/// Finds a wormhole by its type code, ignoring case and surrounding space.
pub(crate) fn by_code(code: &str) -> Option<&'static WormholeType> {
    let code = code.trim();
    WORMHOLE_TYPES
        .iter()
        .find(|wormhole| wormhole.code.eq_ignore_ascii_case(code))
}

pub(crate) fn by_type_id(type_id: u32) -> Option<&'static WormholeType> {
    WORMHOLE_TYPES
        .iter()
        .find(|wormhole| wormhole.type_id == type_id || wormhole.alt_type_ids.contains(&type_id))
}

const fn wormhole(
    code: &'static str,
    type_id: u32,
    target: TargetClass,
    max_stable_hours: f32,
    total_mass_kg: u64,
    max_jump_mass_kg: u64,
    mass_regeneration_kg: u64,
) -> WormholeType {
    WormholeType {
        code,
        type_id,
        alt_type_ids: &[],
        target,
        stats: Some(WormholeStats {
            max_stable_hours,
            total_mass_kg,
            max_jump_mass_kg,
            mass_regeneration_kg,
        }),
    }
}

const fn exit(code: &'static str, type_id: u32) -> WormholeType {
    WormholeType {
        code,
        type_id,
        alt_type_ids: &[],
        target: TargetClass::Unknown,
        stats: None,
    }
}

impl WormholeType {
    const fn also(mut self, alt_type_ids: &'static [u32]) -> Self {
        self.alt_type_ids = alt_type_ids;
        self
    }
}

// Lets the table name its targets bare.
use TargetClass::*;

// BEGIN GENERATED by tools/data/gen_wormhole_types.py
// 102 codes from 130 types in ESI group 988 "Wormhole".

/// Code, type ID, target, max stable hours, total mass, max jump mass and mass
/// regeneration (kg); `also` lists further type IDs sharing the code.
#[rustfmt::skip]
pub(crate) static WORMHOLE_TYPES: &[WormholeType] = &[
    wormhole("Z971", 30579, C1, 16.0, 100_000_000, 62_000_000, 0),
    wormhole("R943", 30583, C2, 16.0, 750_000_000, 375_000_000, 0),
    wormhole("X702", 30584, C3, 24.0, 1_000_000_000, 375_000_000, 0),
    wormhole("O128", 30642, C4, 24.0, 1_000_000_000, 375_000_000, 0),
    wormhole("N432", 30643, C5, 24.0, 3_300_000_000, 2_000_000_000, 0),
    wormhole("M555", 30644, C5, 24.0, 3_000_000_000, 1_000_000_000, 0),
    wormhole("B041", 30645, C6, 48.0, 3_000_000_000, 1_000_000_000, 0),
    wormhole("U319", 30646, C6, 48.0, 3_300_000_000, 2_000_000_000, 0),
    wormhole("B449", 30647, HighSec, 16.0, 2_000_000_000, 1_000_000_000, 0),
    wormhole("N944", 30648, LowSec, 24.0, 3_300_000_000, 2_000_000_000, 0),
    wormhole("S199", 30649, NullSec, 16.0, 3_300_000_000, 2_000_000_000, 0),
    wormhole("A641", 30657, HighSec, 16.0, 2_000_000_000, 1_000_000_000, 0),
    wormhole("R051", 30658, LowSec, 16.0, 3_000_000_000, 1_000_000_000, 0),
    wormhole("V283", 30659, NullSec, 16.0, 3_000_000_000, 1_000_000_000, 0),
    wormhole("H121", 30660, C1, 16.0, 500_000_000, 62_000_000, 0),
    wormhole("C125", 30661, C2, 16.0, 1_000_000_000, 62_000_000, 0),
    wormhole("O883", 30662, C3, 16.0, 1_000_000_000, 62_000_000, 0),
    wormhole("M609", 30663, C4, 16.0, 1_000_000_000, 62_000_000, 0),
    wormhole("L614", 30664, C5, 24.0, 1_000_000_000, 62_000_000, 0),
    wormhole("S804", 30665, C6, 24.0, 1_000_000_000, 62_000_000, 0),
    wormhole("N110", 30666, HighSec, 24.0, 1_000_000_000, 62_000_000, 0),
    wormhole("J244", 30667, LowSec, 24.0, 1_000_000_000, 62_000_000, 0).also(&[73748]),
    wormhole("Z060", 30668, NullSec, 16.0, 1_000_000_000, 62_000_000, 0),
    wormhole("Z647", 30671, C1, 16.0, 500_000_000, 62_000_000, 0),
    wormhole("D382", 30672, C2, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("O477", 30673, C3, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("Y683", 30674, C4, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("N062", 30675, C5, 24.0, 3_000_000_000, 375_000_000, 0),
    wormhole("R474", 30676, C6, 24.0, 3_000_000_000, 375_000_000, 0),
    wormhole("B274", 30677, HighSec, 24.0, 2_000_000_000, 375_000_000, 0),
    wormhole("A239", 30678, LowSec, 24.0, 2_000_000_000, 375_000_000, 0),
    wormhole("E545", 30679, NullSec, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("V301", 30680, C1, 16.0, 500_000_000, 62_000_000, 0),
    wormhole("I182", 30681, C2, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("N968", 30682, C3, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("T405", 30683, C4, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("N770", 30684, C5, 24.0, 3_000_000_000, 375_000_000, 0),
    wormhole("A982", 30685, C6, 24.0, 3_000_000_000, 375_000_000, 0),
    wormhole("S047", 30686, HighSec, 24.0, 3_000_000_000, 375_000_000, 0),
    wormhole("U210", 30687, LowSec, 24.0, 3_000_000_000, 375_000_000, 0),
    wormhole("K346", 30688, NullSec, 16.0, 3_000_000_000, 375_000_000, 0),
    wormhole("P060", 30689, C1, 16.0, 500_000_000, 62_000_000, 0),
    wormhole("N766", 30690, C2, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("C247", 30691, C3, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("X877", 30692, C4, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("H900", 30693, C5, 24.0, 3_000_000_000, 375_000_000, 0),
    wormhole("U574", 30694, C6, 24.0, 3_000_000_000, 375_000_000, 0),
    wormhole("D845", 30695, HighSec, 24.0, 5_000_000_000, 375_000_000, 0),
    wormhole("N290", 30696, LowSec, 24.0, 3_000_000_000, 375_000_000, 0),
    wormhole("K329", 30697, NullSec, 16.0, 3_000_000_000, 375_000_000, 0),
    wormhole("Y790", 30698, C1, 16.0, 500_000_000, 62_000_000, 0),
    wormhole("D364", 30699, C2, 16.0, 1_000_000_000, 375_000_000, 0),
    wormhole("M267", 30700, C3, 16.0, 1_000_000_000, 375_000_000, 0),
    wormhole("E175", 30701, C4, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("H296", 30702, C5, 24.0, 3_300_000_000, 2_000_000_000, 0),
    wormhole("V753", 30703, C6, 24.0, 3_300_000_000, 2_000_000_000, 0),
    wormhole("D792", 30704, HighSec, 24.0, 3_000_000_000, 1_000_000_000, 0),
    wormhole("C140", 30705, LowSec, 24.0, 3_300_000_000, 2_000_000_000, 0),
    wormhole("Z142", 30706, NullSec, 16.0, 3_300_000_000, 2_000_000_000, 0),
    wormhole("Q317", 30707, C1, 16.0, 500_000_000, 62_000_000, 0),
    wormhole("G024", 30708, C2, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("L477", 30709, C3, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("Z457", 30710, C4, 16.0, 2_000_000_000, 375_000_000, 0),
    wormhole("V911", 30711, C5, 24.0, 3_300_000_000, 2_000_000_000, 0),
    wormhole("W237", 30712, C6, 24.0, 3_300_000_000, 2_000_000_000, 0),
    wormhole("B520", 30713, HighSec, 48.0, 3_000_000_000, 1_000_000_000, 0),
    wormhole("C391", 30714, LowSec, 48.0, 3_300_000_000, 2_000_000_000, 0),
    wormhole("C248", 30715, NullSec, 16.0, 3_300_000_000, 2_000_000_000, 0),
    exit("K162", 30831),
    wormhole("E004", 34134, C1, 4.5, 3_000_000_000, 5_000_000, 3_000_000_000),
    wormhole("L005", 34135, C2, 4.5, 3_000_000_000, 5_000_000, 3_000_000_000),
    wormhole("Z006", 34136, C3, 4.5, 3_000_000_000, 5_000_000, 3_000_000_000),
    wormhole("M001", 34137, C4, 4.5, 3_000_000_000, 5_000_000, 3_000_000_000),
    wormhole("C008", 34138, C5, 4.5, 3_000_000_000, 5_000_000, 3_000_000_000),
    wormhole("G008", 34139, C6, 4.5, 3_000_000_000, 5_000_000, 3_000_000_000),
    wormhole("Q003", 34140, NullSec, 4.5, 3_000_000_000, 5_000_000, 3_000_000_000),
    wormhole("T458", 34338, Thera, 16.0, 500_000_000, 62_000_000, 0),
    wormhole("M164", 34366, Thera, 16.0, 2_000_000_000, 1_000_000_000, 0),
    wormhole("L031", 34367, Thera, 16.0, 3_000_000_000, 1_000_000_000, 0),
    wormhole("Q063", 34368, HighSec, 16.0, 500_000_000, 62_000_000, 0),
    wormhole("V898", 34369, LowSec, 16.0, 2_000_000_000, 1_000_000_000, 0),
    wormhole("E587", 34370, NullSec, 16.0, 3_000_000_000, 1_000_000_000, 0),
    wormhole("F353", 34371, Thera, 16.0, 100_000_000, 62_000_000, 0),
    wormhole("F135", 34372, Thera, 16.0, 750_000_000, 375_000_000, 0),
    wormhole("A009", 34439, C13, 4.5, 3_000_000_000, 5_000_000, 3_000_000_000),
    wormhole("S877", 35650, C14, 16.0, 750_000_000, 375_000_000, 0),
    wormhole("B735", 35651, C15, 16.0, 750_000_000, 375_000_000, 0),
    wormhole("V928", 35652, C16, 16.0, 750_000_000, 375_000_000, 0),
    wormhole("C414", 35653, C17, 16.0, 750_000_000, 375_000_000, 0),
    wormhole("R259", 35654, C18, 16.0, 750_000_000, 375_000_000, 0),
    wormhole("C729", 56026, Varies, 12.0, 1_000_000_000, 410_000_000, 0).also(&[56535, 56536, 56537, 56538, 56539, 56545, 56546, 56547, 56548, 56549, 56550, 56551, 56552, 56553, 56554, 56555, 56556, 56557, 56558, 56559, 56560, 56561, 56562, 56563, 56564, 56565]),
    wormhole("X450", 56540, NullSec, 12.0, 1_000_000_000, 375_000_000, 0),
    wormhole("R081", 56541, C4, 12.0, 1_000_000_000, 375_000_000, 0),
    wormhole("F216", 56542, Pochven, 12.0, 1_000_000_000, 375_000_000, 0).also(&[56543]),
    wormhole("U372", 56544, Pochven, 12.0, 1_000_000_000, 375_000_000, 0),
    wormhole("J377", 73749, LowSec, 24.0, 1_000_000_000, 62_000_000, 0),
    wormhole("J492", 87827, LowSec, 24.0, 1_000_000_000, 62_000_000, 0),
    wormhole("I078", 92287, Pochven, 12.0, 750_000_000, 62_000_000, 0),
    wormhole("L687", 92288, Pochven, 12.0, 750_000_000, 62_000_000, 0),
    wormhole("O546", 92289, Pochven, 12.0, 750_000_000, 62_000_000, 0),
    wormhole("R327", 97881, Pochven, 12.0, 1_000_000_000, 375_000_000, 0),
    wormhole("O753", 97882, Pochven, 12.0, 1_000_000_000, 375_000_000, 0),
];
// END GENERATED

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_well_formed() {
        let mut codes: Vec<_> = WORMHOLE_TYPES
            .iter()
            .map(|wormhole| wormhole.code)
            .collect();
        codes.sort_unstable();
        let count = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), count, "duplicate wormhole codes");
        for code in codes {
            let bytes = code.as_bytes();
            assert!(
                bytes.len() == 4
                    && bytes[0].is_ascii_uppercase()
                    && bytes[1..].iter().all(u8::is_ascii_digit),
                "odd wormhole code {code}"
            );
        }
    }

    #[test]
    fn type_ids_are_unique() {
        let mut ids: Vec<_> = WORMHOLE_TYPES
            .iter()
            .flat_map(|wormhole| std::iter::once(&wormhole.type_id).chain(wormhole.alt_type_ids))
            .collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "a type ID appears twice");
    }

    #[test]
    fn finds_by_code_ignoring_case() {
        let b274 = by_code("b274").expect("B274 is a known hole");
        assert_eq!(b274.code, "B274");
        assert_eq!(b274.type_id, 30677);
        assert_eq!(b274.target, HighSec);
        let stats = b274.stats.unwrap();
        assert_eq!(stats.max_stable_hours, 24.0);
        assert_eq!(stats.total_mass_kg, 2_000_000_000);
        assert_eq!(stats.max_jump_mass_kg, 375_000_000);
        assert_eq!(by_code("  B274 "), Some(b274));
        assert_eq!(by_code("Z999"), None);
        assert_eq!(by_code(""), None);
    }

    #[test]
    fn finds_by_type_id_including_variants() {
        assert_eq!(
            by_type_id(30579).map(|wormhole| wormhole.code),
            Some("Z971")
        );
        let c729 = by_code("C729").unwrap();
        assert!(!c729.alt_type_ids.is_empty());
        for &id in c729.alt_type_ids {
            assert_eq!(by_type_id(id), Some(c729));
        }
        assert_eq!(by_type_id(0), None);
    }

    #[test]
    fn k162_is_the_generic_exit() {
        let k162 = by_code("K162").unwrap();
        assert!(k162.is_exit());
        assert_eq!(k162.target, Unknown);
        assert_eq!(k162.jump_mass_category(), None);
        assert_eq!(
            WORMHOLE_TYPES
                .iter()
                .filter(|wormhole| wormhole.is_exit())
                .count(),
            1
        );
    }

    #[test]
    fn every_hole_has_sane_stats() {
        for wormhole in WORMHOLE_TYPES.iter().filter(|wormhole| !wormhole.is_exit()) {
            let stats = wormhole.stats.unwrap();
            assert!(
                (1.0..=48.0).contains(&stats.max_stable_hours),
                "{} lives {}h",
                wormhole.code,
                stats.max_stable_hours
            );
            assert!(stats.max_jump_mass_kg > 0, "{}", wormhole.code);
            assert!(
                stats.max_jump_mass_kg <= stats.total_mass_kg,
                "{} lets more through in one jump than in total",
                wormhole.code
            );
            assert_ne!(wormhole.target, Unknown, "{}", wormhole.code);
        }
    }

    #[test]
    fn known_holes_land_in_the_right_space() {
        let cases = [
            ("Z971", C1),
            ("O477", C3),
            ("H296", C5),
            ("N944", LowSec),
            ("S199", NullSec),
            ("T458", Thera),
            ("A009", C13),
            ("S877", C14),
            ("R259", C18),
            ("F216", Pochven),
            ("C729", Varies),
        ];
        for (code, target) in cases {
            assert_eq!(by_code(code).unwrap().target, target, "{code}");
        }
    }

    #[test]
    fn jump_mass_categories_follow_the_hole_sizes() {
        let category = |code| by_code(code).unwrap().jump_mass_category().unwrap();
        assert_eq!(category("E004"), JumpMassCategory::Frigate);
        assert_eq!(category("Z971"), JumpMassCategory::Medium);
        assert_eq!(category("B274"), JumpMassCategory::Large);
        assert_eq!(category("C729"), JumpMassCategory::Large);
        assert_eq!(category("B041"), JumpMassCategory::Freighter);
        assert_eq!(category("N432"), JumpMassCategory::Capital);

        assert_eq!(
            JumpMassCategory::from_max_jump_mass(5_000_001),
            JumpMassCategory::Medium
        );
        assert_eq!(
            JumpMassCategory::from_max_jump_mass(0),
            JumpMassCategory::Frigate
        );
        assert!(JumpMassCategory::Frigate < JumpMassCategory::Capital);
    }

    #[test]
    fn labels_read_like_the_game() {
        assert_eq!(C13.label(), "Shattered C13");
        assert_eq!(C16.label(), "Drifter C16");
        assert_eq!(HighSec.label(), "High-sec");
        assert_eq!(JumpMassCategory::Freighter.label(), "Freighter");
    }
}
