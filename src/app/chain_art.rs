//! The chain map's artwork, painted rather than loaded so it stays sharp at any zoom:
//! wormhole systems are vortices tinted by class, known space is an orb whose ring
//! shows the security band with a badge for the NPC faction holding it, and
//! connections are styled by mass and age. Every design here is original.

use std::f32::consts::{PI, TAU};
use std::time::Duration;

use eframe::egui::{
    Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2,
};

use super::NomadeerApp;
use crate::config::theme;
use crate::data::factions::{self, Faction};
use crate::data::wormhole_types::WormholeType;
use crate::local_intel;
use crate::model::{Connection, SystemNode};
pub(super) use crate::wormhole::MassState;
use crate::wormhole::{self, LifeState};

/// Below this on-screen radius a system drops its finer art: fewer spiral arms, no
/// halo, and a faction dot instead of a badge.
const FULL_DETAIL_RADIUS: f32 = 22.0;
/// Below this a system is just a disc and its rim.
const REDUCED_DETAIL_RADIUS: f32 = 11.0;
/// How fast a hovered or selected wormhole turns, in radians a second.
const SPIN_RATE: f64 = 0.45;
/// How often a turning wormhole is redrawn: smooth enough, and cheap.
pub(super) const ANIMATION_FRAME: Duration = Duration::from_millis(66);
/// How far each spiral arm winds from the core to the rim.
const SWIRL: f32 = 2.4;

/// How much of a system's art is worth drawing at its size on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Detail {
    Minimal,
    Reduced,
    Full,
}

pub(super) fn detail_for_radius(radius: f32) -> Detail {
    if radius >= FULL_DETAIL_RADIUS {
        Detail::Full
    } else if radius >= REDUCED_DETAIL_RADIUS {
        Detail::Reduced
    } else {
        Detail::Minimal
    }
}

/// What a system looks like on the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NodeLook {
    Wormhole(WormholeClass),
    KnownSpace {
        band: SecurityBand,
        /// The NPC faction holding it; player-held and unclaimed space has none.
        faction: Option<Faction>,
    },
    /// Not located yet.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WormholeClass {
    /// C1 to C6.
    Class(u8),
    Thera,
    /// The shattered C13 frigate holes.
    Shattered,
    /// A Drifter system, C14 to C18, or 0 when only "Drifter" is known.
    Drifter(u8),
    /// W-space whose class is not known yet.
    Unclassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SecurityBand {
    High,
    Low,
    Null,
}

impl WormholeClass {
    /// From the class the map shows, e.g. "C3", "Thera", "C13", "C15" or "Drifter".
    pub(super) fn from_label(label: Option<&str>) -> Self {
        let Some(label) = label else {
            return Self::Unclassified;
        };
        match label {
            "Thera" => Self::Thera,
            "Drifter" => Self::Drifter(0),
            _ => match label.strip_prefix('C').and_then(|n| n.parse::<u8>().ok()) {
                Some(n @ 1..=6) => Self::Class(n),
                Some(13) => Self::Shattered,
                Some(n @ 14..=18) => Self::Drifter(n),
                _ => Self::Unclassified,
            },
        }
    }
}

pub(super) fn node_look(system: &SystemNode) -> NodeLook {
    if local_intel::is_wormhole_space(system) {
        return NodeLook::Wormhole(WormholeClass::from_label(NomadeerApp::wormhole_class(
            system,
        )));
    }
    let band = match NomadeerApp::inferred_system_class(system).as_str() {
        "HS" => SecurityBand::High,
        "LS" => SecurityBand::Low,
        "NS" => SecurityBand::Null,
        _ => return NodeLook::Unknown,
    };
    NodeLook::KnownSpace {
        band,
        faction: factions::faction_for_system(
            system.solar_system_id,
            system.region_id,
            system.constellation_id,
        ),
    }
}

/// The rim colour of a wormhole class: C1 to C6 run from cool blue to hot red.
pub(super) fn wormhole_color(class: WormholeClass) -> Color32 {
    match class {
        WormholeClass::Class(1) => Color32::from_rgb(70, 135, 255),
        WormholeClass::Class(2) => Color32::from_rgb(60, 200, 240),
        WormholeClass::Class(3) => Color32::from_rgb(80, 220, 140),
        WormholeClass::Class(4) => Color32::from_rgb(230, 210, 80),
        WormholeClass::Class(5) => Color32::from_rgb(255, 145, 55),
        WormholeClass::Class(_) => Color32::from_rgb(255, 55, 45),
        WormholeClass::Thera => Color32::from_rgb(195, 115, 255),
        WormholeClass::Shattered => Color32::from_rgb(160, 205, 220),
        WormholeClass::Drifter(_) => Color32::from_rgb(230, 238, 255),
        WormholeClass::Unclassified => Color32::from_rgb(140, 130, 170),
    }
}

/// An orb's colours: lit side, body, shadowed edge and ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OrbPalette {
    pub(super) light: Color32,
    pub(super) body: Color32,
    pub(super) shade: Color32,
    pub(super) ring: Color32,
}

pub(super) fn band_palette(band: SecurityBand) -> OrbPalette {
    match band {
        SecurityBand::High => OrbPalette {
            light: Color32::from_rgb(200, 235, 255),
            body: Color32::from_rgb(70, 150, 230),
            shade: Color32::from_rgb(12, 36, 66),
            ring: Color32::from_rgb(120, 200, 255),
        },
        SecurityBand::Low => OrbPalette {
            light: Color32::from_rgb(255, 225, 160),
            body: Color32::from_rgb(220, 140, 40),
            shade: Color32::from_rgb(66, 36, 8),
            ring: Color32::from_rgb(255, 180, 70),
        },
        SecurityBand::Null => OrbPalette {
            light: Color32::from_rgb(255, 150, 140),
            body: Color32::from_rgb(185, 35, 50),
            shade: Color32::from_rgb(48, 6, 12),
            ring: Color32::from_rgb(255, 75, 90),
        },
    }
}

/// The small mark on a faction's badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FactionGlyph {
    /// A sun with rays: the Amarr and their vassals.
    Sunburst,
    /// Stacked chevrons: the Caldari.
    Chevron,
    /// A moon on its orbit: the Gallente.
    Orbit,
    /// Three slashes: the Minmatar and the Thukker.
    Slashes,
    /// Crossed blades: pirates.
    Blades,
    /// A triangle of three: the Triglavians.
    Trinary,
    /// A shield: CONCORD.
    Shield,
    /// A plain diamond for everyone else.
    Diamond,
}

pub(super) fn faction_glyph(faction: Faction) -> FactionGlyph {
    match faction {
        Faction::TriglavianCollective => FactionGlyph::Trinary,
        Faction::Concord => FactionGlyph::Shield,
        _ => match faction.empire_group() {
            factions::EmpireGroup::Amarr => FactionGlyph::Sunburst,
            factions::EmpireGroup::Caldari => FactionGlyph::Chevron,
            factions::EmpireGroup::Gallente => FactionGlyph::Orbit,
            factions::EmpireGroup::Minmatar => FactionGlyph::Slashes,
            factions::EmpireGroup::Pirate => FactionGlyph::Blades,
            factions::EmpireGroup::Other => FactionGlyph::Diamond,
        },
    }
}

