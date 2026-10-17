//! Keeps each system's signatures up to date from pasted probe scans: new ones are
//! added, known ones learn what the scan adds, and ones missing from a scan are
//! offered for removal rather than dropped.

use crate::model::{Connection, Signature};
use crate::probe_scan::{ProbeRow, ProbeScan, ScanGroup, SiteKind};

/// What a probe scan changed in one system.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ScanMerge {
    /// Signature IDs added.
    pub(crate) added: Vec<String>,
    /// Signature IDs that learned something.
    pub(crate) updated: Vec<String>,
    pub(crate) unchanged: usize,
    /// Signature IDs recorded here but missing from the scan, in groups the scan
    /// covered. They are kept until the player removes them.
    pub(crate) gone: Vec<String>,
}

impl ScanMerge {
    /// "2 new, 1 updated", or "Nothing new" when the scan matched what was known.
    pub(crate) fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.added.is_empty() {
            parts.push(format!("{} new", self.added.len()));
        }
        if !self.updated.is_empty() {
            parts.push(format!("{} updated", self.updated.len()));
        }
        if parts.is_empty() {
            "Nothing new".to_owned()
        } else {
            parts.join(", ")
        }
    }
}

pub(crate) fn group_kind(signature: &Signature) -> ScanGroup {
    ScanGroup::of(&signature.scan_group)
}

pub(crate) fn site_kind(signature: &Signature) -> SiteKind {
    SiteKind::of(&signature.sig_type)
}

pub(crate) fn is_wormhole(signature: &Signature) -> bool {
    site_kind(signature) == SiteKind::Wormhole
}

/// Merges a probe scan into the signatures of `system_id`. Known information is
/// never replaced by a blank, and the strongest signal seen is kept. Ships and
/// other combat probe results are not sites, so they are left out.
pub(crate) fn merge_probe_scan(
    signatures: &mut Vec<Signature>,
    system_id: &str,
    scan: &ProbeScan,
    now: u64,
    mut new_id: impl FnMut() -> String,
) -> ScanMerge {
    let mut merge = ScanMerge::default();
    let rows = scan
        .rows
        .iter()
        .filter(|row| row.group_kind() != ScanGroup::Other)
        .collect::<Vec<_>>();
    for row in &rows {
        match signatures.iter_mut().find(|signature| {
            signature.system_id == system_id && signature.code.eq_ignore_ascii_case(&row.id)
        }) {
            Some(signature) => {
                if apply_row(signature, row) {
                    signature.updated_at = now;
                    merge.updated.push(row.id.clone());
                } else {
                    merge.unchanged += 1;
                }
            }
            None => {
                let mut signature = Signature {
                    id: new_id(),
                    system_id: system_id.to_owned(),
                    code: row.id.clone(),
                    created_at: now,
                    updated_at: now,
                    ..Signature::default()
                };
                apply_row(&mut signature, row);
                signatures.push(signature);
                merge.added.push(row.id.clone());
            }
        }
    }

    let covered = rows.iter().map(|row| row.group_kind()).collect::<Vec<_>>();
    merge.gone = signatures
        .iter()
        .filter(|signature| signature.system_id == system_id)
        .filter(|signature| covered.contains(&group_kind(signature)))
        .filter(|signature| {
            !rows
                .iter()
                .any(|row| signature.code.eq_ignore_ascii_case(&row.id))
        })
        .map(|signature| signature.code.clone())
        .collect();
    merge
}

/// Copies what the row knows onto the signature; true if anything changed.
fn apply_row(signature: &mut Signature, row: &ProbeRow) -> bool {
    let before = signature.clone();
    for (field, value) in [
        (&mut signature.scan_group, &row.group),
        (&mut signature.sig_type, &row.site_type),
        (&mut signature.target, &row.name),
    ] {
        if !value.is_empty() {
            value.clone_into(field);
        }
    }
    // A site that turned out to be something else loses the name of its old guess.
    if !row.site_type.is_empty()
        && !before.sig_type.is_empty()
        && SiteKind::of(&before.sig_type) != row.kind()
        && row.name.is_empty()
    {
        signature.target.clear();
    }
    if let Some(signal) = row.signal {
        signature.signal = signature.signal.max(signal);
    }
    *signature != before
}

