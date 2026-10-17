//! Reads what EVE's probe scanner window copies: one row per result, tab-separated
//! as ID, Group, Type, Name, Signal and Distance, e.g.
//! "ABC-123\tCosmic Signature\tWormhole\tUnstable Wormhole\t100.0%\t4.32 AU".
//! Group, Type and Name stay blank until a signature is scanned far enough, and the
//! client writes numbers and some names in the player's language.

/// What a result is, from its Group column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ScanGroup {
    /// Hidden until probed down: wormholes and most exploration sites.
    Signature,
    /// Shown without probing: combat and ore anomalies.
    Anomaly,
    /// Anything else, such as ships found with combat probes.
    Other,
}

impl ScanGroup {
    pub(crate) fn of(group: &str) -> Self {
        let group = group.trim().to_lowercase();
        if group.is_empty() {
            // The group is known from the first scan; a row without one is a signature.
            return Self::Signature;
        }
        const SIGNATURE: &[&str] = &[
            "cosmic signature",
            "kosmische signatur",
            "signature cosmique",
            "firma cósmica",
            "скрытый сигнал",
            "宇宙のシグネチャ",
            "宇宙信号",
            "우주 시그니처",
        ];
        const ANOMALY: &[&str] = &[
            "cosmic anomaly",
            "kosmische anomalie",
            "anomalie cosmique",
            "anomalía cósmica",
            "космическая аномалия",
            "宇宙の特異点",
            "宇宙异常",
            "우주 이상 현상",
        ];
        if SIGNATURE.contains(&group.as_str())
            || group.contains("signat")
            || group.contains("сигнал")
        {
            Self::Signature
        } else if ANOMALY.contains(&group.as_str())
            || group.contains("anomal")
            || group.contains("аномал")
        {
            Self::Anomaly
        } else {
            Self::Other
        }
    }
}

/// What kind of site a result is, from its Type column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SiteKind {
    /// Not scanned far enough to tell.
    Unknown,
    Wormhole,
    Combat,
    Data,
    Relic,
    Gas,
    Ore,
    Other,
}

impl SiteKind {
    pub(crate) fn of(site_type: &str) -> Self {
        let site_type = site_type.trim().to_lowercase();
        if site_type.is_empty() {
            return Self::Unknown;
        }
        const KINDS: &[(SiteKind, &[&str])] = &[
            (
                SiteKind::Wormhole,
                &[
                    "wormhole",
                    "wurmloch",
                    "trou de ver",
                    "agujero de gusano",
                    "червоточина",
                    "ワームホール",
                    "虫洞",
                    "웜홀",
                ],
            ),
            (
                SiteKind::Combat,
                &[
                    "combat site",
                    "combat",
                    "kampfgebiet",
                    "site de combat",
                    "боевой район",
                ],
            ),
            (
                SiteKind::Data,
                &[
                    "data site",
                    "data",
                    "datengebiet",
                    "site de données",
                    "информационный район",
                ],
            ),
            (
                SiteKind::Relic,
                &[
                    "relic site",
                    "relic",
                    "reliktgebiet",
                    "site de reliques",
                    "археологический район",
                ],
            ),
            (
                SiteKind::Gas,
                &[
                    "gas site",
                    "gas",
                    "gasgebiet",
                    "site de gaz",
                    "газовый район",
                ],
            ),
            (
                SiteKind::Ore,
                &[
                    "ore site",
                    "ore",
                    "erzgebiet",
                    "site de minerai",
                    "рудный район",
                ],
            ),
        ];
        KINDS
            .iter()
            .find(|(_, names)| names.contains(&site_type.as_str()))
            .map(|(kind, _)| *kind)
            .unwrap_or(Self::Other)
    }
}

/// One row of the probe scanner.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProbeRow {
    /// The signature ID, e.g. "ABC-123", in upper case.
    pub(crate) id: String,
    pub(crate) group: String,
    pub(crate) site_type: String,
    pub(crate) name: String,
    /// Signal strength from 0 to 100.
    pub(crate) signal: Option<f32>,
    pub(crate) distance_km: Option<f64>,
}

impl ProbeRow {
    pub(crate) fn group_kind(&self) -> ScanGroup {
        ScanGroup::of(&self.group)
    }

    pub(crate) fn kind(&self) -> SiteKind {
        SiteKind::of(&self.site_type)
    }
}

/// A pasted probe scan.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ProbeScan {
    /// The rows in the order pasted, one per signature ID.
    pub(crate) rows: Vec<ProbeRow>,
    /// Non-blank lines that were not probe scanner rows.
    pub(crate) skipped: usize,
}

const KM_PER_AU: f64 = 149_597_870.7;