pub(super) fn faction_color(faction: Faction) -> Color32 {
    match faction {
        Faction::AmarrEmpire => Color32::from_rgb(235, 195, 80),
        Faction::AmmatarMandate => Color32::from_rgb(215, 160, 75),
        Faction::KhanidKingdom => Color32::from_rgb(205, 175, 120),
        Faction::CaldariState => Color32::from_rgb(125, 165, 205),
        Faction::GallenteFederation => Color32::from_rgb(90, 205, 130),
        Faction::MinmatarRepublic => Color32::from_rgb(195, 95, 60),
        Faction::ThukkerTribe => Color32::from_rgb(210, 125, 70),
        Faction::JoveEmpire => Color32::from_rgb(150, 170, 235),
        Faction::Concord => Color32::from_rgb(170, 205, 245),
        Faction::EverMore => Color32::from_rgb(220, 205, 255),
        Faction::SanshasNation => Color32::from_rgb(150, 215, 60),
        Faction::GuristasPirates => Color32::from_rgb(235, 200, 40),
        Faction::Serpentis => Color32::from_rgb(165, 120, 235),
        Faction::AngelCartel => Color32::from_rgb(235, 75, 60),
        Faction::BloodRaiderCovenant => Color32::from_rgb(185, 25, 45),
        Faction::DeathlessCircle => Color32::from_rgb(215, 205, 185),
        Faction::Ore => Color32::from_rgb(245, 160, 40),
        Faction::MordusLegion => Color32::from_rgb(150, 160, 170),
        Faction::SistersOfEve => Color32::from_rgb(240, 240, 250),
        Faction::SocietyOfConsciousThought => Color32::from_rgb(130, 205, 225),
        Faction::TriglavianCollective => Color32::from_rgb(225, 40, 55),
    }
}

/// The pip for a system the player marked, if its mark has one.
pub(super) fn status_color(status: &str) -> Option<Color32> {
    match status {
        "home" => Some(Color32::from_rgb(60, 215, 195)),
        "hostile" => Some(Color32::from_rgb(255, 90, 110)),
        "farmable" => Some(Color32::from_rgb(120, 225, 120)),
        _ => None,
    }
}

/// How much a system's art shows: rolled-out systems are faded back.
pub(super) fn status_opacity(status: &str) -> f32 {
    if status == "rolled" { 0.45 } else { 1.0 }
}

/// Each wormhole rests at its own angle, so a chain of them does not look stamped out.
pub(super) fn resting_phase(system_id: &str) -> f32 {
    let hash = system_id.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    (hash % 3600) as f32 / 3600.0 * TAU
}

/// How far a turning wormhole has turned by `time` seconds.
pub(super) fn spin(time: f64) -> f32 {
    -((time * SPIN_RATE) % std::f64::consts::TAU) as f32
}

/// The point on a circle `degrees` clockwise from the right, with y pointing down.
pub(super) fn rim_point(center: Pos2, radius: f32, degrees: f32) -> Pos2 {
    let angle = degrees.to_radians();
    center + vec2(angle.cos(), angle.sin()) * radius
}

pub(super) fn paint_node(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    look: NodeLook,
    phase: f32,
    detail: Detail,
    opacity: f32,
) {
    match look {
        NodeLook::Wormhole(class) => {
            paint_vortex(painter, center, radius, class, phase, detail, opacity)
        }
        NodeLook::KnownSpace { band, .. } => {
            paint_orb(painter, center, radius, band, detail, opacity)
        }
        NodeLook::Unknown => paint_unknown(painter, center, radius, detail, opacity),
    }
}

fn paint_vortex(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    class: WormholeClass,
    phase: f32,
    detail: Detail,
    opacity: f32,
) {
    let fade = |color: Color32| color.gamma_multiply(opacity);
    let rim = wormhole_color(class);
    let drifter = matches!(class, WormholeClass::Drifter(_));
    let body = if drifter {
        Color32::from_rgb(12, 14, 22)
    } else {
        mix(rim, Color32::BLACK, 0.72)
    };
    let core = Color32::from_rgb(3, 5, 9);
    let scale = line_scale(radius);
    if detail == Detail::Minimal {
        painter.circle_filled(center, radius, fade(body));
        painter.circle_stroke(center, radius, Stroke::new(1.5_f32, fade(rim)));
        return;
    }

    let segments = segments_for(radius);
    if detail == Detail::Full {
        let halo = if drifter { 0.18 } else { 0.32 };
        gradient_disc(
            painter,
            center,
            radius,
            Vec2::ZERO,
            &[
                (0.92, rim.gamma_multiply(halo * opacity)),
                (1.35, Color32::TRANSPARENT),
            ],
            segments,
        );
    }
    gradient_disc(
        painter,
        center,
        radius,
        Vec2::ZERO,
        &[
            (0.0, fade(core)),
            (0.26, fade(core)),
            (0.7, fade(body)),
            (0.93, fade(mix(body, rim, 0.5))),
            (1.0, fade(mix(body, rim, 0.3))),
        ],
        segments,
    );

    // Spiral arms, brightest midway and fading into the core and the rim.
    let arms: usize = match (class, detail) {
        (_, Detail::Reduced) => 2,
        (WormholeClass::Thera, _) => 5,
        (WormholeClass::Drifter(_), _) => 2,
        _ => 3,
    };
    let steps: usize = if detail == Detail::Full { 16 } else { 7 };
    let arm_color = if drifter {
        mix(rim, body, 0.35)
    } else {
        mix(rim, Color32::WHITE, 0.2)
    };
    for arm in 0..arms {
        let start = phase + arm as f32 * TAU / arms as f32;
        let point = |t: f32| {
            let angle = start + SWIRL * t;
            center + vec2(angle.cos(), angle.sin()) * radius * (0.22 + 0.72 * t)
        };
        for step in 0..steps {
            let (t0, t1) = (step as f32 / steps as f32, (step + 1) as f32 / steps as f32);
            let strength = (PI * (t0 + t1) / 2.0).sin();
            painter.line_segment(
                [point(t0), point(t1)],
                Stroke::new(
                    (0.8 + 2.2 * t1) * scale,
                    arm_color.gamma_multiply((0.2 + 0.7 * strength) * opacity),
                ),
            );
        }
    }

    match class {
        WormholeClass::Shattered => {
            paint_fractured_rim(painter, center, radius, rim, scale, opacity)
        }
        WormholeClass::Drifter(_) => {
            // A thin bright rim, an iris round the core and four ticks: a watching eye.
            painter.circle_stroke(center, radius, Stroke::new(1.2 * scale, fade(rim)));
            painter.circle_stroke(
                center,
                radius * 0.3,
                Stroke::new(1.2 * scale, rim.gamma_multiply(0.8 * opacity)),
            );
            painter.line_segment(
                [
                    center - vec2(radius * 0.14, 0.0),
                    center + vec2(radius * 0.14, 0.0),
                ],
                Stroke::new(1.6 * scale, fade(rim)),
            );
            if detail == Detail::Full {
                for tick in 0..4 {
                    let angle = phase.to_degrees() * 0.5 + 45.0 + tick as f32 * 90.0;
                    painter.line_segment(
                        [
                            rim_point(center, radius * 1.04, angle),
                            rim_point(center, radius * 1.17, angle),
                        ],
                        Stroke::new(1.5 * scale, fade(rim)),
                    );
                }
            }
        }
        WormholeClass::Thera => {
            painter.circle_stroke(center, radius, Stroke::new(2.0 * scale, fade(rim)));
            painter.circle_stroke(
                center,
                radius * 1.09,
                Stroke::new(1.0 * scale, rim.gamma_multiply(0.45 * opacity)),
            );
        }
        _ => {
            painter.circle_stroke(center, radius, Stroke::new(2.0 * scale, fade(rim)));
        }
    }
}

