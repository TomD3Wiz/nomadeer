//! Recon pilots, the characters whose arrivals gather intel, and whether each
//! character is logged in.

use crate::auto_intel::Presence;
use crate::esi::{self, ONLINE_SCOPE};
use crate::model::{Account, AppData};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Recon pilots at a time, so a large roster does not queue intel for every system.
pub(crate) const MAX_RECON_CHARACTERS: usize = 3;
/// ESI refreshes online status about once a minute; asking sooner gets the same answer.
const MIN_ONLINE_POLL: Duration = Duration::from_secs(60);
const MAX_ONLINE_RETRY: Duration = Duration::from_secs(15 * 60);

pub(crate) fn recon_count(accounts: &[Account]) -> usize {
    accounts.iter().filter(|account| account.recon).count()
}

/// Whether the character may be a recon pilot: they already are, or there is room.
pub(crate) fn can_mark_recon(accounts: &[Account], account_id: &str) -> bool {
    accounts
        .iter()
        .any(|account| account.id == account_id && account.recon)
        || recon_count(accounts) < MAX_RECON_CHARACTERS
}

/// Marks or unmarks a recon pilot and returns whether anything changed. Marking one
/// beyond [`MAX_RECON_CHARACTERS`] is refused.
pub(crate) fn set_recon(accounts: &mut [Account], account_id: &str, recon: bool) -> bool {
    if recon && !can_mark_recon(accounts, account_id) {
        return false;
    }
    match accounts.iter_mut().find(|account| account.id == account_id) {
        Some(account) if account.recon != recon => {
            account.recon = recon;
            true
        }
        _ => false,
    }
}

/// The character Nomadeer treats as the first: the first one with a known location,
/// as the selected system starts there, or else the first in the list.
fn first_character(accounts: &[Account]) -> Option<usize> {
    accounts
        .iter()
        .position(|account| !account.last_system_id.is_empty())
        .or((!accounts.is_empty()).then_some(0))
}

/// Makes the first character a recon pilot when characters exist but none is one yet,
/// e.g. from before recon pilots existed. It applies once; a player who later unmarks
/// everyone keeps it that way. Returns whether the data changed.
pub(crate) fn apply_recon_default(data: &mut AppData) -> bool {
    if data.recon_defaults_applied || data.accounts.is_empty() {
        return false;
    }
    data.recon_defaults_applied = true;
    if recon_count(&data.accounts) == 0
        && let Some(index) = first_character(&data.accounts)
    {
        data.accounts[index].recon = true;
    }
    true
}

/// A newly added character is a recon pilot when it is the only character.
pub(crate) fn mark_added_character(data: &mut AppData, account_id: &str) {
    if data.accounts.len() == 1 {
        set_recon(&mut data.accounts, account_id, true);
    }
    data.recon_defaults_applied = true;
}

/// Whether Nomadeer can ask ESI if the character is logged in: they are signed in and
/// their sign-in included online status. `None` when the sign-in cannot be read.
pub(crate) fn online_scope(account: &Account) -> Option<bool> {
    if account.auth_status != "authorized" {
        return Some(false);
    }
    account
        .access_token
        .as_deref()
        .map_or(Some(false), |token| {
            esi::token_has_scope(token, ONLINE_SCOPE)
        })
}

pub(crate) fn online_status_available(account: &Account) -> bool {
    online_scope(account) == Some(true)
}

/// Online status per character, polled no more often than ESI refreshes it.
#[derive(Debug, Default)]
pub(crate) struct OnlinePolls {
    characters: HashMap<String, OnlineState>,
}

#[derive(Debug)]
struct OnlineState {
    /// `None` after a failed poll, until one succeeds.
    online: Option<bool>,
    next_poll: Instant,
    failures: u32,
}