/// Removes the given signatures from `system_id`, and unlinks any connection that
/// pointed at them. Returns how many were removed.
pub(crate) fn remove_signatures(
    signatures: &mut Vec<Signature>,
    connections: &mut [Connection],
    system_id: &str,
    codes: &[String],
) -> usize {
    let removes = |code: &str| codes.iter().any(|gone| gone.eq_ignore_ascii_case(code));
    let before = signatures.len();
    signatures.retain(|signature| signature.system_id != system_id || !removes(&signature.code));
    for connection in connections.iter_mut() {
        if connection.from_system == system_id && removes(&connection.signature_from) {
            connection.signature_from.clear();
        }
        if connection.to_system == system_id && removes(&connection.signature_to) {
            connection.signature_to.clear();
        }
    }
    before - signatures.len()
}

/// The connection a signature in `system_id` is linked to, if any.
pub(crate) fn linked_connection<'a>(
    connections: &'a [Connection],
    system_id: &str,
    code: &str,
) -> Option<&'a Connection> {
    connections
        .iter()
        .find(|connection| linked_code(connection, system_id) == Some(code))
}

/// The signature ID a connection is linked to on the side in `system_id`.
pub(crate) fn linked_code<'a>(connection: &'a Connection, system_id: &str) -> Option<&'a str> {
    let code = if connection.from_system == system_id {
        &connection.signature_from
    } else if connection.to_system == system_id {
        &connection.signature_to
    } else {
        return None;
    };
    (!code.is_empty()).then_some(code.as_str())
}

/// Links the signature `code` in `system_id` to a connection, unlinking it from any
/// other connection first so each signature leads one way. An empty code unlinks.
pub(crate) fn link(
    connections: &mut [Connection],
    connection_id: &str,
    system_id: &str,
    code: &str,
) {
    for connection in connections.iter_mut() {
        let is_target = connection.id == connection_id;
        let side = if connection.from_system == system_id {
            &mut connection.signature_from
        } else if connection.to_system == system_id {
            &mut connection.signature_to
        } else {
            continue;
        };
        if is_target {
            code.clone_into(side);
        } else if !code.is_empty() && side.eq_ignore_ascii_case(code) {
            side.clear();
        }
    }
}

/// The link a w-space system's signatures leave no doubt about: every signature is
/// identified, and exactly one wormhole and one connection are still unlinked.
/// Known space is never linked this way, as its connections may be stargates.
pub(crate) fn sure_link(
    signatures: &[Signature],
    connections: &[Connection],
    system_id: &str,
) -> Option<(String, String)> {
    let in_system = || {
        signatures.iter().filter(|signature| {
            signature.system_id == system_id && group_kind(signature) == ScanGroup::Signature
        })
    };
    if in_system().any(|signature| site_kind(signature) == SiteKind::Unknown) {
        return None;
    }
    let free_signature = only(in_system().filter(|signature| {
        is_wormhole(signature)
            && linked_connection(connections, system_id, &signature.code).is_none()
    }))?;
    let free_connection = only(connections.iter().filter(|connection| {
        (connection.from_system == system_id || connection.to_system == system_id)
            && linked_code(connection, system_id).is_none()
    }))?;
    Some((free_connection.id.clone(), free_signature.code.clone()))
}