/// The C13 rim: shards with gaps between them, some nudged in or out, and cracks
/// running in towards the core.
fn paint_fractured_rim(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    rim: Color32,
    scale: f32,
    opacity: f32,
) {
    // Start angle, share of its slot and radius of each shard.
    const SHARDS: [(f32, f32, f32); 7] = [
        (0.10, 0.70, 1.00),
        (0.05, 0.55, 1.05),
        (0.18, 0.62, 0.97),
        (0.00, 0.78, 1.02),
        (0.12, 0.50, 0.95),
        (0.08, 0.66, 1.04),
        (0.15, 0.58, 0.99),
    ];
    let slot = TAU / SHARDS.len() as f32;
    let stroke = Stroke::new(2.0 * scale, rim.gamma_multiply(opacity));
    for (index, (offset, share, reach)) in SHARDS.into_iter().enumerate() {
        let start = (index as f32 + offset) * slot;
        painter.add(Shape::line(
            arc(center, radius * reach, start, start + share * slot, 6),
            stroke,
        ));
    }
    let crack = Stroke::new(1.0 * scale, rim.gamma_multiply(0.6 * opacity));
    for degrees in [35.0_f32, 160.0, 280.0] {
        let points = vec![
            rim_point(center, radius * 0.98, degrees),
            rim_point(center, radius * 0.78, degrees + 9.0),
            rim_point(center, radius * 0.6, degrees - 4.0),
        ];
        painter.add(Shape::line(points, crack));
    }
}

fn paint_orb(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    band: SecurityBand,
    detail: Detail,
    opacity: f32,
) {
    let fade = |color: Color32| color.gamma_multiply(opacity);
    let palette = band_palette(band);
    let scale = line_scale(radius);
    let orb = radius * 0.78;
    if detail == Detail::Minimal {
        painter.circle_filled(center, orb, fade(palette.body));
        painter.circle_stroke(
            center,
            radius * 0.95,
            Stroke::new(1.5_f32, fade(palette.ring)),
        );
        return;
    }

    let segments = segments_for(radius);
    if detail == Detail::Full {
        gradient_disc(
            painter,
            center,
            radius,
            Vec2::ZERO,
            &[
                (0.7, palette.ring.gamma_multiply(0.22 * opacity)),
                (1.2, Color32::TRANSPARENT),
            ],
            segments,
        );
    }
    // Lit from the upper left, like a planet against its star.
    gradient_disc(
        painter,
        center,
        orb,
        vec2(-0.35, -0.4) * orb,
        &[
            (0.0, fade(palette.light)),
            (0.45, fade(palette.body)),
            (0.85, fade(mix(palette.body, palette.shade, 0.6))),
            (1.0, fade(palette.shade)),
        ],
        segments,
    );

    let ring = Stroke::new(2.0 * scale, fade(palette.ring));
    match band {
        // A calm, unbroken ring with a faint orbit outside it.
        SecurityBand::High => {
            painter.circle_stroke(center, radius * 0.95, ring);
            if detail == Detail::Full {
                painter.circle_stroke(
                    center,
                    radius * 1.08,
                    Stroke::new(1.0 * scale, palette.ring.gamma_multiply(0.3 * opacity)),
                );
            }
        }
        // A broken ring.
        SecurityBand::Low => {
            let dashes = if detail == Detail::Full { 14 } else { 10 };
            let slot = TAU / dashes as f32;
            for dash in 0..dashes {
                let start = dash as f32 * slot - PI / 2.0;
                painter.add(Shape::line(
                    arc(center, radius * 0.95, start, start + slot * 0.6, 4),
                    ring,
                ));
            }
        }
        // A sharp, spiked ring.
        SecurityBand::Null => {
            let points = if detail == Detail::Full { 12 } else { 8 };
            let spikes = (0..points * 2)
                .map(|index| {
                    let reach = if index % 2 == 0 { 1.07 } else { 0.9 };
                    rim_point(
                        center,
                        radius * reach,
                        index as f32 * 180.0 / points as f32 - 90.0,
                    )
                })
                .collect();
            painter.add(Shape::closed_line(spikes, ring));
        }
    }
}

fn paint_unknown(painter: &Painter, center: Pos2, radius: f32, detail: Detail, opacity: f32) {
    let muted = theme::muted_text().gamma_multiply(0.6 * opacity);
    painter.circle_filled(
        center,
        radius * 0.8,
        Color32::from_rgb(20, 30, 36).gamma_multiply(opacity),
    );
    if detail == Detail::Minimal {
        painter.circle_stroke(center, radius * 0.9, Stroke::new(1.0_f32, muted));
        return;
    }
    let stroke = Stroke::new(1.5 * line_scale(radius), muted);
    let slot = TAU / 12.0;
    for dash in 0..12 {
        let start = dash as f32 * slot;
        painter.add(Shape::line(
            arc(center, radius * 0.9, start, start + slot * 0.5, 3),
            stroke,
        ));
    }
    painter.text(
        center,
        Align2::CENTER_CENTER,
        "?",
        FontId::proportional(radius * 0.7),
        muted,
    );
}

/// A faction's badge: a dark disc ringed and marked in its colour, or at small sizes
/// a dot of the colour alone.
pub(super) fn paint_faction_badge(
    painter: &Painter,
    center: Pos2,
    size: f32,
    faction: Faction,
    detail: Detail,
    opacity: f32,
) {
    let color = faction_color(faction).gamma_multiply(opacity);
    match detail {
        Detail::Minimal => {}
        Detail::Reduced => {
            painter.circle_filled(center, size * 0.55, color);
            painter.circle_stroke(
                center,
                size * 0.55,
                Stroke::new(1.0_f32, theme::panel_background().gamma_multiply(opacity)),
            );
        }
        Detail::Full => {
            painter.circle_filled(
                center,
                size,
                theme::panel_background().gamma_multiply(0.94 * opacity),
            );
            painter.circle_stroke(center, size, Stroke::new(1.2_f32, color));
            paint_glyph(painter, center, size * 0.62, faction_glyph(faction), color);
        }
    }
}