impl OnlinePolls {
    /// The character to ask about next among `account_ids`, the ones whose status is
    /// available: never asked first, then the longest overdue.
    pub(crate) fn next_due<'a>(
        &self,
        account_ids: impl IntoIterator<Item = &'a str>,
        at: Instant,
    ) -> Option<&'a str> {
        account_ids
            .into_iter()
            .filter(|account_id| {
                self.characters
                    .get(*account_id)
                    .is_none_or(|state| at >= state.next_poll)
            })
            .min_by_key(|account_id| {
                self.characters
                    .get(*account_id)
                    .map(|state| state.next_poll)
            })
    }

    /// Records ESI's answer and returns the previous one, if known. The next poll waits
    /// until ESI's cached answer expires, and at least a minute.
    pub(crate) fn record(
        &mut self,
        account_id: &str,
        online: bool,
        cache_lifetime: Duration,
        at: Instant,
    ) -> Option<bool> {
        let wait = cache_lifetime.max(MIN_ONLINE_POLL);
        self.characters
            .insert(
                account_id.to_owned(),
                OnlineState {
                    online: Some(online),
                    next_poll: at + wait,
                    failures: 0,
                },
            )
            .and_then(|previous| previous.online)
    }

    /// A failed poll leaves the status unknown and backs off before asking again.
    pub(crate) fn record_failure(&mut self, account_id: &str, at: Instant) {
        let state = self
            .characters
            .entry(account_id.to_owned())
            .or_insert(OnlineState {
                online: None,
                next_poll: at,
                failures: 0,
            });
        state.online = None;
        state.failures = state.failures.saturating_add(1);
        let factor = 1_u32 << (state.failures - 1).min(8);
        state.next_poll = at + (MIN_ONLINE_POLL * factor).min(MAX_ONLINE_RETRY);
    }

    pub(crate) fn online(&self, account_id: &str) -> Option<bool> {
        self.characters
            .get(account_id)
            .and_then(|state| state.online)
    }

    pub(crate) fn presence(&self, account_id: &str, status_available: bool) -> Presence {
        match self.online(account_id) {
            Some(true) if status_available => Presence::Online,
            Some(false) if status_available => Presence::Offline,
            _ => Presence::Unknown,
        }
    }

    /// Whether to poll the character's location. Until ESI first says whether they are
    /// logged in, their location waits, so the first fix is judged knowing it. After
    /// that they are polled whether logged in or not: the status can lag a login by a
    /// minute or two, and every jump in that time belongs on the map.
    pub(crate) fn wants_location_poll(&self, account_id: &str, status_available: bool) -> bool {
        !status_available || self.characters.contains_key(account_id)
    }

    /// Starts over for a character, e.g. after they sign in again or are removed.
    pub(crate) fn forget(&mut self, account_id: &str) {
        self.characters.remove(account_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_RECON_CHARACTERS, OnlinePolls, apply_recon_default, can_mark_recon,
        mark_added_character, online_scope, recon_count, set_recon,
    };
    use crate::auto_intel::Presence;
    use crate::esi::ONLINE_SCOPE;
    use crate::model::{Account, AppData};
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use std::time::{Duration, Instant};

    fn secs(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    fn account(id: &str, last_system_id: &str) -> Account {
        Account {
            id: id.to_owned(),
            character_id: "90000001".to_owned(),
            character_name: id.to_owned(),
            owner_hash: String::new(),
            auth_status: "authorized".to_owned(),
            last_system_id: last_system_id.to_owned(),
            last_system_name: String::new(),
            access_token: None,
            refresh_token: None,
            token_expires_at: None,
            created_at: 1,
            updated_at: 1,
            recon: false,
        }
    }

    fn roster(count: usize) -> Vec<Account> {
        (0..count)
            .map(|index| account(&format!("acct_{index}"), ""))
            .collect()
    }

    fn recon_ids(accounts: &[Account]) -> Vec<&str> {
        accounts
            .iter()
            .filter(|account| account.recon)
            .map(|account| account.id.as_str())
            .collect()
    }

    #[test]
    fn recon_pilots_are_limited() {
        let mut accounts = roster(5);
        for index in 0..MAX_RECON_CHARACTERS {
            assert!(set_recon(&mut accounts, &format!("acct_{index}"), true));
        }
        assert_eq!(recon_count(&accounts), MAX_RECON_CHARACTERS);
        assert!(!can_mark_recon(&accounts, "acct_4"));
        assert!(!set_recon(&mut accounts, "acct_4", true));
        assert!(!accounts[4].recon);
        // Recon pilots can still be unmarked, which makes room.
        assert!(can_mark_recon(&accounts, "acct_0"));
        assert!(!set_recon(&mut accounts, "acct_0", true));
        assert!(set_recon(&mut accounts, "acct_0", false));
        assert!(can_mark_recon(&accounts, "acct_4"));
        assert!(set_recon(&mut accounts, "acct_4", true));
        assert_eq!(recon_ids(&accounts), ["acct_1", "acct_2", "acct_4"]);
        assert!(!set_recon(&mut accounts, "missing", false));
    }

    #[test]
    fn the_first_character_becomes_recon_once() {
        let mut data = AppData {
            accounts: vec![account("acct_a", ""), account("acct_b", "sys_1")],
            ..AppData::default()
        };
        assert!(apply_recon_default(&mut data));
        // The first character with a known location, as the selected system starts there.
        assert_eq!(recon_ids(&data.accounts), ["acct_b"]);
        assert!(data.recon_defaults_applied);

        // A player who unmarks everyone is not overruled on the next launch.
        set_recon(&mut data.accounts, "acct_b", false);
        assert!(!apply_recon_default(&mut data));
        assert_eq!(recon_count(&data.accounts), 0);

        // Nobody located yet: the first in the list.
        let mut fresh = AppData {
            accounts: vec![account("acct_a", ""), account("acct_b", "")],
            ..AppData::default()
        };
        assert!(apply_recon_default(&mut fresh));
        assert_eq!(recon_ids(&fresh.accounts), ["acct_a"]);

        // Existing recon pilots are kept as they are.
        let mut chosen = AppData {
            accounts: vec![account("acct_a", "sys_1"), account("acct_b", "")],
            ..AppData::default()
        };
        chosen.accounts[1].recon = true;
        assert!(apply_recon_default(&mut chosen));
        assert_eq!(recon_ids(&chosen.accounts), ["acct_b"]);

        // Without characters there is nothing to apply yet.
        let mut empty = AppData::default();
        assert!(!apply_recon_default(&mut empty));
        assert!(!empty.recon_defaults_applied);
    }

    #[test]
    fn an_added_character_is_recon_only_when_it_is_the_only_one() {
        let mut data = AppData::default();
        data.accounts.push(account("acct_a", ""));
        mark_added_character(&mut data, "acct_a");
        assert_eq!(recon_ids(&data.accounts), ["acct_a"]);
        assert!(data.recon_defaults_applied);

        data.accounts.push(account("acct_b", ""));
        mark_added_character(&mut data, "acct_b");
        assert_eq!(recon_ids(&data.accounts), ["acct_a"]);

        // Starting over after removing everyone makes the new character recon again.
        data.accounts.clear();
        data.accounts.push(account("acct_c", ""));
        mark_added_character(&mut data, "acct_c");
        assert_eq!(recon_ids(&data.accounts), ["acct_c"]);
    }

    fn token(scopes: &str) -> String {
        format!(
            "e30.{}.sig",
            URL_SAFE_NO_PAD.encode(format!(r#"{{"scp":{scopes}}}"#))
        )
    }

    #[test]
    fn online_status_needs_a_signed_in_character_with_the_scope() {
        let mut character = account("acct_a", "");
        assert_eq!(online_scope(&character), Some(false));
        character.access_token = Some(token(&format!(r#"["publicData","{ONLINE_SCOPE}"]"#)));
        assert_eq!(online_scope(&character), Some(true));
        character.access_token = Some(token(r#"["publicData"]"#));
        assert_eq!(online_scope(&character), Some(false));
        character.access_token = Some("garbled".to_owned());
        assert_eq!(online_scope(&character), None);
        character.access_token = Some(token(&format!(r#""{ONLINE_SCOPE}""#)));
        character.auth_status = "authorizing".to_owned();
        assert_eq!(online_scope(&character), Some(false));
    }

    #[test]
    fn online_polls_follow_esi_expiry_and_ask_new_characters_first() {
        let start = Instant::now();
        let mut polls = OnlinePolls::default();
        let ids = ["acct_a", "acct_b"];
        assert_eq!(polls.next_due(ids, start), Some("acct_a"));
        assert_eq!(polls.record("acct_a", true, secs(30), start), None);
        // Never asked before beats everything else.
        assert_eq!(polls.next_due(ids, start), Some("acct_b"));
        assert_eq!(polls.record("acct_b", false, secs(120), start), None);
        assert_eq!(polls.next_due(ids, start), None);
        // ESI's cache lasts 30 s, but polls are at least a minute apart.
        assert_eq!(polls.next_due(ids, start + Duration::from_secs(30)), None);
        assert_eq!(
            polls.next_due(ids, start + Duration::from_secs(60)),
            Some("acct_a")
        );
        // The longer expiry is respected too.
        assert_eq!(
            polls.next_due(["acct_b"], start + Duration::from_secs(119)),
            None
        );
        assert_eq!(
            polls.next_due(ids, start + Duration::from_secs(200)),
            Some("acct_a")
        );
        assert_eq!(polls.record("acct_b", true, secs(60), start), Some(false));
        assert_eq!(polls.online("acct_b"), Some(true));
        // Characters whose status is unavailable are never asked about.
        assert_eq!(polls.next_due([], start), None);
    }

    #[test]
    fn failed_online_polls_back_off_and_leave_status_unknown() {
        let start = Instant::now();
        let mut polls = OnlinePolls::default();
        polls.record("acct_a", false, secs(0), start);
        polls.record_failure("acct_a", start);
        assert_eq!(polls.online("acct_a"), None);
        assert_eq!(polls.presence("acct_a", true), Presence::Unknown);
        assert_eq!(
            polls.next_due(["acct_a"], start + Duration::from_secs(59)),
            None
        );
        assert!(
            polls
                .next_due(["acct_a"], start + Duration::from_secs(60))
                .is_some()
        );
        polls.record_failure("acct_a", start);
        assert!(
            polls
                .next_due(["acct_a"], start + Duration::from_secs(60))
                .is_none()
        );
        assert!(
            polls
                .next_due(["acct_a"], start + Duration::from_secs(120))
                .is_some()
        );
        for _ in 0..20 {
            polls.record_failure("acct_a", start);
        }
        assert!(
            polls
                .next_due(["acct_a"], start + Duration::from_secs(900))
                .is_some()
        );
        // Once known again, the status is followed as before.
        polls.record("acct_a", true, secs(0), start);
        assert_eq!(polls.presence("acct_a", true), Presence::Online);
    }

    #[test]
    fn presence_needs_the_scope_and_an_answer() {
        let start = Instant::now();
        let mut polls = OnlinePolls::default();
        assert_eq!(polls.presence("acct_a", true), Presence::Unknown);
        polls.record("acct_a", true, secs(0), start);
        assert_eq!(polls.presence("acct_a", true), Presence::Online);
        assert_eq!(polls.presence("acct_a", false), Presence::Unknown);
        polls.record("acct_a", false, secs(0), start);
        assert_eq!(polls.presence("acct_a", true), Presence::Offline);
        polls.forget("acct_a");
        assert_eq!(polls.presence("acct_a", true), Presence::Unknown);
    }

    #[test]
    fn locations_wait_for_the_first_online_status_only() {
        let start = Instant::now();
        let mut polls = OnlinePolls::default();
        // Without online status, locations are polled as always.
        assert!(polls.wants_location_poll("acct_a", false));
        // The first location waits for ESI to say whether they are logged in.
        assert!(!polls.wants_location_poll("acct_a", true));

        // Logged off or not, they are polled from then on.
        polls.record("acct_a", false, secs(0), start);
        assert!(polls.wants_location_poll("acct_a", true));
        polls.record("acct_a", true, secs(0), start);
        assert!(polls.wants_location_poll("acct_a", true));

        // A failed status check keeps polling too.
        polls.record_failure("acct_a", start);
        assert!(polls.wants_location_poll("acct_a", true));
        // Starting over waits for the status again.
        polls.forget("acct_a");
        assert!(!polls.wants_location_poll("acct_a", true));
    }
}
