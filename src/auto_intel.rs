//! Intel Nomadeer gathers on its own as recon pilots move: killboard refreshes queued
//! when one arrives somewhere, and Umbra's briefing once the fresh assessment is in.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::local_intel;
use crate::model::SystemAssessment;
use crate::util::HOUR_SECS;

/// The assessment weighs the last hour most, so after an hour it no longer describes
/// the system as it is now.
pub(crate) const ASSESSMENT_STALE_SECS: u64 = HOUR_SECS;
/// Systems waiting for a refresh; beyond this the oldest arrivals are dropped, as the
/// characters have long moved on from them. The player's own requests are never dropped.
const MAX_WAITING: usize = 8;
/// Briefings Umbra still owes; beyond this the oldest requests are dropped, so she does
/// not spend long after a roam briefing systems the characters left behind.
const MAX_PENDING_BRIEFINGS: usize = 8;
/// Pause between queued refreshes, on top of the killboard's own paging delay.
const QUEUE_GAP: Duration = Duration::from_secs(2);
/// An arrival this soon after a refresh finished does not fetch the system again.
const RECENTLY_REFRESHED: Duration = Duration::from_secs(60);

pub(crate) fn assessment_is_stale(generated_at: u64, current_time: u64) -> bool {
    current_time.saturating_sub(generated_at) > ASSESSMENT_STALE_SECS
}

/// Whether a character is logged in, as far as Nomadeer can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presence {
    Online,
    Offline,
    /// Their sign-in cannot read it, or ESI has not said.
    Unknown,
}

/// A character's location as just reported by ESI.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LocationFix<'a> {
    pub(crate) previous_system_id: &'a str,
    pub(crate) system_id: &'a str,
    /// The character's first location since Nomadeer started, or since they logged in.
    pub(crate) first_fix_this_session: bool,
    /// The system was not on the map before.
    pub(crate) node_created: bool,
    /// The system is the one selected on screen.
    pub(crate) on_screen: bool,
}

/// Whether a location update brings a recon pilot into a system. Other characters
/// never arrive anywhere as far as intel goes, and an offline pilot cannot move.
///
/// A jump (including one made while Nomadeer was closed) or a system new to the map is
/// an arrival. So is an online pilot's system when Nomadeer starts or they log in, as
/// that is where they are flying now. Without online status, a pilot found where they
/// were last seen only counts for the system on screen, as the player opens Nomadeer to
/// see it.
pub(crate) fn is_arrival(recon: bool, presence: Presence, fix: &LocationFix<'_>) -> bool {
    let moved = fix.node_created || fix.previous_system_id != fix.system_id;
    recon
        && match presence {
            Presence::Offline => false,
            Presence::Online => moved || fix.first_fix_this_session,
            Presence::Unknown => moved || (fix.first_fix_this_session && fix.on_screen),
        }
}

/// Whether a character has just logged in. The first answer of the session is judged
/// by the launch rule instead.
pub(crate) fn logged_in(was_online: Option<bool>, online: bool) -> bool {
    online && was_online == Some(false)
}