fn paint_glyph(painter: &Painter, center: Pos2, size: f32, glyph: FactionGlyph, color: Color32) {
    let stroke = Stroke::new((size * 0.22).clamp(1.0, 2.2), color);
    let at = |x: f32, y: f32| center + vec2(x, y) * size;
    match glyph {
        FactionGlyph::Sunburst => {
            painter.circle_filled(center, size * 0.38, color);
            for ray in 0..8 {
                let reach = if ray % 2 == 0 { 1.0 } else { 0.78 };
                let degrees = ray as f32 * 45.0;
                painter.line_segment(
                    [
                        rim_point(center, size * 0.56, degrees),
                        rim_point(center, size * reach, degrees),
                    ],
                    stroke,
                );
            }
        }
        FactionGlyph::Chevron => {
            for lift in [-0.45, 0.25] {
                painter.add(Shape::line(
                    vec![
                        at(-0.8, lift - 0.3),
                        at(0.0, lift + 0.35),
                        at(0.8, lift - 0.3),
                    ],
                    stroke,
                ));
            }
        }
        FactionGlyph::Orbit => {
            painter.circle_stroke(center, size * 0.85, stroke);
            painter.circle_filled(center, size * 0.25, color);
            painter.circle_filled(rim_point(center, size * 0.85, -45.0), size * 0.28, color);
        }
        FactionGlyph::Slashes => {
            for x in [-0.55, 0.0, 0.55] {
                painter.line_segment([at(x - 0.3, 0.8), at(x + 0.3, -0.8)], stroke);
            }
        }
        FactionGlyph::Blades => {
            painter.line_segment([at(-0.8, -0.8), at(0.8, 0.8)], stroke);
            painter.line_segment([at(0.8, -0.8), at(-0.8, 0.8)], stroke);
            painter.line_segment([at(-0.95, 0.45), at(-0.45, 0.95)], stroke);
            painter.line_segment([at(0.95, 0.45), at(0.45, 0.95)], stroke);
        }
        FactionGlyph::Trinary => {
            let corners =
                [-90.0_f32, 30.0, 150.0].map(|degrees| rim_point(center, size * 0.85, degrees));
            painter.add(Shape::closed_line(corners.to_vec(), stroke));
            for corner in corners {
                painter.circle_filled(corner, size * 0.24, color);
            }
        }
        FactionGlyph::Shield => {
            painter.add(Shape::closed_line(
                vec![
                    at(-0.75, -0.8),
                    at(0.75, -0.8),
                    at(0.75, 0.05),
                    at(0.0, 0.9),
                    at(-0.75, 0.05),
                ],
                stroke,
            ));
        }
        FactionGlyph::Diamond => {
            painter.add(Shape::convex_polygon(
                vec![at(0.0, -0.85), at(0.7, 0.0), at(0.0, 0.85), at(-0.7, 0.0)],
                color,
                Stroke::NONE,
            ));
        }
    }
}

/// A soft glow and a crisp ring round a system, e.g. the selected one.
pub(super) fn paint_glow_ring(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    color: Color32,
    width: f32,
) {
    gradient_disc(
        painter,
        center,
        radius,
        Vec2::ZERO,
        &[
            (1.0, Color32::TRANSPARENT),
            (1.1, color.gamma_multiply(0.45)),
            (1.45, Color32::TRANSPARENT),
        ],
        segments_for(radius),
    );
    painter.circle_stroke(center, radius * 1.1, Stroke::new(width, color));
}

/// How many of the player's characters are in a system, in the character colour.
pub(super) fn paint_count_badge(painter: &Painter, center: Pos2, size: f32, count: usize) {
    painter.circle_filled(center, size, theme::character_text());
    painter.circle_stroke(
        center,
        size,
        Stroke::new(1.5_f32, theme::panel_background()),
    );
    let text = if count > 9 {
        "9+".to_owned()
    } else {
        count.to_string()
    };
    painter.text(
        center,
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(size * 1.25),
        theme::panel_background(),
    );
}

/// A font for text that scales with the zoom, rounded to half a point. Each size
/// egui sees is kept for the session, so smooth zooming must not make a new one
/// every frame.
pub(super) fn label_font(size: f32) -> FontId {
    FontId::proportional(((size * 2.0).round() / 2.0).max(1.0))
}

/// Text with a dark shadow, so it reads over the art.
pub(super) fn shadowed_text(
    painter: &Painter,
    pos: Pos2,
    align: Align2,
    text: &str,
    font: FontId,
    color: Color32,
) {
    let shadow = Color32::from_black_alpha((f32::from(color.a()) * 0.85) as u8);
    painter.text(pos + vec2(1.0, 1.0), align, text, font.clone(), shadow);
    painter.text(pos, align, text, font, color);
}

/// How a connection is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ConnectionStyle {
    pub(super) mass: MassState,
    pub(super) end_of_life: bool,
    /// The wormhole's type when known: its own code, or K162 when only the exit
    /// side is known.
    pub(super) wormhole: Option<&'static WormholeType>,
    /// Hours left at most, once it is marked end of life with a time.
    pub(super) eol_hours_left: Option<u64>,
}

/// A connection's line: its width at 100% zoom, colour, and dash and gap lengths
/// when it is broken.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct EdgeStroke {
    pub(super) width: f32,
    pub(super) color: Color32,
    pub(super) dash: Option<(f32, f32)>,
}

impl ConnectionStyle {
    pub(super) fn of(connection: &Connection, now: u64) -> Self {
        let end_of_life = wormhole::life(connection) == LifeState::EndOfLife;
        Self {
            mass: wormhole::mass(connection),
            end_of_life,
            wormhole: wormhole::label_type(connection),
            eol_hours_left: end_of_life
                .then(|| wormhole::hours_left(connection, now))
                .flatten(),
        }
    }

    /// Thick and bright while stable, thinner and hotter as mass goes, broken into
    /// dashes at end of life. A hole nobody has checked is thinner and grey, so it is
    /// never taken for a stable one.
    pub(super) fn stroke(&self) -> EdgeStroke {
        let (width, color) = match self.mass {
            MassState::Unknown => (2.0, theme::muted_text().gamma_multiply(0.8)),
            MassState::Stable => (3.0, theme::accent()),
            MassState::Reduced => (2.2, Color32::from_rgb(255, 180, 70)),
            MassState::Critical => (1.6, Color32::from_rgb(255, 75, 95)),
        };
        EdgeStroke {
            width,
            color,
            dash: self.end_of_life.then_some((7.0, 5.0)),
        }
    }

