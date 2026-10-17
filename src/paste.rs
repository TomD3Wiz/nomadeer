//! Tells apart what the player pasted from the game: probe scanner results, the
//! local member list or a directional scan.

use crate::live_intel::{self, DscanRow};
use crate::probe_scan::{self, ProbeScan};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Paste {
    ProbeScan(ProbeScan),
    /// Directional scanner rows.
    DirectionalScan(Vec<DscanRow>),
    /// Pilot names from the local member list.
    Local(Vec<String>),
    Unrecognised,
}

pub(crate) fn classify(text: &str) -> Paste {
    if let Some(scan) = probe_scan::parse(text) {
        return Paste::ProbeScan(scan);
    }
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Paste::Unrecognised;
    }
    // Every line must be a d-scan row; a stray line makes the paste unrecognised
    // rather than half read.
    let rows = lines
        .iter()
        .map(|line| live_intel::parse_dscan_row(line))
        .collect::<Option<Vec<_>>>();
    if let Some(rows) = rows {
        return Paste::DirectionalScan(rows);
    }
    if lines.iter().all(|line| is_pilot_name(line)) {
        return Paste::Local(lines.into_iter().map(str::to_owned).collect());
    }
    Paste::Unrecognised
}

/// Whether pasted names can be the local member list. That list always holds the
/// pilot who copied it, so a lone name only counts when it is one of the player's
/// characters: a single word copied from chat is not taken for local.
pub(crate) fn could_be_local(names: &[String], own_characters: &[&str]) -> bool {
    match names {
        [] => false,
        [name] => own_characters
            .iter()
            .any(|own| own.trim().eq_ignore_ascii_case(name.trim())),
        _ => true,
    }
}

/// Pilot names are 3 to 37 letters, digits, spaces, dashes, dots and apostrophes.
fn is_pilot_name(line: &str) -> bool {
    let length = line.chars().count();
    (3..=37).contains(&length)
        && line.chars().all(|character| {
            character.is_alphanumeric() || matches!(character, ' ' | '-' | '\'' | '.' | '_')
        })
}

#[cfg(test)]
mod tests {
    use super::{Paste, classify, could_be_local};

    fn kind(text: &str) -> &'static str {
        match classify(text) {
            Paste::ProbeScan(_) => "probe scan",
            Paste::DirectionalScan(_) => "d-scan",
            Paste::Local(_) => "local",
            Paste::Unrecognised => "unrecognised",
        }
    }

    #[test]
    fn pastes_are_told_apart() {
        assert!(matches!(
            classify("ABC-123\tCosmic Signature\tWormhole\tUnstable Wormhole\t100.0%\t4.32 AU"),
            Paste::ProbeScan(_)
        ));
        let Paste::DirectionalScan(rows) =
            classify("670\tCapsule\tCapsule\t1,234 km\n11182\tCheetah\tCheetah\t-\n")
        else {
            panic!("a d-scan");
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].type_id, Some(670));
        assert_eq!(rows[1].distance_km, None);
        assert_eq!(
            classify("Some Pilot\nO'Neill Jr.\nxX-Hunter-Xx\n"),
            Paste::Local(vec![
                "Some Pilot".to_owned(),
                "O'Neill Jr.".to_owned(),
                "xX-Hunter-Xx".to_owned(),
            ])
        );
        assert_eq!(classify(""), Paste::Unrecognised);
        assert_eq!(classify("  \n"), Paste::Unrecognised);
        assert_eq!(
            classify("https://zkillboard.com/kill/1/"),
            Paste::Unrecognised
        );
    }

    #[test]
    fn each_game_window_reads_as_itself_and_nothing_else() {
        // Probe scanner, partly scanned rows included.
        assert_eq!(
            kind(
                "ABC-123\tCosmic Signature\t\t\t12.5%\t4.32 AU\r\nDEF-456\tCosmic Anomaly\tCombat Site\tSansha Forsaken Hub\t100,0%\t1.234 km"
            ),
            "probe scan"
        );
        // D-scan, current and older clients, localised distances, off-grid rows.
        assert_eq!(
            kind("22456\tSabre\tSabre\t12 km\r\n14\tJ123456 VII - Moon 2\tMoon\t12,3 AE\r\n"),
            "d-scan"
        );
        assert_eq!(
            kind(
                "+\tNoctis\t3,225 m\nsome dude's Stabber\tStabber Fleet Issue\t-\nWreck\tTayra\t82 km"
            ),
            "d-scan"
        );
        // Local, including a pilot whose name looks like a signature ID.
        assert_eq!(kind("Some Pilot\nABC-123\nAnother Pilot"), "local");
        assert_eq!(kind("Jita"), "local");
        // Mixtures and near misses are not guessed at.
        assert_eq!(
            kind("22456\tSabre\tSabre\t12 km\nSome Pilot"),
            "unrecognised"
        );
        assert_eq!(kind("Name\tType\tsoon"), "unrecognised");
        assert_eq!(
            kind("A pilot name that is far too long to be a pilot"),
            "unrecognised"
        );
        assert_eq!(kind("Pilot <url=showinfo:1377//123>"), "unrecognised");
    }

    #[test]
    fn a_lone_name_is_local_only_when_it_is_yours() {
        let names = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
        let own = ["My Scout", "Other Alt"];
        assert!(could_be_local(&names("Some Pilot\nMy Scout"), &own));
        assert!(could_be_local(&names("Some Pilot\nAnother Pilot"), &own));
        assert!(could_be_local(&names("my scout"), &own));
        assert!(!could_be_local(&names("J123456"), &own));
        assert!(!could_be_local(&names("Jita"), &[]));
        assert!(!could_be_local(&[], &own));
    }
}