/// Reads a probe scanner paste. None when nothing in it looks like a probe scanner
/// row, or when most of its lines are something else.
pub(crate) fn parse(text: &str) -> Option<ProbeScan> {
    let mut scan = ProbeScan::default();
    for line in text.lines().map(|line| line.trim_end_matches('\r')) {
        if line.trim().is_empty() {
            continue;
        }
        match parse_row(line) {
            Some(row) => match scan.rows.iter_mut().find(|known| known.id == row.id) {
                // The same signature twice keeps what each copy knows.
                Some(known) => merge_rows(known, row),
                None => scan.rows.push(row),
            },
            None => scan.skipped += 1,
        }
    }
    (!scan.rows.is_empty() && scan.rows.len() >= scan.skipped).then_some(scan)
}

fn merge_rows(known: &mut ProbeRow, row: ProbeRow) {
    for (field, value) in [
        (&mut known.group, row.group),
        (&mut known.site_type, row.site_type),
        (&mut known.name, row.name),
    ] {
        if field.is_empty() {
            *field = value;
        }
    }
    known.signal = match (known.signal, row.signal) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    known.distance_km = known.distance_km.or(row.distance_km);
}

fn parse_row(line: &str) -> Option<ProbeRow> {
    let cells = line.split('\t').map(str::trim).collect::<Vec<_>>();
    let id = cells.first()?;
    if !is_signature_id(id) || cells.len() < 2 {
        return None;
    }
    let cell = |index: usize| cells.get(index).copied().unwrap_or_default();
    Some(ProbeRow {
        id: id.to_ascii_uppercase(),
        group: cell(1).to_owned(),
        site_type: cell(2).to_owned(),
        name: cell(3).to_owned(),
        signal: parse_signal(cell(4)),
        distance_km: parse_distance_km(cell(5)),
    })
}

/// Three letters, a dash and three digits, as every client language writes them.
pub(crate) fn is_signature_id(text: &str) -> bool {
    let bytes = text.trim().as_bytes();
    bytes.len() == 7
        && bytes[..3].iter().all(u8::is_ascii_alphabetic)
        && bytes[3] == b'-'
        && bytes[4..].iter().all(u8::is_ascii_digit)
}

/// "100.0%", "45,3 %" and the like, as 0 to 100.
fn parse_signal(text: &str) -> Option<f32> {
    let number = text.trim().trim_end_matches('%').trim();
    parse_number(number)
        .map(|value| value as f32)
        .filter(|value| (0.0..=100.0).contains(value))
}

/// "4.32 AU", "1,234 km", "12 345 м", "0,75 AE" and the like, in kilometres.
pub(crate) fn parse_distance_km(text: &str) -> Option<f64> {
    let text = text.trim();
    let split = text
        .char_indices()
        .find(|(_, ch)| ch.is_alphabetic())
        .map_or(text.len(), |(index, _)| index);
    let (number, unit) = text.split_at(split);
    let value = parse_number(number.trim())?;
    let unit = unit.trim().trim_end_matches('.').to_lowercase();
    let scale = match unit.as_str() {
        "au" | "ae" | "ua" | "а.е" | "ае" | "天文単位" | "天文单位" => KM_PER_AU,
        "km" | "км" | "公里" | "キロメートル" => 1.0,
        "m" | "м" | "米" | "メートル" => 0.001,
        _ => return None,
    };
    Some(value * scale)
}

