//! Tells apart what the player pasted from the game: probe scanner results, the
//! local member list or a directional scan.

use crate::probe_scan::{self, ProbeScan};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Paste {
    ProbeScan(ProbeScan),
    /// Directional scanner rows, as pasted.
    DirectionalScan(Vec<String>),
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
    if lines.iter().all(|line| is_directional_row(line)) {
        return Paste::DirectionalScan(lines.into_iter().map(str::to_owned).collect());
    }
    if lines.iter().all(|line| is_pilot_name(line)) {
        return Paste::Local(lines.into_iter().map(str::to_owned).collect());
    }
    Paste::Unrecognised
}

/// "670\tCapsule\tCapsule\t1,234 km": type ID, name, type and distance.
fn is_directional_row(line: &str) -> bool {
    let cells = line.split('\t').collect::<Vec<_>>();
    cells.len() >= 3
        && cells[0]
            .trim()
            .chars()
            .all(|character| character.is_ascii_digit())
        && !cells[0].trim().is_empty()
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
    use super::{Paste, classify};

    #[test]
    fn pastes_are_told_apart() {
        assert!(matches!(
            classify("ABC-123\tCosmic Signature\tWormhole\tUnstable Wormhole\t100.0%\t4.32 AU"),
            Paste::ProbeScan(_)
        ));
        assert_eq!(
            classify("670\tCapsule\tCapsule\t1,234 km\n11182\tCheetah\tCheetah\t-\n"),
            Paste::DirectionalScan(vec![
                "670\tCapsule\tCapsule\t1,234 km".to_owned(),
                "11182\tCheetah\tCheetah\t-".to_owned(),
            ])
        );
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
}