    /// The text in the label slot, e.g. "H296 → C5 · Capital", "K162" for an exit,
    /// with "EOL ~3h" once it is dying.
    pub(super) fn label(&self) -> Option<String> {
        let wormhole = self
            .wormhole
            .map(|wormhole| match wormhole.jump_mass_category() {
                Some(size) => format!(
                    "{} → {} · {}",
                    wormhole.code,
                    wormhole.target.label(),
                    size.label()
                ),
                None => wormhole.code.to_owned(),
            });
        let dying = self.end_of_life.then(|| match self.eol_hours_left {
            Some(0) => "EOL · overdue".to_owned(),
            Some(hours) => format!("EOL ~{hours}h"),
            None => "EOL".to_owned(),
        });
        match (wormhole, dying) {
            (Some(wormhole), Some(dying)) => Some(format!("{wormhole} · {dying}")),
            (wormhole, dying) => wormhole.or(dying),
        }
    }
}

/// Draws a connection from `a` to `b`. `line_scale` sizes the line and `text_scale`
/// the label, which shows at `label_opacity`.
pub(super) fn paint_connection(
    painter: &Painter,
    [a, b]: [Pos2; 2],
    style: &ConnectionStyle,
    line_scale: f32,
    text_scale: f32,
    hovered: bool,
    label_opacity: f32,
) {
    let edge = style.stroke();
    let (width, color) = if hovered {
        (edge.width + 1.5, Color32::WHITE)
    } else {
        (edge.width, edge.color)
    };
    let stroke = Stroke::new(width * line_scale, color);
    painter.line_segment(
        [a, b],
        Stroke::new(width * line_scale * 2.6, color.gamma_multiply(0.14)),
    );
    match edge.dash {
        Some((dash, gap)) => painter.extend(Shape::dashed_line(
            &[a, b],
            stroke,
            dash * line_scale,
            gap * line_scale,
        )),
        None => {
            painter.line_segment([a, b], stroke);
        }
    }

    if label_opacity > 0.0
        && let Some(label) = style.label()
    {
        let galley = painter.layout_no_wrap(
            label,
            label_font(11.0 * text_scale),
            Color32::WHITE.gamma_multiply(label_opacity),
        );
        let pill = Rect::from_center_size(a.lerp(b, 0.5), galley.size() + vec2(8.0, 4.0));
        painter.rect_filled(
            pill,
            4.0,
            theme::panel_background().gamma_multiply(0.9 * label_opacity),
        );
        painter.rect_stroke(
            pill,
            4.0,
            Stroke::new(1.0_f32, edge.color.gamma_multiply(label_opacity)),
            eframe::egui::StrokeKind::Inside,
        );
        painter.galley(pill.min + vec2(4.0, 2.0), galley, Color32::WHITE);
    }
}

/// One line of the map's legend.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LegendIcon {
    Heading,
    /// C1 to C6 side by side.
    ClassRamp,
    Node(NodeLook),
    Badge(Faction),
    Edge(ConnectionStyle),
    Selected,
    Characters,
    Marks,
}

impl LegendIcon {
    const fn wormhole(class: WormholeClass) -> Self {
        Self::Node(NodeLook::Wormhole(class))
    }

    const fn known_space(band: SecurityBand) -> Self {
        Self::Node(NodeLook::KnownSpace {
            band,
            faction: None,
        })
    }

    const fn edge(mass: MassState, end_of_life: bool) -> Self {
        Self::Edge(ConnectionStyle {
            mass,
            end_of_life,
            wormhole: None,
            eol_hours_left: None,
        })
    }
}

const LEGEND: &[(LegendIcon, &str)] = &[
    (LegendIcon::Heading, "Wormholes"),
    (LegendIcon::ClassRamp, "C1 to C6"),
    (LegendIcon::wormhole(WormholeClass::Thera), "Thera"),
    (
        LegendIcon::wormhole(WormholeClass::Shattered),
        "Shattered C13",
    ),
    (
        LegendIcon::wormhole(WormholeClass::Drifter(0)),
        "Drifter C14-18",
    ),
    (
        LegendIcon::wormhole(WormholeClass::Unclassified),
        "Class not known",
    ),
    (LegendIcon::Heading, "Known space"),
    (LegendIcon::known_space(SecurityBand::High), "High-sec"),
    (LegendIcon::known_space(SecurityBand::Low), "Low-sec"),
    (LegendIcon::known_space(SecurityBand::Null), "Null-sec"),
    (LegendIcon::Node(NodeLook::Unknown), "Not located"),
    (LegendIcon::Heading, "Held by"),
    (LegendIcon::Badge(Faction::AmarrEmpire), "Amarr"),
    (LegendIcon::Badge(Faction::CaldariState), "Caldari"),
    (LegendIcon::Badge(Faction::GallenteFederation), "Gallente"),
    (LegendIcon::Badge(Faction::MinmatarRepublic), "Minmatar"),
    (LegendIcon::Badge(Faction::AngelCartel), "Pirates"),
    (
        LegendIcon::Badge(Faction::TriglavianCollective),
        "Triglavians",
    ),
    (LegendIcon::Badge(Faction::Concord), "CONCORD"),
    (LegendIcon::Badge(Faction::Ore), "Other NPCs"),
    (LegendIcon::Heading, "Connections"),
    (
        LegendIcon::edge(MassState::Unknown, false),
        "Mass not checked",
    ),
    (LegendIcon::edge(MassState::Stable, false), "Stable"),
    (LegendIcon::edge(MassState::Reduced, false), "Reduced"),
    (LegendIcon::edge(MassState::Critical, false), "Critical"),
    (LegendIcon::edge(MassState::Stable, true), "End of life"),
    (LegendIcon::Heading, "Markers"),
    (LegendIcon::Selected, "Selected"),
    (LegendIcon::Characters, "Your characters"),
    (LegendIcon::Marks, "Home, hostile, farm"),
];

const LEGEND_ROW: f32 = 19.0;
const LEGEND_COLUMN: f32 = 136.0;
const LEGEND_PADDING: f32 = 8.0;
/// A tall map still lays the legend out in columns rather than one long list.
const LEGEND_MAX_ROWS: usize = 10;

/// Puts each entry in columns of up to `rows`, as (column, row). A heading is
/// never left at the foot of a column without its first entry.
fn legend_slots(headings: &[bool], rows: usize) -> Vec<(usize, usize)> {
    let rows = rows.max(2);
    let (mut column, mut row) = (0, 0);
    headings
        .iter()
        .map(|&heading| {
            if row >= rows || (heading && row + 1 >= rows) {
                column += 1;
                row = 0;
            }
            row += 1;
            (column, row - 1)
        })
        .collect()
}

/// The legend laid out to fit `max_height` points.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Legend {
    slots: Vec<(usize, usize)>,
    pub(super) size: Vec2,
}