fn only<T>(mut items: impl Iterator<Item = T>) -> Option<T> {
    let first = items.next()?;
    items.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe_scan;

    const NOW: u64 = 1_760_000_000;

    fn ids() -> impl FnMut() -> String {
        let mut next = 0;
        move || {
            next += 1;
            format!("sig_{next}")
        }
    }

    fn scan(text: &str) -> ProbeScan {
        probe_scan::parse(text).expect("test scan should parse")
    }

    fn find<'a>(signatures: &'a [Signature], code: &str) -> &'a Signature {
        signatures
            .iter()
            .find(|signature| signature.code == code)
            .unwrap()
    }

    #[test]
    fn a_first_scan_adds_every_row() {
        let mut signatures = Vec::new();
        let merge = merge_probe_scan(
            &mut signatures,
            "home",
            &scan(
                "ABC-123\tCosmic Signature\tWormhole\tUnstable Wormhole\t100.0%\t4 AU\n\
                 DEF-456\tCosmic Signature\t\t\t12.5%\t9 AU\n\
                 GHI-789\tCosmic Anomaly\tCombat Site\tSansha Forsaken Hub\t100.0%\t40 km",
            ),
            NOW,
            ids(),
        );
        assert_eq!(merge.added, vec!["ABC-123", "DEF-456", "GHI-789"]);
        assert!(merge.updated.is_empty() && merge.gone.is_empty());
        assert_eq!(merge.summary(), "3 new");
        let wormhole = find(&signatures, "ABC-123");
        assert_eq!(wormhole.id, "sig_1");
        assert_eq!(wormhole.system_id, "home");
        assert!(is_wormhole(wormhole));
        assert_eq!(wormhole.target, "Unstable Wormhole");
        assert_eq!(wormhole.created_at, NOW);
        assert_eq!(group_kind(find(&signatures, "GHI-789")), ScanGroup::Anomaly);
        assert_eq!(site_kind(find(&signatures, "DEF-456")), SiteKind::Unknown);
    }

    #[test]
    fn rescans_add_detail_and_never_blank_it() {
        let mut signatures = Vec::new();
        merge_probe_scan(
            &mut signatures,
            "home",
            &scan(
                "ABC-123\tCosmic Signature\tWormhole\tUnstable Wormhole\t100.0%\t4 AU\nDEF-456\tCosmic Signature\t\t\t12.5%\t9 AU",
            ),
            NOW,
            ids(),
        );
        // Probes moved away: weaker signal and blank columns.
        let merge = merge_probe_scan(
            &mut signatures,
            "home",
            &scan(
                "ABC-123\tCosmic Signature\t\t\t30.0%\t4 AU\nDEF-456\tCosmic Signature\tRelic Site\t\t55.0%\t9 AU",
            ),
            NOW + 60,
            ids(),
        );
        assert!(merge.added.is_empty());
        assert_eq!(merge.updated, vec!["DEF-456"]);
        assert_eq!(merge.unchanged, 1);
        assert_eq!(merge.summary(), "1 updated");
        let wormhole = find(&signatures, "ABC-123");
        assert_eq!(wormhole.sig_type, "Wormhole");
        assert_eq!(wormhole.target, "Unstable Wormhole");
        assert_eq!(wormhole.signal, 100.0);
        assert_eq!(wormhole.updated_at, NOW);
        let relic = find(&signatures, "DEF-456");
        assert_eq!(relic.sig_type, "Relic Site");
        assert_eq!(relic.signal, 55.0);
        assert_eq!(relic.updated_at, NOW + 60);
        assert_eq!(relic.created_at, NOW);

        let again = merge_probe_scan(
            &mut signatures,
            "home",
            &scan("abc-123\tCosmic Signature\tWormhole\tUnstable Wormhole\t100.0%\t4 AU"),
            NOW + 120,
            ids(),
        );
        assert_eq!(again.summary(), "Nothing new");
        assert_eq!(again.unchanged, 1);
    }

    #[test]
    fn a_site_that_was_misread_takes_the_new_type() {
        let mut signatures = vec![Signature {
            id: "sig_old".to_owned(),
            system_id: "home".to_owned(),
            code: "ABC-123".to_owned(),
            sig_type: "Data Site".to_owned(),
            target: "Central Guristas Survey Site".to_owned(),
            ..Signature::default()
        }];
        merge_probe_scan(
            &mut signatures,
            "home",
            &scan("ABC-123\tCosmic Signature\tWormhole\t\t80.0%\t4 AU"),
            NOW,
            ids(),
        );
        assert_eq!(signatures[0].sig_type, "Wormhole");
        assert_eq!(signatures[0].target, "");
        assert_eq!(signatures[0].id, "sig_old");
    }

    #[test]
    fn missing_signatures_are_offered_only_for_groups_the_scan_covered() {
        let mut signatures = Vec::new();
        merge_probe_scan(
            &mut signatures,
            "home",
            &scan(
                "ABC-123\tCosmic Signature\t\t\t0.0%\t4 AU\n\
                 DEF-456\tCosmic Signature\t\t\t0.0%\t4 AU\n\
                 GHI-789\tCosmic Anomaly\tOre Site\tAsteroid Field\t100.0%\t4 AU",
            ),
            NOW,
            ids(),
        );
        // Another system's signatures are never touched.
        merge_probe_scan(
            &mut signatures,
            "jita",
            &scan("ZZZ-999\tCosmic Signature\t\t\t0.0%\t4 AU"),
            NOW,
            ids(),
        );
        // Anomalies filtered out of the scanner: only signatures can be gone.
        let merge = merge_probe_scan(
            &mut signatures,
            "home",
            &scan("ABC-123\tCosmic Signature\t\t\t10.0%\t4 AU"),
            NOW,
            ids(),
        );
        assert_eq!(merge.gone, vec!["DEF-456"]);
        // Nothing is removed until the player says so.
        assert_eq!(signatures.len(), 4);
    }

    #[test]
    fn combat_probe_results_are_not_signatures() {
        let mut signatures = vec![Signature {
            system_id: "home".to_owned(),
            code: "ABC-123".to_owned(),
            ..Signature::default()
        }];
        let merge = merge_probe_scan(
            &mut signatures,
            "home",
            &scan("KLM-321\tFrigate\tBuzzard\tSomebody's Buzzard\t100.0%\t8,000 km"),
            NOW,
            ids(),
        );
        assert_eq!(merge, ScanMerge::default());
        assert_eq!(signatures.len(), 1);
    }

    #[test]
    fn removing_gone_signatures_unlinks_their_connections() {
        let mut signatures = vec![
            Signature {
                system_id: "home".to_owned(),
                code: "ABC-123".to_owned(),
                ..Signature::default()
            },
            Signature {
                system_id: "home".to_owned(),
                code: "DEF-456".to_owned(),
                ..Signature::default()
            },
            Signature {
                system_id: "jita".to_owned(),
                code: "ABC-123".to_owned(),
                ..Signature::default()
            },
        ];
        let mut connections = vec![Connection {
            id: "conn".to_owned(),
            from_system: "home".to_owned(),
            to_system: "jita".to_owned(),
            signature_from: "ABC-123".to_owned(),
            signature_to: "ABC-123".to_owned(),
            ..Connection::default()
        }];
        let removed = remove_signatures(
            &mut signatures,
            &mut connections,
            "home",
            &["abc-123".to_owned()],
        );
        assert_eq!(removed, 1);
        assert_eq!(signatures.len(), 2);
        assert_eq!(connections[0].signature_from, "");
        // The same ID in the far system stays linked.
        assert_eq!(connections[0].signature_to, "ABC-123");
    }

    #[test]
    fn only_an_unambiguous_scan_links_by_itself() {
        let wormhole = |code: &str| Signature {
            system_id: "home".to_owned(),
            code: code.to_owned(),
            scan_group: "Cosmic Signature".to_owned(),
            sig_type: "Wormhole".to_owned(),
            ..Signature::default()
        };
        let connection = |id: &str, to: &str| Connection {
            id: id.to_owned(),
            from_system: "home".to_owned(),
            to_system: to.to_owned(),
            ..Connection::default()
        };
        let data_site = Signature {
            sig_type: "Data Site".to_owned(),
            ..wormhole("DAT-111")
        };
        let mut signatures = vec![wormhole("ABC-123"), data_site];
        let mut connections = vec![connection("a", "jita")];
        assert_eq!(
            sure_link(&signatures, &connections, "home"),
            Some(("a".to_owned(), "ABC-123".to_owned()))
        );

        // A signature not yet identified could be the one.
        signatures.push(Signature {
            sig_type: String::new(),
            ..wormhole("UNK-000")
        });
        assert_eq!(sure_link(&signatures, &connections, "home"), None);
        signatures.pop();

        // Two holes, one connection: no telling which.
        signatures.push(wormhole("DEF-456"));
        assert_eq!(sure_link(&signatures, &connections, "home"), None);

        // Once one is linked the other is the only one left.
        connections.push(connection("b", "amarr"));
        link(&mut connections, "a", "home", "ABC-123");
        assert_eq!(
            sure_link(&signatures, &connections, "home"),
            Some(("b".to_owned(), "DEF-456".to_owned()))
        );
        link(&mut connections, "b", "home", "DEF-456");
        assert_eq!(sure_link(&signatures, &connections, "home"), None);
    }

    #[test]
    fn a_signature_links_to_one_connection() {
        let connection = |id: &str, to: &str| Connection {
            id: id.to_owned(),
            from_system: "home".to_owned(),
            to_system: to.to_owned(),
            ..Connection::default()
        };
        let mut connections = vec![connection("a", "jita"), connection("b", "amarr")];
        link(&mut connections, "a", "home", "ABC-123");
        assert_eq!(
            linked_connection(&connections, "home", "ABC-123").map(|c| c.id.as_str()),
            Some("a")
        );
        link(&mut connections, "b", "home", "ABC-123");
        assert_eq!(connections[0].signature_from, "");
        assert_eq!(connections[1].signature_from, "ABC-123");
        assert_eq!(linked_code(&connections[1], "home"), Some("ABC-123"));
        assert_eq!(linked_code(&connections[1], "amarr"), None);
        // Linking the far side leaves the near side alone.
        link(&mut connections, "b", "amarr", "XYZ-001");
        assert_eq!(connections[1].signature_to, "XYZ-001");
        assert_eq!(connections[1].signature_from, "ABC-123");
        link(&mut connections, "b", "home", "");
        assert_eq!(linked_code(&connections[1], "home"), None);
    }
}