/// Whether an arrival should gather intel: the system has never been assessed, or
/// its assessment no longer describes it.
pub(crate) fn wants_intel_on_arrival(
    solar_system_id: u64,
    assessed_at: Option<u64>,
    current_time: u64,
) -> bool {
    solar_system_id != 0
        && assessed_at.is_none_or(|generated_at| assessment_is_stale(generated_at, current_time))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntelRequest {
    /// A character arrived; gathered in the background.
    Arrival,
    /// The player asked for it, so it goes ahead of arrivals.
    Player,
}

/// Killboard refreshes, one at a time. Player requests come first, then arrivals,
/// newest first within each.
#[derive(Debug, Default)]
pub(crate) struct IntelQueue {
    waiting: VecDeque<(u64, IntelRequest)>,
    in_flight: Option<(u64, IntelRequest)>,
    finished_at: HashMap<u64, Instant>,
    next_start: Option<Instant>,
}

impl IntelQueue {
    /// Queues a refresh of the system and returns whether one is now on its way.
    pub(crate) fn push(
        &mut self,
        solar_system_id: u64,
        request: IntelRequest,
        at: Instant,
    ) -> bool {
        if solar_system_id == 0 {
            return false;
        }
        if self.in_flight_system() == Some(solar_system_id) {
            return true;
        }
        let waiting_request = self
            .waiting
            .iter()
            .find(|(id, _)| *id == solar_system_id)
            .map(|(_, request)| *request);
        if request == IntelRequest::Arrival {
            let recently_refreshed =
                self.finished_at
                    .get(&solar_system_id)
                    .is_some_and(|finished| {
                        at.saturating_duration_since(*finished) < RECENTLY_REFRESHED
                    });
            if recently_refreshed && waiting_request.is_none() {
                return false;
            }
            // The player's own request already holds a better place.
            if waiting_request == Some(IntelRequest::Player) {
                return true;
            }
        }
        self.waiting.retain(|(id, _)| *id != solar_system_id);
        let position = match request {
            IntelRequest::Player => 0,
            IntelRequest::Arrival => self
                .waiting
                .iter()
                .take_while(|(_, request)| *request == IntelRequest::Player)
                .count(),
        };
        self.waiting.insert(position, (solar_system_id, request));
        while self.waiting.len() > MAX_WAITING {
            let Some(oldest_arrival) = self
                .waiting
                .iter()
                .rposition(|(_, request)| *request == IntelRequest::Arrival)
            else {
                break;
            };
            self.waiting.remove(oldest_arrival);
        }
        self.place(solar_system_id).is_some()
    }

    /// Takes the next system to refresh, once nothing is in flight and the pause after
    /// the last one has passed. Systems no longer mapped are skipped.
    pub(crate) fn start_next(
        &mut self,
        at: Instant,
        is_mapped: impl Fn(u64) -> bool,
    ) -> Option<(u64, IntelRequest)> {
        if self.in_flight.is_some() || self.next_start.is_some_and(|start| at < start) {
            return None;
        }
        while let Some(next) = self.waiting.pop_front() {
            if is_mapped(next.0) {
                self.in_flight = Some(next);
                return Some(next);
            }
        }
        None
    }

    /// Ends the refresh in flight and returns what it was.
    pub(crate) fn finish(&mut self, at: Instant) -> Option<(u64, IntelRequest)> {
        let done = self.in_flight.take();
        self.finished_at
            .retain(|_, finished| at.saturating_duration_since(*finished) < RECENTLY_REFRESHED);
        if let Some((solar_system_id, _)) = done {
            self.finished_at.insert(solar_system_id, at);
        }
        self.next_start = Some(at + QUEUE_GAP);
        done
    }

    /// Lets the next arrival in the system gather intel again, e.g. when the last
    /// refresh found the system gone from the map and could not be kept.
    pub(crate) fn forget_refresh(&mut self, solar_system_id: u64) {
        self.finished_at.remove(&solar_system_id);
    }

    /// Forgets waiting arrivals, e.g. once the player stops intel on arrival. The
    /// player's own requests stay.
    pub(crate) fn drop_arrivals(&mut self) {
        self.waiting
            .retain(|(_, request)| *request == IntelRequest::Player);
    }

    pub(crate) fn in_flight(&self) -> Option<(u64, IntelRequest)> {
        self.in_flight
    }

    pub(crate) fn in_flight_system(&self) -> Option<u64> {
        self.in_flight.map(|(solar_system_id, _)| solar_system_id)
    }

    /// How many refreshes wait ahead of the system, when it is waiting.
    pub(crate) fn place(&self, solar_system_id: u64) -> Option<usize> {
        self.waiting
            .iter()
            .position(|(id, _)| *id == solar_system_id)
    }
}

/// Briefings Umbra writes on her own once fresh intel is in, at most one per system
/// and only the newest few.
#[derive(Debug, Default)]
pub(crate) struct AutoBriefings {
    pending: Vec<PendingBriefing>,
    requests: u64,
}

#[derive(Debug)]
struct PendingBriefing {
    source: SystemAssessment,
    order: u64,
}

impl AutoBriefings {
    /// Asks for a briefing on a fresh assessment, replacing any older request for the
    /// same system.
    pub(crate) fn request(&mut self, source: SystemAssessment) {
        self.pending
            .retain(|pending| pending.source.system_id != source.system_id);
        self.requests += 1;
        self.pending.push(PendingBriefing {
            source,
            order: self.requests,
        });
        if self.pending.len() > MAX_PENDING_BRIEFINGS
            && let Some(oldest) = self
                .pending
                .iter()
                .enumerate()
                .min_by_key(|(_, pending)| pending.order)
                .map(|(index, _)| index)
        {
            self.pending.swap_remove(oldest);
        }
    }

    /// The assessment to brief next: the selected system first, then the newest
    /// request. Requests whose assessment was replaced or already briefed are dropped.
    pub(crate) fn next(
        &mut self,
        current: &HashMap<String, SystemAssessment>,
        selected_system_id: &str,
    ) -> Option<SystemAssessment> {
        self.pending.retain(|pending| {
            current
                .get(&pending.source.system_id)
                .is_some_and(|assessment| {
                    !assessment.model_generated
                        && local_intel::same_facts(assessment, &pending.source)
                })
        });
        let index = self
            .pending
            .iter()
            .enumerate()
            .max_by_key(|(_, pending)| {
                (
                    pending.source.system_id == selected_system_id,
                    pending.order,
                )
            })
            .map(|(index, _)| index)?;
        let chosen = self.pending.swap_remove(index);
        current.get(&chosen.source.system_id).cloned()
    }

    pub(crate) fn is_pending(&self, system_id: &str) -> bool {
        self.pending
            .iter()
            .any(|pending| pending.source.system_id == system_id)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// How many briefings she still owes.
    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }

    pub(crate) fn clear(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use super::{
        ASSESSMENT_STALE_SECS, AutoBriefings, IntelQueue, IntelRequest, LocationFix, Presence,
        is_arrival, logged_in, wants_intel_on_arrival,
    };
    use crate::model::SystemAssessment;

    const NOW: u64 = 1_760_000_000;

    fn mapped(_: u64) -> bool {
        true
    }

    fn drain(queue: &mut IntelQueue, at: Instant) -> Vec<u64> {
        let mut order = Vec::new();
        let mut at = at;
        while let Some((solar_system_id, _)) = queue.start_next(at, mapped) {
            order.push(solar_system_id);
            queue.finish(at);
            at += Duration::from_secs(10);
        }
        order
    }

    fn fix<'a>(
        previous_system_id: &'a str,
        system_id: &'a str,
        first_fix_this_session: bool,
        node_created: bool,
        on_screen: bool,
    ) -> LocationFix<'a> {
        LocationFix {
            previous_system_id,
            system_id,
            first_fix_this_session,
            node_created,
            on_screen,
        }
    }

    #[test]
    fn arrivals_without_online_status_are_jumps_and_new_systems() {
        let arrives = |fix: LocationFix<'_>| is_arrival(true, Presence::Unknown, &fix);
        assert!(arrives(fix("", "sys_a", false, false, false)));
        assert!(arrives(fix("sys_a", "sys_b", false, false, false)));
        // A jump made while Nomadeer was closed counts on the first fix.
        assert!(arrives(fix("sys_a", "sys_b", true, false, false)));
        assert!(arrives(fix("sys_a", "sys_a", false, true, false)));
        // On launch, only the system on screen counts for a character still there.
        assert!(arrives(fix("sys_a", "sys_a", true, false, true)));
        assert!(!arrives(fix("sys_a", "sys_a", true, false, false)));
        // Sitting in the same system between location polls is not an arrival.
        assert!(!arrives(fix("sys_a", "sys_a", false, false, true)));
    }

    #[test]
    fn only_recon_pilots_arrive() {
        let jump = fix("sys_a", "sys_b", false, false, true);
        for presence in [Presence::Online, Presence::Offline, Presence::Unknown] {
            assert!(!is_arrival(false, presence, &jump));
        }
        assert!(!is_arrival(
            false,
            Presence::Online,
            &fix("", "sys_a", true, true, true)
        ));
        assert!(is_arrival(true, Presence::Online, &jump));
    }

    #[test]
    fn online_pilots_arrive_where_they_are_at_launch_and_offline_ones_never() {
        let launch = fix("sys_a", "sys_a", true, false, false);
        assert!(is_arrival(true, Presence::Online, &launch));
        assert!(!is_arrival(true, Presence::Unknown, &launch));
        // Between polls, sitting still is not an arrival even when online.
        assert!(!is_arrival(
            true,
            Presence::Online,
            &fix("sys_a", "sys_a", false, false, true)
        ));
        assert!(is_arrival(
            true,
            Presence::Online,
            &fix("sys_a", "sys_b", false, false, false)
        ));
        // An offline pilot found somewhere new moved while logged off; nobody is there.
        for offline in [
            launch,
            fix("sys_a", "sys_b", true, false, true),
            fix("", "sys_a", true, true, true),
        ] {
            assert!(!is_arrival(true, Presence::Offline, &offline));
        }
    }

    #[test]
    fn logging_in_needs_a_known_logoff() {
        assert!(logged_in(Some(false), true));
        // The first answer of the session is judged by the launch rule instead.
        assert!(!logged_in(None, true));
        assert!(!logged_in(Some(true), true));
        assert!(!logged_in(Some(true), false));
        assert!(!logged_in(Some(false), false));
    }

    #[test]
    fn arrivals_gather_intel_only_without_a_fresh_assessment() {
        assert!(wants_intel_on_arrival(31_002_238, None, NOW));
        assert!(wants_intel_on_arrival(
            31_002_238,
            Some(NOW - ASSESSMENT_STALE_SECS - 1),
            NOW
        ));
        assert!(!wants_intel_on_arrival(
            31_002_238,
            Some(NOW - ASSESSMENT_STALE_SECS),
            NOW
        ));
        assert!(!wants_intel_on_arrival(31_002_238, Some(NOW - 60), NOW));
        // A system placed on the map by hand has no EVE system to look up.
        assert!(!wants_intel_on_arrival(0, None, NOW));
    }

    #[test]
    fn queue_runs_newest_arrival_first_without_duplicates() {
        let start = Instant::now();
        let mut queue = IntelQueue::default();
        assert!(queue.push(1, IntelRequest::Arrival, start));
        assert!(queue.push(2, IntelRequest::Arrival, start));
        assert!(queue.push(3, IntelRequest::Arrival, start));
        // Coming back to a waiting system moves it to the front, once.
        assert!(queue.push(1, IntelRequest::Arrival, start));
        assert_eq!(queue.place(1), Some(0));
        assert_eq!(queue.place(3), Some(1));
        assert_eq!(drain(&mut queue, start), vec![1, 3, 2]);
    }

    #[test]
    fn player_requests_jump_the_queue() {
        let start = Instant::now();
        let mut queue = IntelQueue::default();
        queue.push(1, IntelRequest::Arrival, start);
        queue.push(2, IntelRequest::Arrival, start);
        queue.push(3, IntelRequest::Player, start);
        // Later arrivals still wait behind the player's request.
        queue.push(4, IntelRequest::Arrival, start);
        // An arrival does not demote the player's own request.
        queue.push(3, IntelRequest::Arrival, start);
        assert_eq!(drain(&mut queue, start), vec![3, 4, 2, 1]);

        queue.push(5, IntelRequest::Arrival, start + Duration::from_secs(100));
        queue.push(6, IntelRequest::Player, start + Duration::from_secs(100));
        queue.drop_arrivals();
        assert_eq!(queue.place(5), None);
        assert_eq!(queue.place(6), Some(0));
    }

    #[test]
    fn queue_runs_one_refresh_at_a_time_with_a_pause_between() {
        let start = Instant::now();
        let mut queue = IntelQueue::default();
        queue.push(1, IntelRequest::Arrival, start);
        queue.push(2, IntelRequest::Arrival, start);
        assert_eq!(
            queue.start_next(start, mapped),
            Some((2, IntelRequest::Arrival))
        );
        assert_eq!(queue.start_next(start, mapped), None);
        assert_eq!(queue.finish(start), Some((2, IntelRequest::Arrival)));
        assert_eq!(queue.start_next(start, mapped), None);
        assert_eq!(
            queue.start_next(start + Duration::from_secs(2), mapped),
            Some((1, IntelRequest::Arrival))
        );
    }

    #[test]
    fn systems_in_flight_or_just_refreshed_are_not_queued_again() {
        let start = Instant::now();
        let mut queue = IntelQueue::default();
        queue.push(1, IntelRequest::Arrival, start);
        queue.start_next(start, mapped);
        assert!(queue.push(1, IntelRequest::Arrival, start));
        assert!(queue.push(1, IntelRequest::Player, start));
        assert_eq!(queue.place(1), None);

        queue.finish(start);
        let soon = start + Duration::from_secs(5);
        assert!(!queue.push(1, IntelRequest::Arrival, soon));
        assert_eq!(queue.place(1), None);
        // The player can always ask again, and later arrivals refresh it too.
        assert!(queue.push(1, IntelRequest::Player, soon));
        let mut later = IntelQueue::default();
        later.push(1, IntelRequest::Arrival, start);
        later.start_next(start, mapped);
        later.finish(start);
        assert!(later.push(1, IntelRequest::Arrival, start + Duration::from_secs(120)));
    }

    #[test]
    fn a_refresh_that_was_not_kept_does_not_hold_back_the_next_arrival() {
        let start = Instant::now();
        let mut queue = IntelQueue::default();
        queue.push(1, IntelRequest::Arrival, start);
        queue.start_next(start, mapped);
        queue.finish(start);
        queue.forget_refresh(1);
        assert!(queue.push(1, IntelRequest::Arrival, start + Duration::from_secs(5)));
    }

    #[test]
    fn a_full_queue_never_drops_the_players_requests() {
        let start = Instant::now();
        let mut queue = IntelQueue::default();
        queue.push(100, IntelRequest::Arrival, start);
        for solar_system_id in 1..=9 {
            assert!(queue.push(solar_system_id, IntelRequest::Player, start));
        }
        assert_eq!(queue.place(100), None);
        assert_eq!(queue.place(1), Some(8));
        assert!(!queue.push(101, IntelRequest::Arrival, start));
        assert_eq!(drain(&mut queue, start), (1..=9).rev().collect::<Vec<_>>());
    }

    #[test]
    fn queue_skips_systems_no_longer_mapped_and_drops_old_arrivals() {
        let start = Instant::now();
        let mut queue = IntelQueue::default();
        for solar_system_id in 1..=10 {
            queue.push(solar_system_id, IntelRequest::Arrival, start);
        }
        assert_eq!(queue.place(10), Some(0));
        assert_eq!(queue.place(2), None);
        assert_eq!(queue.place(1), None);
        assert_eq!(
            queue.start_next(start, |solar_system_id| solar_system_id != 10),
            Some((9, IntelRequest::Arrival))
        );
        assert!(!queue.push(0, IntelRequest::Player, start));
    }

    fn assessment(system_id: &str, threat: u8) -> SystemAssessment {
        SystemAssessment {
            system_id: system_id.to_owned(),
            threat_score: threat,
            activity_score: 10,
            confidence: "Medium".to_owned(),
            evidence: vec![format!("threat {threat}")],
            briefing: String::new(),
            model_generated: false,
            generated_at: NOW,
        }
    }

    fn current(assessments: &[SystemAssessment]) -> HashMap<String, SystemAssessment> {
        assessments
            .iter()
            .map(|assessment| (assessment.system_id.clone(), assessment.clone()))
            .collect()
    }

    #[test]
    fn auto_briefings_prefer_the_selected_system_then_the_newest() {
        let mut briefings = AutoBriefings::default();
        let assessments = [
            assessment("a", 10),
            assessment("b", 20),
            assessment("c", 30),
        ];
        for assessment in &assessments {
            briefings.request(assessment.clone());
        }
        let current = current(&assessments);
        let next = |briefings: &mut AutoBriefings, selected: &str| {
            briefings
                .next(&current, selected)
                .map(|assessment| assessment.system_id)
        };
        assert_eq!(next(&mut briefings, "a").as_deref(), Some("a"));
        assert_eq!(next(&mut briefings, "a").as_deref(), Some("c"));
        assert!(briefings.is_pending("b"));
        assert_eq!(next(&mut briefings, "x").as_deref(), Some("b"));
        assert!(briefings.is_empty());
        assert_eq!(next(&mut briefings, "x"), None);
    }

    #[test]
    fn auto_briefings_keep_one_request_per_system() {
        let mut briefings = AutoBriefings::default();
        briefings.request(assessment("a", 10));
        briefings.request(assessment("b", 20));
        briefings.request(assessment("a", 40));
        assert_eq!(briefings.len(), 2);
        let current = current(&[assessment("a", 40), assessment("b", 20)]);
        // The newer request for "a" is now the newest.
        let first = briefings.next(&current, "").unwrap();
        assert_eq!((first.system_id.as_str(), first.threat_score), ("a", 40));
        assert_eq!(briefings.next(&current, "").unwrap().system_id, "b");
        assert_eq!(briefings.next(&current, ""), None);
    }

    #[test]
    fn auto_briefings_keep_only_the_newest_requests() {
        let mut briefings = AutoBriefings::default();
        let assessments: Vec<_> = (0..10)
            .map(|index| assessment(&format!("sys_{index}"), index))
            .collect();
        for assessment in &assessments {
            briefings.request(assessment.clone());
        }
        assert!(!briefings.is_pending("sys_0"));
        assert!(!briefings.is_pending("sys_1"));
        assert!((2..10).all(|index| briefings.is_pending(&format!("sys_{index}"))));
        let current = current(&assessments);
        let mut briefed = 0;
        while briefings.next(&current, "").is_some() {
            briefed += 1;
        }
        assert_eq!(briefed, 8);
    }

    #[test]
    fn auto_briefings_drop_replaced_or_briefed_assessments() {
        let mut briefings = AutoBriefings::default();
        briefings.request(assessment("replaced", 10));
        briefings.request(assessment("briefed", 20));
        briefings.request(assessment("removed", 30));
        briefings.request(assessment("refreshed", 40));
        let mut briefed = assessment("briefed", 20);
        briefed.model_generated = true;
        // A refresh that found the same facts is briefed as it stands now.
        let mut refreshed = assessment("refreshed", 40);
        refreshed.generated_at = NOW + 300;
        let current = current(&[assessment("replaced", 90), briefed, refreshed]);
        let next = briefings.next(&current, "replaced").unwrap();
        assert_eq!(next.system_id, "refreshed");
        assert_eq!(next.generated_at, NOW + 300);
        assert!(briefings.is_empty());
    }
}