impl Legend {
    pub(super) fn fit(max_height: f32) -> Self {
        let rows = (((max_height - 2.0 * LEGEND_PADDING) / LEGEND_ROW)
            .floor()
            .max(0.0) as usize)
            .min(LEGEND_MAX_ROWS);
        let headings = LEGEND
            .iter()
            .map(|(icon, _)| *icon == LegendIcon::Heading)
            .collect::<Vec<_>>();
        let slots = legend_slots(&headings, rows);
        let columns = slots
            .iter()
            .map(|(column, _)| column + 1)
            .max()
            .unwrap_or(1);
        let used_rows = slots.iter().map(|(_, row)| row + 1).max().unwrap_or(1);
        Self {
            slots,
            size: vec2(
                columns as f32 * LEGEND_COLUMN + 2.0 * LEGEND_PADDING,
                used_rows as f32 * LEGEND_ROW + 2.0 * LEGEND_PADDING,
            ),
        }
    }

    pub(super) fn paint(&self, painter: &Painter, rect: Rect) {
        painter.rect_filled(rect, 8.0, theme::panel_background().gamma_multiply(0.94));
        painter.rect_stroke(
            rect,
            8.0,
            Stroke::new(1.0_f32, theme::panel_stroke()),
            eframe::egui::StrokeKind::Inside,
        );
        let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
        for (&(icon, label), &(column, row)) in LEGEND.iter().zip(&self.slots) {
            let top_left = rect.min
                + vec2(
                    LEGEND_PADDING + column as f32 * LEGEND_COLUMN,
                    LEGEND_PADDING + row as f32 * LEGEND_ROW,
                );
            let middle = top_left.y + LEGEND_ROW / 2.0;
            if icon == LegendIcon::Heading {
                painter.text(
                    pos2(top_left.x, middle),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(12.0),
                    theme::accent(),
                );
                continue;
            }
            let icon_center = pos2(top_left.x + 10.0, middle);
            let text_left = match icon {
                LegendIcon::ClassRamp => {
                    for class in 1..=6_u8 {
                        let center = pos2(top_left.x + 6.0 + f32::from(class - 1) * 13.0, middle);
                        paint_node(
                            &painter,
                            center,
                            6.0,
                            NodeLook::Wormhole(WormholeClass::Class(class)),
                            0.0,
                            Detail::Minimal,
                            1.0,
                        );
                    }
                    top_left.x + 82.0
                }
                LegendIcon::Node(look) => {
                    paint_node(&painter, icon_center, 8.0, look, 0.6, Detail::Reduced, 1.0);
                    top_left.x + 24.0
                }
                LegendIcon::Badge(faction) => {
                    paint_faction_badge(&painter, icon_center, 7.5, faction, Detail::Full, 1.0);
                    top_left.x + 24.0
                }
                LegendIcon::Edge(style) => {
                    paint_connection(
                        &painter,
                        [icon_center - vec2(9.0, 0.0), icon_center + vec2(9.0, 0.0)],
                        &style,
                        0.7,
                        1.0,
                        false,
                        0.0,
                    );
                    top_left.x + 24.0
                }
                LegendIcon::Selected => {
                    painter.circle_filled(icon_center, 5.0, theme::panel_stroke());
                    paint_glow_ring(&painter, icon_center, 5.0, theme::accent(), 1.5);
                    top_left.x + 24.0
                }
                LegendIcon::Characters => {
                    paint_count_badge(&painter, icon_center, 6.5, 2);
                    top_left.x + 24.0
                }
                LegendIcon::Marks => {
                    for (index, status) in ["home", "hostile", "farmable"].into_iter().enumerate() {
                        if let Some(color) = status_color(status) {
                            painter.circle_filled(
                                pos2(top_left.x + 3.0 + index as f32 * 8.0, middle),
                                3.0,
                                color,
                            );
                        }
                    }
                    top_left.x + 24.0
                }
                LegendIcon::Heading => unreachable!("headings are drawn above"),
            };
            painter.text(
                pos2(text_left, middle),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(12.0),
                theme::muted_text(),
            );
        }
    }
}

/// Fills a disc from `center` outwards through colour `stops`, each a share of
/// `radius` and a colour, as rings of a mesh. `focus` moves the innermost point off
/// centre, for a lit sphere; stops beyond 1.0 reach outside the disc, for a halo.
fn gradient_disc(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    focus: Vec2,
    stops: &[(f32, Color32)],
    segments: usize,
) {
    if stops.len() < 2 || radius <= 0.0 {
        return;
    }
    let mut mesh = Mesh::default();
    let focus_point = center + focus;
    for &(reach, color) in stops {
        for segment in 0..segments {
            let angle = segment as f32 / segments as f32 * TAU;
            let edge = center + vec2(angle.cos(), angle.sin()) * radius;
            let pos = if reach <= 1.0 {
                focus_point.lerp(edge, reach)
            } else {
                center + (edge - center) * reach
            };
            mesh.colored_vertex(pos, color);
        }
    }
    let segments = segments as u32;
    for ring in 0..stops.len() as u32 - 1 {
        let (inner, outer) = (ring * segments, (ring + 1) * segments);
        for segment in 0..segments {
            let next = (segment + 1) % segments;
            mesh.add_triangle(inner + segment, outer + segment, outer + next);
            mesh.add_triangle(inner + segment, outer + next, inner + next);
        }
    }
    painter.add(Shape::mesh(mesh));
}