/// A number written with either decimal separator and any thousands grouping:
/// "1,234.5", "1.234,5", "1 234", "4,32". A lone separator followed by exactly
/// three digits groups thousands; otherwise it marks the decimals.
pub(crate) fn parse_number(text: &str) -> Option<f64> {
    let text: String = text
        .chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, '\u{a0}' | '\u{202f}' | '\''))
        .collect();
    if text.is_empty() {
        return None;
    }
    let last_dot = text.rfind('.');
    let last_comma = text.rfind(',');
    let decimal = match (last_dot, last_comma) {
        (Some(dot), Some(comma)) => Some(dot.max(comma)),
        (Some(index), None) | (None, Some(index)) => {
            let separator = text.as_bytes()[index];
            let repeated = text.bytes().filter(|byte| *byte == separator).count() > 1;
            let digits_after = text.len() - index - 1;
            (!repeated && digits_after != 3).then_some(index)
        }
        (None, None) => None,
    };
    let mut plain = String::with_capacity(text.len());
    for (index, ch) in text.char_indices() {
        match ch {
            '.' | ',' if Some(index) == decimal => plain.push('.'),
            '.' | ',' => {}
            '0'..='9' | '-' | '+' => plain.push(ch),
            _ => return None,
        }
    }
    plain.parse::<f64>().ok().filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use super::{ScanGroup, SiteKind, is_signature_id, parse, parse_number};

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-6 * b.abs().max(1.0))
    }

    #[test]
    fn a_full_scan_reads_every_column() {
        let scan = parse(
            "ABC-123\tCosmic Signature\tWormhole\tUnstable Wormhole\t100.0%\t4.32 AU\n\
             DEF-456\tCosmic Signature\tData Site\tUnsecured Perimeter Amplifier\t100.0%\t12.08 AU\n\
             GHI-789\tCosmic Anomaly\tCombat Site\tSansha Forsaken Hub\t100.0%\t1,234 km\n",
        )
        .unwrap();
        assert_eq!(scan.skipped, 0);
        assert_eq!(scan.rows.len(), 3);
        let wormhole = &scan.rows[0];
        assert_eq!(wormhole.id, "ABC-123");
        assert_eq!(wormhole.group_kind(), ScanGroup::Signature);
        assert_eq!(wormhole.kind(), SiteKind::Wormhole);
        assert_eq!(wormhole.name, "Unstable Wormhole");
        assert_eq!(wormhole.signal, Some(100.0));
        assert!(close(wormhole.distance_km, 4.32 * 149_597_870.7));
        assert_eq!(scan.rows[1].kind(), SiteKind::Data);
        assert_eq!(scan.rows[2].group_kind(), ScanGroup::Anomaly);
        assert_eq!(scan.rows[2].kind(), SiteKind::Combat);
        assert!(close(scan.rows[2].distance_km, 1234.0));
    }

    #[test]
    fn partly_scanned_signatures_keep_blank_columns_blank() {
        let scan = parse(
            "XYZ-001\tCosmic Signature\t\t\t0.0%\t18.2 AU\n\
             XYZ-002\tCosmic Signature\tGas Site\t\t36.4%\t5.5 AU\n\
             XYZ-003\tCosmic Signature\tWormhole\t\t74.9%\t2.01 AU",
        )
        .unwrap();
        let unknown = &scan.rows[0];
        assert_eq!(unknown.site_type, "");
        assert_eq!(unknown.name, "");
        assert_eq!(unknown.kind(), SiteKind::Unknown);
        assert_eq!(unknown.signal, Some(0.0));
        assert_eq!(scan.rows[1].kind(), SiteKind::Gas);
        assert_eq!(scan.rows[1].signal, Some(36.4));
        assert_eq!(scan.rows[2].kind(), SiteKind::Wormhole);
        assert_eq!(scan.rows[2].name, "");
    }

    #[test]
    fn short_and_padded_rows_still_read() {
        // Some copies drop trailing blank cells or pad with spaces and Windows endings.
        let scan = parse("abc-123\tCosmic Signature\r\n  \n DEF-456 \t Cosmic Anomaly \t Ore Site \t Common Perimeter Deposit \t 100,0 % \t 30 km \t\n").unwrap();
        assert_eq!(scan.rows.len(), 2);
        assert_eq!(scan.rows[0].id, "ABC-123");
        assert_eq!(scan.rows[0].signal, None);
        assert_eq!(scan.rows[0].distance_km, None);
        assert_eq!(scan.rows[1].id, "DEF-456");
        assert_eq!(scan.rows[1].kind(), SiteKind::Ore);
        assert_eq!(scan.rows[1].signal, Some(100.0));
        assert!(close(scan.rows[1].distance_km, 30.0));
    }

    #[test]
    fn localised_clients_read_the_same() {
        let german = parse(
            "QRS-555\tKosmische Signatur\tWurmloch\tInstabiles Wurmloch\t100,0%\t4,32 AE\n\
             TUV-666\tKosmische Anomalie\tKampfgebiet\tSansha-Versteck\t100,0%\t1.234 km",
        )
        .unwrap();
        assert_eq!(german.rows[0].group_kind(), ScanGroup::Signature);
        assert_eq!(german.rows[0].kind(), SiteKind::Wormhole);
        assert!(close(german.rows[0].distance_km, 4.32 * 149_597_870.7));
        assert_eq!(german.rows[1].group_kind(), ScanGroup::Anomaly);
        assert_eq!(german.rows[1].kind(), SiteKind::Combat);
        assert!(close(german.rows[1].distance_km, 1234.0));

        let russian = parse(
            "ABC-123\tСкрытый сигнал\tЧервоточина\tНестабильная червоточина\t100,0 %\t4,32 а.е.\n\
             ABD-124\tКосмическая аномалия\tБоевой район\t\t100,0 %\t12 345 км",
        )
        .unwrap();
        assert_eq!(russian.rows[0].group_kind(), ScanGroup::Signature);
        assert_eq!(russian.rows[0].kind(), SiteKind::Wormhole);
        assert!(close(russian.rows[0].distance_km, 4.32 * 149_597_870.7));
        assert_eq!(russian.rows[1].group_kind(), ScanGroup::Anomaly);
        assert!(close(russian.rows[1].distance_km, 12_345.0));

        let french =
            parse("ZZZ-999\tSignature cosmique\tTrou de ver\tTrou de ver instable\t55,5 %\t0,8 UA")
                .unwrap();
        assert_eq!(french.rows[0].group_kind(), ScanGroup::Signature);
        assert_eq!(french.rows[0].kind(), SiteKind::Wormhole);
        assert_eq!(french.rows[0].signal, Some(55.5));

        let japanese = parse("JPN-101\t宇宙のシグネチャ\tワームホール\t\t100.0%\t3.1 AU").unwrap();
        assert_eq!(japanese.rows[0].kind(), SiteKind::Wormhole);
    }

    #[test]
    fn combat_probe_results_are_other() {
        let scan =
            parse("KLM-321\tFrigate\tBuzzard\tSomebody's Buzzard\t100.0%\t8,000 km").unwrap();
        assert_eq!(scan.rows[0].group_kind(), ScanGroup::Other);
        assert_eq!(scan.rows[0].kind(), SiteKind::Other);
    }

    #[test]
    fn other_text_is_not_a_probe_scan() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("Some Pilot\nAnother Pilot\nThird Pilot"), None);
        // Directional scanner rows lead with a type ID.
        assert_eq!(
            parse("670\tCapsule\tCapsule\t1,234 km\n11182\tCheetah\tCheetah\t-"),
            None
        );
        assert_eq!(parse("ABC-1234\tCosmic Signature"), None);
        assert_eq!(parse("AB1-123\tCosmic Signature"), None);
        // A probe row quoted in a chat line is not a scan.
        assert_eq!(parse("ABC-123"), None);
        // Mostly something else.
        assert_eq!(
            parse("hello\nthere\nABC-123\tCosmic Signature\t\t\t0.0%\t1 AU"),
            None
        );
    }

    #[test]
    fn a_stray_line_is_skipped() {
        let scan = parse(
            "ABC-123\tCosmic Signature\t\t\t12.0%\t3 AU\nthis is not a row\nDEF-456\tCosmic Signature\t\t\t0.0%\t9 AU",
        )
        .unwrap();
        assert_eq!(scan.rows.len(), 2);
        assert_eq!(scan.skipped, 1);
    }

    #[test]
    fn a_signature_pasted_twice_keeps_what_each_copy_knows() {
        let scan = parse(
            "ABC-123\tCosmic Signature\t\t\t20.0%\t3 AU\nABC-123\tCosmic Signature\tWormhole\t\t60.0%\t",
        )
        .unwrap();
        assert_eq!(scan.rows.len(), 1);
        assert_eq!(scan.rows[0].kind(), SiteKind::Wormhole);
        assert_eq!(scan.rows[0].signal, Some(60.0));
        assert!(scan.rows[0].distance_km.is_some());
    }

    #[test]
    fn signature_ids_are_three_letters_and_three_digits() {
        assert!(is_signature_id("ABC-123"));
        assert!(is_signature_id("xyz-000"));
        assert!(!is_signature_id("ABC123"));
        assert!(!is_signature_id("AB-1234"));
        assert!(!is_signature_id("ÄBC-123"));
    }

    #[test]
    fn numbers_read_with_either_separator() {
        assert_eq!(parse_number("100.0"), Some(100.0));
        assert_eq!(parse_number("100,0"), Some(100.0));
        assert_eq!(parse_number("4,32"), Some(4.32));
        assert_eq!(parse_number("1,234"), Some(1234.0));
        assert_eq!(parse_number("1.234"), Some(1234.0));
        assert_eq!(parse_number("12,345,678"), Some(12_345_678.0));
        assert_eq!(parse_number("1,234.56"), Some(1234.56));
        assert_eq!(parse_number("1.234,56"), Some(1234.56));
        assert_eq!(parse_number("12 345"), Some(12_345.0));
        assert_eq!(parse_number("12\u{a0}345,5"), Some(12_345.5));
        assert_eq!(parse_number("-"), None);
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("abc"), None);
    }

    #[test]
    fn signal_outside_a_percentage_is_ignored() {
        let scan = parse("ABC-123\tCosmic Signature\t\t\t250%\t1 AU").unwrap();
        assert_eq!(scan.rows[0].signal, None);
        let metres = parse("ABC-123\tCosmic Anomaly\tOre Site\tX\t100%\t500 m").unwrap();
        assert!(close(metres.rows[0].distance_km, 0.5));
    }
}