fn arc(center: Pos2, radius: f32, from: f32, to: f32, steps: usize) -> Vec<Pos2> {
    (0..=steps)
        .map(|step| {
            let angle = from + (to - from) * step as f32 / steps as f32;
            center + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect()
}

/// Mesh rings smooth enough for the size without wasting triangles.
fn segments_for(radius: f32) -> usize {
    ((radius * 0.9) as usize).clamp(16, 56)
}

/// Lines in the art thicken and thin with the node, within reason.
fn line_scale(radius: f32) -> f32 {
    (radius / super::chain_view::NODE_RADIUS).clamp(0.4, 1.6)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::wormhole_types;

    fn system(
        solar_system_id: u64,
        region_id: u64,
        constellation_id: u64,
        security: f32,
    ) -> SystemNode {
        SystemNode {
            id: format!("sys_{solar_system_id}"),
            solar_system_id,
            name: String::new(),
            class: String::new(),
            security_status: security,
            constellation_id,
            constellation_name: String::new(),
            region_id,
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

    /// The hue as a colour picker shows it, in degrees.
    fn hue(color: Color32) -> f32 {
        let [r, g, b] = [color.r(), color.g(), color.b()].map(|c| f32::from(c) / 255.0);
        let max = r.max(g).max(b);
        let range = max - r.min(g).min(b);
        if range == 0.0 {
            return 0.0;
        }
        let sector = if max == r {
            ((g - b) / range).rem_euclid(6.0)
        } else if max == g {
            (b - r) / range + 2.0
        } else {
            (r - g) / range + 4.0
        };
        sector * 60.0
    }

    fn connection(mass: &str, life: &str) -> Connection {
        Connection {
            id: "c".to_owned(),
            from_system: "a".to_owned(),
            to_system: "b".to_owned(),
            signature_from: String::new(),
            signature_to: String::new(),
            mass_status: mass.to_owned(),
            life_status: life.to_owned(),
            ship_size: String::new(),
            notes: String::new(),
            created_at: 0,
            ..Connection::default()
        }
    }

    #[test]
    fn wormhole_classes_are_read_from_the_map_label() {
        assert_eq!(
            WormholeClass::from_label(Some("C1")),
            WormholeClass::Class(1)
        );
        assert_eq!(
            WormholeClass::from_label(Some("C6")),
            WormholeClass::Class(6)
        );
        assert_eq!(
            WormholeClass::from_label(Some("Thera")),
            WormholeClass::Thera
        );
        assert_eq!(
            WormholeClass::from_label(Some("C13")),
            WormholeClass::Shattered
        );
        assert_eq!(
            WormholeClass::from_label(Some("C16")),
            WormholeClass::Drifter(16)
        );
        assert_eq!(
            WormholeClass::from_label(Some("Drifter")),
            WormholeClass::Drifter(0)
        );
        assert_eq!(
            WormholeClass::from_label(Some("C7")),
            WormholeClass::Unclassified
        );
        assert_eq!(WormholeClass::from_label(None), WormholeClass::Unclassified);
    }

    #[test]
    fn systems_get_the_look_of_their_space() {
        // A C3 entered by hand, before ESI has filled anything in.
        let mut c3 = system(0, 0, 0, 0.0);
        c3.class = "C3".to_owned();
        assert_eq!(node_look(&c3), NodeLook::Wormhole(WormholeClass::Class(3)));

        // Thera by its ID.
        assert_eq!(
            node_look(&system(31_000_005, 11_000_032, 21_000_334, -1.0)),
            NodeLook::Wormhole(WormholeClass::Thera)
        );

        // Jita: high-sec, held by CONCORD.
        assert_eq!(
            node_look(&system(30_000_142, 10_000_002, 20_000_020, 0.95)),
            NodeLook::KnownSpace {
                band: SecurityBand::High,
                faction: Some(Faction::Concord)
            }
        );

        // Player-held null-sec has no faction badge.
        let NodeLook::KnownSpace { band, faction } =
            node_look(&system(30_004_759, 10_000_060, 20_000_696, -0.4))
        else {
            panic!("1DQ1-A is known space");
        };
        assert_eq!((band, faction), (SecurityBand::Null, None));

        assert_eq!(node_look(&system(0, 0, 0, 0.0)), NodeLook::Unknown);
    }

    #[test]
    fn security_bands_follow_the_map_labels() {
        let band = |security| match node_look(&system(30_000_142, 10_000_002, 20_000_020, security))
        {
            NodeLook::KnownSpace { band, .. } => band,
            look => panic!("{look:?}"),
        };
        assert_eq!(band(1.0), SecurityBand::High);
        assert_eq!(band(0.45), SecurityBand::High);
        assert_eq!(band(0.44), SecurityBand::Low);
        assert_eq!(band(0.1), SecurityBand::Low);
        assert_eq!(band(0.0), SecurityBand::Null);
        assert_eq!(band(-0.8), SecurityBand::Null);
    }

    #[test]
    fn wormhole_classes_run_from_cool_to_hot() {
        let hues = (1..=6)
            .map(|class| hue(wormhole_color(WormholeClass::Class(class))))
            .collect::<Vec<_>>();
        assert!(hues.windows(2).all(|pair| pair[0] > pair[1]), "{hues:?}");
        // The special classes stand apart from the ramp and each other.
        let special = [
            WormholeClass::Thera,
            WormholeClass::Shattered,
            WormholeClass::Drifter(14),
            WormholeClass::Unclassified,
        ]
        .map(wormhole_color);
        for (index, color) in special.iter().enumerate() {
            assert!(!special[index + 1..].contains(color));
            assert!((1..=6).all(|class| wormhole_color(WormholeClass::Class(class)) != *color));
        }
    }

    #[test]
    fn security_bands_have_their_own_colours() {
        let rings = [SecurityBand::High, SecurityBand::Low, SecurityBand::Null]
            .map(|band| hue(band_palette(band).ring));
        // High-sec is cool, low-sec amber and null-sec red.
        assert!((180.0..240.0).contains(&rings[0]), "{rings:?}");
        assert!((25.0..50.0).contains(&rings[1]), "{rings:?}");
        assert!(rings[2] < 10.0 || rings[2] > 345.0, "{rings:?}");
    }

    #[test]
    fn factions_get_their_own_glyph_and_colour() {
        assert_eq!(faction_glyph(Faction::AmarrEmpire), FactionGlyph::Sunburst);
        assert_eq!(
            faction_glyph(Faction::KhanidKingdom),
            FactionGlyph::Sunburst
        );
        assert_eq!(faction_glyph(Faction::CaldariState), FactionGlyph::Chevron);
        assert_eq!(
            faction_glyph(Faction::GallenteFederation),
            FactionGlyph::Orbit
        );
        assert_eq!(faction_glyph(Faction::ThukkerTribe), FactionGlyph::Slashes);
        assert_eq!(
            faction_glyph(Faction::GuristasPirates),
            FactionGlyph::Blades
        );
        assert_eq!(
            faction_glyph(Faction::DeathlessCircle),
            FactionGlyph::Blades
        );
        assert_eq!(
            faction_glyph(Faction::TriglavianCollective),
            FactionGlyph::Trinary
        );
        assert_eq!(faction_glyph(Faction::Concord), FactionGlyph::Shield);
        assert_eq!(faction_glyph(Faction::Ore), FactionGlyph::Diamond);

        // Amarr gold, Caldari steel blue, Gallente green, Minmatar rust.
        assert!((35.0..55.0).contains(&hue(faction_color(Faction::AmarrEmpire))));
        assert!((195.0..225.0).contains(&hue(faction_color(Faction::CaldariState))));
        assert!((120.0..160.0).contains(&hue(faction_color(Faction::GallenteFederation))));
        assert!((10.0..25.0).contains(&hue(faction_color(Faction::MinmatarRepublic))));
        // Pirates each wear their own colour.
        let pirates = [
            Faction::SanshasNation,
            Faction::GuristasPirates,
            Faction::Serpentis,
            Faction::AngelCartel,
            Faction::BloodRaiderCovenant,
            Faction::DeathlessCircle,
        ]
        .map(faction_color);
        for (index, color) in pirates.iter().enumerate() {
            assert!(!pirates[index + 1..].contains(color));
        }
    }

    #[test]
    fn detail_drops_as_systems_shrink() {
        assert_eq!(detail_for_radius(38.0), Detail::Full);
        assert_eq!(detail_for_radius(FULL_DETAIL_RADIUS), Detail::Full);
        assert_eq!(detail_for_radius(15.0), Detail::Reduced);
        assert_eq!(detail_for_radius(9.5), Detail::Minimal);
    }

    #[test]
    fn connections_are_styled_by_mass_and_age() {
        let style = ConnectionStyle::of(&connection("stable", "unknown"), 0);
        assert_eq!(style.mass, MassState::Stable);
        assert!(!style.end_of_life);
        assert_eq!(style.stroke().dash, None);

        let reduced = ConnectionStyle::of(&connection("destab", ""), 0).stroke();
        let critical = ConnectionStyle::of(&connection("Critical", ""), 0).stroke();
        assert!(style.stroke().width > reduced.width && reduced.width > critical.width);
        assert_ne!(reduced.color, critical.color);
        let unknown = ConnectionStyle::of(&connection("unknown", ""), 0);
        assert_eq!(unknown.mass, MassState::Unknown);
        // A hole nobody checked does not pass for a stable one, and has a legend line.
        assert!(unknown.stroke().width < style.stroke().width);
        assert_ne!(unknown.stroke().color, style.stroke().color);
        assert!(LEGEND.contains(&(
            LegendIcon::edge(MassState::Unknown, false),
            "Mass not checked"
        )));

        let dying = ConnectionStyle::of(&connection("", "end_of_life"), 0);
        assert!(dying.end_of_life);
        assert!(dying.stroke().dash.is_some());
        assert!(ConnectionStyle::of(&connection("", "EOL"), 0).end_of_life);
    }

    #[test]
    fn the_label_slot_names_the_wormhole() {
        let mut style = ConnectionStyle::of(&connection("", ""), 0);
        assert_eq!(style.label(), None);
        style.wormhole = wormhole_types::by_code("H296");
        assert_eq!(style.label().as_deref(), Some("H296 → C5 · Capital"));
        style.wormhole = wormhole_types::by_code("K162");
        assert_eq!(style.label().as_deref(), Some("K162"));
    }

    #[test]
    fn connections_show_their_type_and_time_left() {
        const NOW: u64 = 1_760_000_000;
        let mut marked = connection("reduced", "");
        marked.created_at = NOW;
        wormhole::set_type_on_side(&mut marked, "a", wormhole_types::by_code("B274").unwrap());
        let style = ConnectionStyle::of(&marked, NOW);
        assert_eq!(style.mass, MassState::Reduced);
        assert_eq!(style.label().as_deref(), Some("B274 → High-sec · Large"));

        wormhole::mark_eol(&mut marked, NOW);
        let style = ConnectionStyle::of(&marked, NOW + 3_600 + 60);
        assert!(style.end_of_life);
        assert!(style.stroke().dash.is_some());
        assert_eq!(
            style.label().as_deref(),
            Some("B274 → High-sec · Large · EOL ~3h")
        );
        assert_eq!(
            ConnectionStyle::of(&marked, NOW + 5 * 3_600)
                .label()
                .as_deref(),
            Some("B274 → High-sec · Large · EOL · overdue")
        );

        // Only the exit side known, or nothing but the mark.
        wormhole::set_type_on_side(&mut marked, "a", wormhole_types::by_code("K162").unwrap());
        assert_eq!(
            ConnectionStyle::of(&marked, NOW).label().as_deref(),
            Some("K162 · EOL ~4h")
        );
        wormhole::clear_type(&mut marked);
        assert_eq!(
            ConnectionStyle::of(&marked, NOW).label().as_deref(),
            Some("EOL ~4h")
        );
        assert_eq!(
            ConnectionStyle::of(&connection("", "eol"), NOW)
                .label()
                .as_deref(),
            Some("EOL")
        );
    }

    #[test]
    fn the_legend_keeps_headings_with_their_entries() {
        let headings = LEGEND
            .iter()
            .map(|(icon, _)| *icon == LegendIcon::Heading)
            .collect::<Vec<_>>();
        for rows in [2, 4, 7, 10] {
            let slots = legend_slots(&headings, rows);
            assert_eq!(slots.len(), LEGEND.len());
            for (index, &(column, row)) in slots.iter().enumerate() {
                assert!(row < rows);
                if headings[index] {
                    assert_eq!(slots[index + 1], (column, row + 1), "{rows} rows");
                }
            }
        }
        // A short map spreads it wider, a tall one stops at its row limit.
        assert!(Legend::fit(120.0).size.x > Legend::fit(400.0).size.x);
        assert_eq!(Legend::fit(2000.0), Legend::fit(4000.0));
    }

    #[test]
    fn every_look_paints_at_every_size() {
        let ctx = eframe::egui::Context::default();
        let _ = ctx.run(eframe::egui::RawInput::default(), |ctx| {
            let painter = Painter::new(
                ctx.clone(),
                eframe::egui::LayerId::background(),
                Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)),
            );
            let looks = LEGEND.iter().filter_map(|(icon, _)| match icon {
                LegendIcon::Node(look) => Some(*look),
                _ => None,
            });
            let looks =
                looks.chain((1..=6).map(|class| NodeLook::Wormhole(WormholeClass::Class(class))));
            for look in looks {
                for radius in [4.0, 15.0, 38.0, 114.0] {
                    let detail = detail_for_radius(radius);
                    paint_node(
                        &painter,
                        pos2(100.0, 100.0),
                        radius,
                        look,
                        1.0,
                        detail,
                        0.45,
                    );
                }
            }
            for (icon, _) in LEGEND {
                if let LegendIcon::Badge(faction) = icon {
                    for detail in [Detail::Minimal, Detail::Reduced, Detail::Full] {
                        paint_faction_badge(
                            &painter,
                            pos2(50.0, 50.0),
                            11.0,
                            *faction,
                            detail,
                            1.0,
                        );
                    }
                }
            }
            let style = ConnectionStyle {
                mass: MassState::Critical,
                end_of_life: true,
                wormhole: crate::data::wormhole_types::by_code("H296"),
                eol_hours_left: Some(2),
            };
            paint_connection(
                &painter,
                [pos2(0.0, 0.0), pos2(200.0, 80.0)],
                &style,
                1.0,
                1.0,
                true,
                1.0,
            );
            paint_glow_ring(&painter, pos2(300.0, 300.0), 38.0, theme::accent(), 3.0);
            paint_count_badge(&painter, pos2(300.0, 300.0), 10.0, 12);
            for height in [0.0, 120.0, 600.0] {
                let legend = Legend::fit(height);
                legend.paint(&painter, Rect::from_min_size(Pos2::ZERO, legend.size));
            }
        });
    }

    #[test]
    fn zoomed_text_comes_in_a_few_sizes() {
        assert_eq!(label_font(12.0 * 1.01), label_font(12.0 * 1.015));
        assert_eq!(label_font(12.24).size, 12.0);
        assert_eq!(label_font(12.3).size, 12.5);
        assert!(label_font(0.0).size > 0.0);
    }

    #[test]
    fn resting_angles_differ_between_systems() {
        assert_ne!(resting_phase("sys_1"), resting_phase("sys_2"));
        assert_eq!(resting_phase("sys_1"), resting_phase("sys_1"));
        assert!((0.0..TAU).contains(&resting_phase("sys_3")));
        assert!(spin(1e9).abs() <= TAU);
    }
}
