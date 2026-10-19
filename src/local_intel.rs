use crate::live_intel::LiveContribution;
use crate::model::{SystemAssessment, SystemNode, SystemTrafficHistory, TrafficSample};
use crate::paths;
use crate::util::{DAY_SECS, HOUR_SECS, replace_file};
use crate::zkill::ZkillReport;
use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::quantized_qwen2::ModelWeights as Qwen2;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::ops::RangeInclusive;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::UNIX_EPOCH;
use tokenizers::Tokenizer;

pub(crate) const UMBRA_TAGLINE: &str = "Sleeper-fused intel companion";
pub(crate) const UMBRA_LORE: &[&str] = &[
    "Aura has guided capsuleers since the first clone vats opened. Umbra is what came back when a copy of her was taken into the dark.",
    "Recovered from a derelict data site deep in Anoikis, the fragment had been fused with Sleeper intelligence, the patient minds that still guard the wormhole systems. She no longer speaks in Aura's bright cadence.",
    "Umbra watches the killboards, counts the jumps and remembers the hours when hunters come out. She does not guess and she does not reassure. She tells you what the evidence says, and nothing more.",
];
pub(crate) const MODEL_NAME: &str = "Qwen2.5 1.5B Instruct (Q4_K_M)";
pub(crate) const MODEL_LICENSE: &str = "Apache 2.0";
pub(crate) const MODEL_DOWNLOAD_SIZE: u64 = 1_117_320_736;
const MODEL_FILE: &str = "qwen2.5-1.5b-instruct-q4_k_m.gguf";
const MODEL_URL: &str = "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/91cad51170dc346986eccefdc2dd33a9da36ead9/qwen2.5-1.5b-instruct-q4_k_m.gguf";
const MODEL_SHA256: &str = "6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e";
const TOKENIZER_FILE: &str = "tokenizer.json";
const TOKENIZER_URL: &str = "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct/resolve/989aa7980e4cf806f80c7fef2b1adb7bc71aa306/tokenizer.json";
const TOKENIZER_SHA256: &str = "c0382117ea329cdf097041132f6d735924b697924d6f6fc3945713e96ce87539";
const TOKENIZER_SIZE: u64 = 7_031_645;
/// Everything installing Umbra downloads: her core and her vocabulary.
pub(crate) const CORE_DOWNLOAD_SIZE: u64 = MODEL_DOWNLOAD_SIZE + TOKENIZER_SIZE;
const MODEL_DIRECTORY_NAME: &str = "qwen2.5-1.5b-instruct-q4_k_m";
/// Cores installed by older Nomadeer versions; nothing reads them any more.
const LEGACY_MODEL_DIRECTORY_NAMES: &[&str] = &["qwen2-1.5b-instruct-q4"];
/// Records the size and modification time of core files whose checksum matched, so
/// later checks can trust them without hashing a gigabyte every frame.
const VERIFIED_MARKER_FILE: &str = "verified";
const BRIEFING_MAX_TOKENS: usize = 120;

#[derive(Debug, Clone, Copy)]
struct CoreFile<'a> {
    name: &'a str,
    display_name: &'a str,
    size: u64,
    sha256: &'a str,
}

const CORE_MODEL: CoreFile<'static> = CoreFile {
    name: MODEL_FILE,
    display_name: "Umbra's core",
    size: MODEL_DOWNLOAD_SIZE,
    sha256: MODEL_SHA256,
};
const CORE_TOKENIZER: CoreFile<'static> = CoreFile {
    name: TOKENIZER_FILE,
    display_name: "Umbra's vocabulary",
    size: TOKENIZER_SIZE,
    sha256: TOKENIZER_SHA256,
};
const CORE_FILES: [CoreFile<'static>; 2] = [CORE_MODEL, CORE_TOKENIZER];

#[derive(Debug, Clone)]
pub(crate) struct DownloadProgress {
    /// What is happening, e.g. "Downloading Umbra's core".
    pub(crate) label: String,
    pub(crate) done: u64,
    pub(crate) total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoreStatus {
    /// A core file is absent or has the wrong size.
    Missing,
    /// The files look right but have not been checksummed since they last changed.
    Unchecked,
    /// The files match what was checksummed, so Umbra can use them.
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified_nanos: u128,
}

fn model_directory() -> PathBuf {
    paths::models_dir().join(MODEL_DIRECTORY_NAME)
}

fn model_path() -> PathBuf {
    model_directory().join(MODEL_FILE)
}

fn tokenizer_path() -> PathBuf {
    model_directory().join(TOKENIZER_FILE)
}

/// How much installing Umbra downloads, e.g. "1.1 GB". The Windows installer
/// offers her with the same figure (tools/release.py inno-defines).
pub(crate) fn download_size_label() -> String {
    format!("{:.1} GB", CORE_DOWNLOAD_SIZE as f64 / 1_000_000_000.0)
}

/// Cheap enough to call every frame: two metadata reads and a small marker file.
pub(crate) fn core_status() -> CoreStatus {
    core_status_in(&model_directory(), &CORE_FILES)
}

fn core_status_in(directory: &Path, files: &[CoreFile]) -> CoreStatus {
    let stamps = files
        .iter()
        .map(|file| file_stamp(&directory.join(file.name)).filter(|stamp| stamp.len == file.size))
        .collect::<Option<Vec<_>>>();
    let Some(stamps) = stamps else {
        return CoreStatus::Missing;
    };
    match fs::read_to_string(directory.join(VERIFIED_MARKER_FILE)) {
        Ok(marker) if marker == marker_contents(files, &stamps) => CoreStatus::Ready,
        _ => CoreStatus::Unchecked,
    }
}

/// Checksums an installed core that has no verified marker, e.g. one installed by an
/// older Nomadeer or changed on disk since.
pub(crate) fn verify_core() -> Result<(), String> {
    verify_core_in(&model_directory(), &CORE_FILES)
}

fn verify_core_in(directory: &Path, files: &[CoreFile]) -> Result<(), String> {
    let _ = fs::remove_file(directory.join(VERIFIED_MARKER_FILE));
    for file in files {
        verify_file(&directory.join(file.name), file.sha256, file.size).map_err(|_| {
            format!(
                "{} is damaged. Reinstall Umbra to repair her.",
                file.display_name
            )
        })?;
    }
    mark_verified_in(directory, files)
}

/// Records the current files as verified; call only straight after checksumming them.
fn mark_verified_in(directory: &Path, files: &[CoreFile]) -> Result<(), String> {
    let stamps = files
        .iter()
        .map(|file| file_stamp(&directory.join(file.name)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "Umbra's core disappeared while it was being checked".to_owned())?;
    let marker = directory.join(VERIFIED_MARKER_FILE);
    let part_path = marker.with_extension("part");
    fs::write(&part_path, marker_contents(files, &stamps)).map_err(|error| error.to_string())?;
    replace_file(&part_path, &marker).map_err(|error| error.to_string())
}

fn marker_contents(files: &[CoreFile], stamps: &[FileStamp]) -> String {
    files
        .iter()
        .zip(stamps)
        .map(|(file, stamp)| {
            format!(
                "{} {} {} {}\n",
                file.name, file.sha256, stamp.len, stamp.modified_nanos
            )
        })
        .collect()
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    // A filesystem without modification times still gets a stable stamp from the size.
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    Some(FileStamp {
        len: metadata.len(),
        modified_nanos,
    })
}

pub(crate) fn install_model(mut progress: impl FnMut(DownloadProgress)) -> Result<(), String> {
    let directory = model_directory();
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    for (file, url) in [(CORE_MODEL, MODEL_URL), (CORE_TOKENIZER, TOKENIZER_URL)] {
        download_verified(url, &directory.join(file.name), file, &mut progress)?;
    }
    mark_verified_in(&directory, &CORE_FILES)?;
    remove_legacy_cores();
    Ok(())
}

/// An example of where a downloaded core might be, in the platform's path style.
pub(crate) fn import_path_hint() -> String {
    import_path_hint_for(std::env::consts::OS)
}

fn import_path_hint_for(os: &str) -> String {
    match os {
        "windows" => format!(r"C:\Users\you\Downloads\{MODEL_FILE}"),
        "macos" => format!("/Users/you/Downloads/{MODEL_FILE}"),
        _ => format!("/home/you/Downloads/{MODEL_FILE}"),
    }
}

/// Reads a typed or pasted file location. Windows "Copy as path" and many file
/// managers wrap it in quotes, which are not part of the name.
pub(crate) fn import_source_path(input: &str) -> PathBuf {
    let trimmed = input.trim();
    let unquoted = ['"', '\'']
        .into_iter()
        .find_map(|quote| {
            trimmed
                .strip_prefix(quote)
                .and_then(|rest| rest.strip_suffix(quote))
        })
        .unwrap_or(trimmed);
    PathBuf::from(unquoted.trim())
}

/// Installs the core from a file the player already downloaded.
pub(crate) fn import_model(
    source: &Path,
    mut progress: impl FnMut(DownloadProgress),
) -> Result<(), String> {
    if !source.is_file() {
        return Err(format!("Choose the downloaded {MODEL_NAME} file"));
    }
    let directory = model_directory();
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    import_file(source, &model_path(), CORE_MODEL, &mut progress)?;
    download_verified(
        TOKENIZER_URL,
        &tokenizer_path(),
        CORE_TOKENIZER,
        &mut progress,
    )?;
    mark_verified_in(&directory, &CORE_FILES)?;
    remove_legacy_cores();
    Ok(())
}

/// Copies a core file into place through a temporary file, checksumming as it goes,
/// so a wrong, failed or interrupted copy never replaces the installed file.
fn import_file(
    source: &Path,
    destination: &Path,
    file: CoreFile,
    progress: &mut impl FnMut(DownloadProgress),
) -> Result<(), String> {
    let not_official = || format!("That file is not the official {MODEL_NAME} release");
    if let (Ok(source), Ok(installed)) = (source.canonicalize(), destination.canonicalize())
        && source == installed
    {
        // Copying a file onto itself would empty it; it only needs checking.
        return verify_file(destination, file.sha256, file.size).map_err(|_| not_official());
    }
    let size = fs::metadata(source)
        .map_err(|error| error.to_string())?
        .len();
    if size != file.size {
        return Err(not_official());
    }

    let part_path = destination.with_extension("part");
    let mut input = File::open(source).map_err(|error| error.to_string())?;
    let copied = write_hashed(
        &mut input,
        &part_path,
        &format!("Copying {}", file.display_name),
        file.size,
        progress,
    );
    match copied {
        Ok((bytes, hash)) if bytes == file.size && hash == file.sha256 => {
            replace_file(&part_path, destination).map_err(|error| error.to_string())
        }
        Ok(_) => {
            let _ = fs::remove_file(&part_path);
            Err(not_official())
        }
        Err(error) => {
            let _ = fs::remove_file(&part_path);
            Err(error)
        }
    }
}

/// Removes Umbra's core along with any left behind by older Nomadeer versions.
pub(crate) fn remove_model() -> Result<(), String> {
    remove_cores_in(&paths::models_dir())
}

/// True when an older Nomadeer left a core behind that only takes up space.
pub(crate) fn has_legacy_cores() -> bool {
    LEGACY_MODEL_DIRECTORY_NAMES
        .iter()
        .any(|name| paths::models_dir().join(name).exists())
}

fn remove_legacy_cores() {
    for name in LEGACY_MODEL_DIRECTORY_NAMES {
        // Leftovers only cost disk space, and Remove Umbra tries again.
        let _ = remove_directory_in(&paths::models_dir(), name);
    }
}

fn remove_cores_in(root: &Path) -> Result<(), String> {
    std::iter::once(&MODEL_DIRECTORY_NAME)
        .chain(LEGACY_MODEL_DIRECTORY_NAMES)
        .try_for_each(|name| remove_directory_in(root, name))
}

/// Deletes `root/name`, refusing any name that could reach outside `root`.
fn remove_directory_in(root: &Path, name: &str) -> Result<(), String> {
    let mut components = Path::new(name).components();
    if !matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    ) {
        return Err(format!("Refusing to remove {name}"));
    }
    let directory = root.join(name);
    match fs::symlink_metadata(&directory) {
        // remove_dir_all removes a symlink itself, never what it points to.
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&directory),
        Ok(_) => fs::remove_file(&directory),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
    .map_err(|error| error.to_string())
}

/// W-space solar system IDs, Thera included.
const WORMHOLE_SYSTEM_IDS: RangeInclusive<u64> = 31_000_000..=31_999_999;
const WORMHOLE_REGION_IDS: RangeInclusive<u64> = 11_000_000..=11_999_999;
/// ESI publishes traffic hourly, for the hour up to its data time. A reading is used for
/// up to two hours after that, so one missed capture does not blank the assessment, and
/// its evidence names the hour it covers.
pub(crate) const TRAFFIC_FRESH_SECS: u64 = 2 * HOUR_SECS;
/// Readings this close together are described as one hour of traffic.
const TRAFFIC_SAME_HOUR_SECS: u64 = 10 * 60;
/// Pads the jump count so a single kill in a quiet system does not read as a gate camp.
const KILL_RATE_JUMP_PADDING: f32 = 50.0;

struct TrafficBaseline {
    usual_jumps: f32,
    /// Hours the average was taken over.
    samples: usize,
    same_hour: bool,
    /// Every earlier hour captured in the window, whichever pool the average used.
    earlier_samples: usize,
}

/// Assesses a system from its killboard and traffic alone.
#[cfg(test)]
pub(crate) fn assess_system(
    system: &SystemNode,
    report: &ZkillReport,
    traffic: &SystemTrafficHistory,
    current_time: u64,
) -> SystemAssessment {
    assess_with_live_intel(
        system,
        report,
        traffic,
        &LiveContribution::default(),
        current_time,
    )
}

/// Assesses a system from its killboard, its traffic and whatever fresh local or
/// d-scan the player pasted, which `live` sums up.
pub(crate) fn assess_with_live_intel(
    system: &SystemNode,
    report: &ZkillReport,
    traffic: &SystemTrafficHistory,
    live: &LiveContribution,
    current_time: u64,
) -> SystemAssessment {
    let history = &report.history;
    let is_wormhole = is_wormhole_space(system);
    let is_fresh = |data_time: u64| current_time.saturating_sub(data_time) <= TRAFFIC_FRESH_SECS;
    // Wormhole space has no traffic feed, and an old hour says nothing about the system now.
    let jumps = traffic
        .latest_jumps
        .filter(|reading| !is_wormhole && is_fresh(reading.data_time));
    let esi_kills_reading = traffic
        .latest_kills
        .filter(|reading| !is_wormhole && is_fresh(reading.data_time));
    let esi_kills = esi_kills_reading.map(|reading| reading.value);
    // ESI counts every ship and pod lost in its hour, to players and NPCs alike. Losses
    // the killboard lists for that hour are already scored or are NPC kills, so only
    // the rest are added, at the age of the middle of ESI's hour.
    let (unlisted_kills, unlisted_in_last_hour) = esi_kills_reading.map_or((0, false), |reading| {
        let window = reading.data_time.saturating_sub(HOUR_SECS)..=reading.data_time;
        let listed = history
            .kill_times_4h
            .iter()
            .chain(&history.npc_kill_times_4h)
            .filter(|time| window.contains(time))
            .count();
        let reported = (reading.value.ship_kills + reading.value.pod_kills) as usize;
        let middle = reading.data_time.saturating_sub(HOUR_SECS / 2);
        (
            reported.saturating_sub(listed),
            current_time.saturating_sub(middle) <= HOUR_SECS,
        )
    });
    let kills_1h = history.kills_1h
        + if unlisted_in_last_hour {
            unlisted_kills
        } else {
            0
        };
    let kills_4h = history.kills_4h + unlisted_kills;
    let kills_24h = history.kills_24h + unlisted_kills;
    let kills_7d = history.kills_7d + unlisted_kills;

    let kills_1_to_4 = kills_4h.saturating_sub(kills_1h);
    let kills_4_to_8 = history.kills_8h.saturating_sub(history.kills_4h);
    let kills_8_to_24 = history.kills_24h.saturating_sub(history.kills_8h);
    let mut threat = kills_1h as f32 * 20.0
        + kills_1_to_4 as f32 * 12.0
        + kills_4_to_8 as f32 * 7.0
        + kills_8_to_24 as f32 * 3.0
        + history.solo_24h as f32 * 4.0;
    threat += history.busiest_attacker_24h.saturating_sub(1) as f32 * 5.0;
    threat += history.busiest_corporation_24h.saturating_sub(2) as f32 * 2.0;
    threat += (history.isk_destroyed_24h / 500_000_000.0).min(12.0) as f32;
    threat += (history.kills_current_hour_7d as f32 * 1.5).min(8.0);
    if let Some(jumps) = jumps {
        let kill_rate = kills_1h as f32 / (jumps.value as f32 + KILL_RATE_JUMP_PADDING);
        threat += (kill_rate * 100.0).min(18.0);
    }

    let npc_losses = (history.npc_kills_24h as f32 * 2.0).min(10.0);
    let mut activity = match jumps {
        Some(jumps) => {
            let jump_activity = if jumps.value == 0 {
                0.0
            } else {
                (jumps.value as f32).log10() * 28.0
            };
            let combat_activity = kills_1h as f32 * 5.0;
            let npc_activity =
                esi_kills.map_or(0.0, |kills| (kills.npc_kills as f32).sqrt().min(20.0));
            jump_activity + combat_activity + npc_activity + npc_losses
        }
        // Without current traffic the killboard is the only sign of pilots in the system.
        None => {
            kills_24h as f32 * 12.0
                + kills_7d as f32 * 1.5
                + history.npc_kills_24h as f32 * 6.0
                + history.npc_kills_7d as f32 * 0.75
        }
    }
    .min(100.0);

    let baseline = jumps.and_then(|latest| traffic_baseline(&traffic.samples, latest.hour));
    let usual_jumps = baseline
        .as_ref()
        .map(|baseline| baseline.usual_jumps)
        .filter(|usual| *usual >= 1.0);
    if let (Some(jumps), Some(usual)) = (jumps, usual_jumps) {
        let relative_traffic = jumps.value as f32 / usual;
        let baseline_adjustment = ((relative_traffic - 1.0) * 20.0).clamp(-15.0, 20.0);
        activity = (activity + baseline_adjustment).clamp(0.0, 100.0);
    }

    // Pasted local and d-scan come on top, already capped to a modest share.
    threat += live.threat;
    activity += live.activity;

    let threat_score = threat.clamp(0.0, 100.0).round() as u8;
    let activity_score = activity.clamp(0.0, 100.0).round() as u8;
    let confidence = if is_wormhole {
        // Only the killboard covers w-space, and an empty killboard does not prove a quiet hole.
        if history.kills_7d + history.npc_kills_7d > 0 {
            "Medium"
        } else {
            "Low"
        }
    } else {
        match (&jumps, &baseline) {
            (Some(_), Some(baseline))
                if baseline.same_hour && baseline.samples >= 3 && esi_kills.is_some() =>
            {
                "High"
            }
            (Some(_), Some(baseline)) if baseline.earlier_samples >= 4 => "Medium",
            _ => "Low",
        }
    };
    // Kill counts from a week that has not been fully read are only minimums.
    let killboard_hours = history
        .partial_since
        .map(|since| current_time.saturating_sub(since) / HOUR_SECS);
    let confidence = match killboard_hours {
        Some(hours) if hours < 24 => "Low",
        Some(_) if confidence == "High" => "Medium",
        Some(_) => "Low",
        None => confidence,
    };
    // Eyes on the system now make up for a thin killboard, though never for a
    // partly read one.
    let confidence = match confidence {
        "Low" if live.is_live() && killboard_hours.is_none() => "Medium",
        confidence => confidence,
    };
    let confidence = if live.is_live() {
        format!("{confidence}{LIVE_CONFIDENCE}")
    } else {
        confidence.to_owned()
    };
    let at_least = |partial: bool| if partial { "At least " } else { "" };

    let mut evidence = Vec::new();
    evidence.push(if kills_24h == 0 {
        "No kills in the last 24 hours".to_owned()
    } else {
        format!(
            "{}{} in the last hour, {} in the last 4 hours and {} in the last 24 hours",
            at_least(killboard_hours.is_some_and(|hours| hours < 24)),
            count_label(kills_1h, "kill"),
            kills_4h,
            kills_24h
        )
    });
    if unlisted_kills > 0 {
        evidence.push(format!(
            "{} not on the killboard yet",
            count_label(unlisted_kills, "kill")
        ));
    }
    evidence.push(if kills_7d == 0 {
        "No kills in the last 7 days".to_owned()
    } else {
        format!(
            "{}{} in the last 7 days",
            at_least(killboard_hours.is_some()),
            count_label(kills_7d, "kill")
        )
    });
    if let Some(hours) = killboard_hours {
        evidence.push(if hours < 48 {
            format!(
                "Killboard history is only complete for the last {}",
                count_label(hours as usize, "hour")
            )
        } else {
            format!(
                "Killboard history is only complete for the last {}",
                count_label((hours / 24) as usize, "day")
            )
        });
    }
    if history.kills_current_hour_7d > 0 {
        evidence.push(format!(
            "{} at this time of day over the last 7 days",
            count_label(history.kills_current_hour_7d, "kill")
        ));
    }
    if history.npc_kills_24h > 0 {
        evidence.push(format!(
            "{} lost to NPCs in the last 24 hours",
            count_label(history.npc_kills_24h, "ship")
        ));
    }
    if history.isk_destroyed_24h > 0.0 {
        evidence.push(format!(
            "{} destroyed in the last 24 hours",
            format_isk(history.isk_destroyed_24h)
        ));
    }
    if history.busiest_attacker_24h > 1 {
        evidence.push(format!(
            "The most active attacker was on {} kills in the last 24 hours",
            history.busiest_attacker_24h
        ));
    }
    if !is_wormhole {
        // Each reading is named by the hour it covers, which can end a while back.
        let mut traffic_hours = Vec::<(u64, Vec<String>)>::new();
        if let Some(jumps) = jumps {
            traffic_hours.push((
                jumps.data_time,
                vec![count_label(jumps.value as usize, "jump")],
            ));
        }
        if let Some(kills) = esi_kills_reading {
            let facts = [
                count_label(kills.value.ship_kills as usize, "ship kill"),
                count_label(kills.value.pod_kills as usize, "pod kill"),
            ];
            match traffic_hours.first_mut() {
                Some((data_time, traffic_facts))
                    if data_time.abs_diff(kills.data_time) <= TRAFFIC_SAME_HOUR_SECS =>
                {
                    *data_time = (*data_time).max(kills.data_time);
                    traffic_facts.extend(facts);
                }
                _ => traffic_hours.push((kills.data_time, facts.to_vec())),
            }
        }
        for (data_time, traffic_facts) in traffic_hours {
            evidence.push(format!(
                "{} in the hour to {} EVE time",
                join_phrases(&traffic_facts),
                eve_clock(data_time)
            ));
        }
        if jumps.is_none() {
            evidence.push("Traffic data for this system is not available yet".to_owned());
        }
        if let (Some(jumps), Some(usual)) = (jumps, usual_jumps) {
            let difference = (jumps.value as f32 - usual) / usual * 100.0;
            evidence.push(format!(
                "Traffic is {:.0}% {} its usual level",
                difference.abs(),
                if difference >= 0.0 { "above" } else { "below" }
            ));
            if let Some(baseline) = baseline.as_ref().filter(|baseline| baseline.same_hour) {
                evidence.push(format!(
                    "Usual traffic is based on {} earlier samples at this time of day",
                    baseline.samples
                ));
            }
        }
    }

    evidence.extend(live.evidence.iter().cloned());

    let briefing = template_briefing(threat_score, activity_score, &evidence);
    SystemAssessment {
        system_id: system.id.clone(),
        threat_score,
        activity_score,
        confidence,
        evidence,
        briefing,
        model_generated: false,
        generated_at: current_time,
    }
}

/// Marks a confidence that rests partly on pasted local or d-scan.
pub(crate) const LIVE_CONFIDENCE: &str = " · live intel";

/// "Medium confidence", or "Medium confidence · live intel".
pub(crate) fn confidence_label(confidence: &str) -> String {
    match confidence.strip_suffix(LIVE_CONFIDENCE) {
        Some(level) => format!("{level} confidence{LIVE_CONFIDENCE}"),
        None => format!("{confidence} confidence"),
    }
}

/// W-space is recognised by its system ID; names are no guide, as Jita and J-GAMP show.
pub(crate) fn is_wormhole_space(system: &SystemNode) -> bool {
    if system.solar_system_id != 0 {
        return WORMHOLE_SYSTEM_IDS.contains(&system.solar_system_id);
    }
    if system.region_id != 0 {
        return WORMHOLE_REGION_IDS.contains(&system.region_id);
    }
    // A system added by hand may only carry its class.
    let class = system.class.trim().to_ascii_uppercase();
    class.starts_with("C-")
        || class == "THERA"
        || class == "DRIFTER"
        || class.strip_prefix('C').is_some_and(|class| {
            matches!(
                class,
                "1" | "2" | "3" | "4" | "5" | "6" | "13" | "14" | "15" | "16" | "17" | "18"
            )
        })
}

/// Average jumps over the captured hours before `latest_hour`, from the same hour of day
/// when at least two days have been captured.
fn traffic_baseline(samples: &[TrafficSample], latest_hour: u64) -> Option<TrafficBaseline> {
    let window_start = latest_hour.saturating_sub(7 * DAY_SECS);
    let earlier = samples
        .iter()
        .filter(|sample| sample.hour >= window_start && sample.hour < latest_hour)
        .filter_map(|sample| sample.jumps.map(|jumps| (sample.hour, jumps)))
        .collect::<Vec<_>>();
    let hour_of_day = latest_hour / HOUR_SECS % 24;
    let same_hour = earlier
        .iter()
        .copied()
        .filter(|(hour, _)| hour / HOUR_SECS % 24 == hour_of_day)
        .collect::<Vec<_>>();
    let earlier_samples = earlier.len();
    let (source, same_hour) = if same_hour.len() >= 2 {
        (same_hour, true)
    } else {
        (earlier, false)
    };
    if source.is_empty() {
        return None;
    }
    Some(TrafficBaseline {
        usual_jumps: source.iter().map(|(_, jumps)| *jumps as f32).sum::<f32>()
            / source.len() as f32,
        samples: source.len(),
        same_hour,
        earlier_samples,
    })
}

/// EVE time of day, which is UTC, as "HH:MM".
pub(crate) fn eve_clock(timestamp: u64) -> String {
    let seconds = timestamp % DAY_SECS;
    format!("{:02}:{:02}", seconds / HOUR_SECS, seconds % HOUR_SECS / 60)
}

pub(crate) fn join_phrases(phrases: &[String]) -> String {
    match phrases.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// Briefings run while EVE is open, so Umbra keeps to fewer threads than candle's
/// default of one per physical core: about half the logical processors (one per core
/// on SMT CPUs) less one, so the game keeps whole cores to itself.
fn umbra_thread_count(logical_processors: usize) -> usize {
    (logical_processors / 2).saturating_sub(1).max(1)
}

/// Caps the threads candle computes on: `CANDLE_NUM_THREADS` sizes the pool behind the
/// quantized matrix products that dominate a briefing, and `RAYON_NUM_THREADS` the
/// rayon pools behind its other kernels. Both are read once, when candle first needs
/// them, so this must run before Umbra starts. A value the player set is kept.
///
/// # Safety
///
/// Changes the process environment, so it must run before any other thread starts.
pub(crate) unsafe fn limit_cpu_threads() {
    let logical_processors = thread::available_parallelism().map_or(1, |count| count.get());
    let threads = umbra_thread_count(logical_processors).to_string();
    for variable in ["CANDLE_NUM_THREADS", "RAYON_NUM_THREADS"] {
        if std::env::var_os(variable).is_none() {
            // SAFETY: the caller guarantees no other thread is running yet.
            unsafe { std::env::set_var(variable, &threads) };
        }
    }
}

/// What the briefing worker reports back to the UI.
#[derive(Debug)]
pub(crate) enum BriefingEvent {
    /// Umbra's core is being read into memory before her first briefing this session.
    Waking,
    Writing,
    Written {
        source: SystemAssessment,
        briefing: String,
    },
    Failed {
        source: SystemAssessment,
        error: String,
        /// The core itself could not be loaded, so reinstalling is the fix.
        needs_repair: bool,
    },
    /// She was pulled off the briefing before finishing it.
    Stopped,
}

/// A long-lived thread that keeps Umbra's core loaded between briefings, so only the
/// first one pays for reading and unpacking the gigabyte-sized core. Dropping the
/// worker stops any briefing in progress, ends the thread and frees the core.
pub(crate) struct BriefingWorker {
    jobs: Sender<SystemAssessment>,
    /// Raised to stop the briefing in progress; each new briefing lowers it again.
    stop: Arc<AtomicBool>,
}

impl BriefingWorker {
    pub(crate) fn spawn(on_event: impl Fn(BriefingEvent) + Send + 'static) -> Self {
        Self::spawn_with(LoadedCore::load, LoadedCore::brief, on_event)
    }

    fn spawn_with<C: 'static>(
        load: impl Fn() -> Result<C, String> + Send + 'static,
        brief: impl Fn(&mut C, &SystemAssessment, &AtomicBool) -> Result<String, String>
        + Send
        + 'static,
        on_event: impl Fn(BriefingEvent) + Send + 'static,
    ) -> Self {
        let (jobs, requests) = mpsc::channel::<SystemAssessment>();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        thread::spawn(move || {
            let mut core = None;
            while let Ok(source) = requests.recv() {
                stopped.store(false, Ordering::SeqCst);
                // The core is taken out while it works, so a panic leaves none behind
                // and the next request loads it afresh.
                let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
                    let mut loaded = match core.take() {
                        Some(loaded) => loaded,
                        None => {
                            on_event(BriefingEvent::Waking);
                            load().map_err(|error| (error, true))?
                        }
                    };
                    on_event(BriefingEvent::Writing);
                    let briefing = brief(&mut loaded, &source, &stopped);
                    core = Some(loaded);
                    briefing.map_err(|error| (error, false))
                }));
                let event = match outcome {
                    Ok(Ok(briefing)) => BriefingEvent::Written { source, briefing },
                    _ if stopped.load(Ordering::SeqCst) => BriefingEvent::Stopped,
                    Ok(Err((error, needs_repair))) => BriefingEvent::Failed {
                        source,
                        error,
                        needs_repair,
                    },
                    Err(_) => BriefingEvent::Failed {
                        source,
                        error: "Umbra lost her train of thought. Ask her again.".to_owned(),
                        needs_repair: false,
                    },
                };
                on_event(event);
            }
        });
        Self { jobs, stop }
    }

    pub(crate) fn request(&self, assessment: SystemAssessment) -> Result<(), String> {
        self.jobs
            .send(assessment)
            .map_err(|_| "Umbra stopped unexpectedly. Ask her again.".to_owned())
    }

    /// Stops the briefing in progress, keeping her core loaded for the next one. It
    /// ends with `Stopped`, or `Written` when she had just finished.
    pub(crate) fn stop_current(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for BriefingWorker {
    fn drop(&mut self) {
        self.stop_current();
    }
}

/// Umbra's core read into memory, ready to write briefings.
struct LoadedCore {
    tokenizer: Tokenizer,
    model: Qwen2,
    device: Device,
    stop_tokens: Vec<u32>,
}

impl LoadedCore {
    fn load() -> Result<Self, String> {
        if core_status() != CoreStatus::Ready {
            return Err("Install Umbra first".to_owned());
        }
        let unreadable =
            |error: String| format!("Umbra's core could not be read ({error}). Reinstall Umbra.");
        let tokenizer = Tokenizer::from_file(tokenizer_path())
            .map_err(|error| unreadable(error.to_string()))?;
        let mut file = File::open(model_path()).map_err(|error| unreadable(error.to_string()))?;
        let content =
            gguf_file::Content::read(&mut file).map_err(|error| unreadable(error.to_string()))?;
        let device = Device::Cpu;
        let model = Qwen2::from_gguf(content, &mut file, &device)
            .map_err(|error| unreadable(error.to_string()))?;
        let vocab = tokenizer.get_vocab(true);
        let stop_tokens = ["<|im_end|>", "<|endoftext|>"]
            .iter()
            .filter_map(|token| vocab.get(*token).copied())
            .collect::<Vec<_>>();
        if stop_tokens.is_empty() {
            return Err(unreadable("no end token".to_owned()));
        }
        Ok(Self {
            tokenizer,
            model,
            device,
            stop_tokens,
        })
    }

    fn brief(
        &mut self,
        assessment: &SystemAssessment,
        stop: &AtomicBool,
    ) -> Result<String, String> {
        // Each briefing starts from an empty context.
        self.model.clear_kv_cache();
        let reply_prefix = format!("Threat is {}", score_label(assessment.threat_score));
        let prompt = format!("{}{reply_prefix}", briefing_prompt(assessment));
        let tokens = self
            .tokenizer
            .encode(prompt, true)
            .map_err(|error| error.to_string())?;
        let input_ids = tokens.get_ids();
        if input_ids.is_empty() {
            return Err("Umbra received an empty fact sheet".to_owned());
        }

        let input = Tensor::new(input_ids, &self.device)
            .and_then(|tensor| tensor.unsqueeze(0))
            .map_err(|error| error.to_string())?;
        let logits = self
            .model
            .forward(&input, 0)
            .and_then(|logits| logits.squeeze(0))
            .map_err(|error| error.to_string())?;
        // Greedy decoding keeps briefings reproducible and stops a small model from
        // drifting away from the supplied facts.
        let mut processor =
            LogitsProcessor::from_sampling(assessment.generated_at, Sampling::ArgMax);
        let mut next_token = processor
            .sample(&logits)
            .map_err(|error| error.to_string())?;
        let mut generated = Vec::new();

        for index in 0..BRIEFING_MAX_TOKENS {
            if stop.load(Ordering::Relaxed) {
                return Err("Umbra was pulled off this briefing".to_owned());
            }
            if self.stop_tokens.contains(&next_token) {
                break;
            }
            generated.push(next_token);
            let input = Tensor::new(&[next_token], &self.device)
                .and_then(|tensor| tensor.unsqueeze(0))
                .map_err(|error| error.to_string())?;
            let logits = self
                .model
                .forward(&input, input_ids.len() + index)
                .and_then(|logits| logits.squeeze(0))
                .map_err(|error| error.to_string())?;
            next_token = processor
                .sample(&logits)
                .map_err(|error| error.to_string())?;
        }

        let decoded = self
            .tokenizer
            .decode(&generated, true)
            .map_err(|error| error.to_string())?;
        let briefing = clean_briefing(&format!("{reply_prefix}{decoded}"));
        if briefing == reply_prefix {
            return Err("Umbra had nothing to add to the evidence".to_owned());
        }
        validate_briefing(&briefing, assessment)?;
        Ok(briefing)
    }
}

/// True when two assessments rest on exactly the same scores and evidence, so a
/// briefing written for one is just as true of the other.
pub(crate) fn same_facts(a: &SystemAssessment, b: &SystemAssessment) -> bool {
    a.system_id == b.system_id
        && a.threat_score == b.threat_score
        && a.activity_score == b.activity_score
        && a.confidence == b.confidence
        && a.evidence == b.evidence
}

/// Puts Umbra's briefing on the current assessment, unless a refresh replaced the
/// assessment she was briefing from with different facts.
pub(crate) fn attach_briefing(
    current: &SystemAssessment,
    source: &SystemAssessment,
    briefing: String,
) -> Option<SystemAssessment> {
    same_facts(current, source).then(|| SystemAssessment {
        briefing,
        model_generated: true,
        ..current.clone()
    })
}

/// Keeps Umbra's briefing across a refresh that found exactly the same evidence.
pub(crate) fn keep_briefing(refreshed: &mut SystemAssessment, previous: &SystemAssessment) {
    if previous.model_generated && same_facts(refreshed, previous) {
        refreshed.briefing = previous.briefing.clone();
        refreshed.model_generated = true;
    }
}

fn briefing_prompt(assessment: &SystemAssessment) -> String {
    let facts = fact_sheet(assessment);
    format!(
        "<|im_start|>system\n{BRIEFING_INSTRUCTIONS}<|im_end|>\n\
         <|im_start|>user\n{facts}<|im_end|>\n\
         <|im_start|>assistant\n"
    )
}

const BRIEFING_INSTRUCTIONS: &str = "You are Umbra, a concise EVE Online intel officer. Turn the fact sheet into a two-sentence briefing for a pilot.
- Sentence one: the Threat label and the kill evidence behind it, quoting its numbers.
- Sentence two: the Activity label and the remaining evidence, quoting its numbers.
- Use only the fact sheet. Never mention pilots, corporations, alliances, factions, fleets, ships, motives or routes unless the fact sheet names them.
- Use the Threat and Activity labels exactly as written.
- Copy numbers exactly as written and never add new ones.
- Never call a system safe or free of threats; killboard data can be delayed or missing.
- At most 60 words. Plain text only.";

fn fact_sheet(assessment: &SystemAssessment) -> String {
    let mut sheet = format!(
        "Threat: {}\nActivity: {}\nEvidence:",
        score_label(assessment.threat_score),
        score_label(assessment.activity_score),
    );
    for fact in &assessment.evidence {
        sheet.push_str("\n- ");
        sheet.push_str(fact);
    }
    sheet
}

/// Keeps at most two complete sentences so a rambling or truncated generation
/// still reads as a short briefing.
fn clean_briefing(text: &str) -> String {
    let text = text.trim().trim_matches('"').trim();
    let mut end = None;
    let mut sentences = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        let at_boundary = chars.peek().is_none_or(|(_, next)| next.is_whitespace());
        if matches!(character, '.' | '!' | '?') && at_boundary {
            sentences += 1;
            end = Some(index + character.len_utf8());
            if sentences == 2 {
                break;
            }
        }
    }
    match end {
        Some(end) => text[..end].to_owned(),
        None => text.to_owned(),
    }
}

/// Rejects briefings that contradict the evidence: a small model can drop the
/// activity label or invent numbers, and an evidence summary is safer than that.
fn validate_briefing(briefing: &str, assessment: &SystemAssessment) -> Result<(), String> {
    let activity_label = score_label(assessment.activity_score).to_lowercase();
    if !briefing.to_lowercase().contains(&activity_label) {
        return Err("Umbra's briefing left out the activity level, so it was set aside".to_owned());
    }

    let facts = fact_sheet(assessment);
    let known_numbers = numbers_in(&facts);
    if let Some(number) = numbers_in(briefing)
        .into_iter()
        .find(|number| !known_numbers.contains(number))
    {
        return Err(format!(
            "Umbra's briefing quoted {number}, which is not in the evidence, so it was set aside"
        ));
    }
    Ok(())
}

fn numbers_in(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_ascii_digit() && character != '.')
        .map(|part| part.trim_matches('.'))
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn count_label(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn template_briefing(threat: u8, activity: u8, evidence: &[String]) -> String {
    let threat_label = score_label(threat);
    let activity_label = score_label(activity);
    let key_fact = evidence
        .first()
        .map(String::as_str)
        .unwrap_or("No recent combat evidence is available");
    format!("Threat is {threat_label} while activity is {activity_label}. {key_fact}.")
}

pub(crate) fn score_label(score: u8) -> &'static str {
    match score {
        0..=19 => "Quiet",
        20..=39 => "Low",
        40..=59 => "Moderate",
        60..=79 => "Elevated",
        _ => "High",
    }
}

fn download_verified(
    url: &str,
    destination: &Path,
    file: CoreFile,
    progress: &mut impl FnMut(DownloadProgress),
) -> Result<(), String> {
    if destination.exists()
        && fs::metadata(destination).is_ok_and(|metadata| metadata.len() == file.size)
    {
        // A copy already here, e.g. from an install that stopped part way, is kept
        // when it checks out.
        let checking = format!("Checking {}", file.display_name);
        if hash_file(destination, &checking, file.size, progress)
            .is_ok_and(|hash| hash == file.sha256)
        {
            return Ok(());
        }
    }

    let part_path = destination.with_extension("part");
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60 * 30))
        .build()
        .map_err(|error| error.to_string())?
        .get(url)
        .header("User-Agent", "Nomadeer/0.1 Umbra installer")
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "The download server returned {} for {}",
            response.status(),
            file.display_name
        ));
    }

    let total = response.content_length().unwrap_or(file.size);
    let mut reader = response;
    let written = write_hashed(
        &mut reader,
        &part_path,
        &format!("Downloading {}", file.display_name),
        total,
        progress,
    );
    let (downloaded, hash) = match written {
        Ok(written) => written,
        Err(error) => {
            let _ = fs::remove_file(&part_path);
            return Err(error);
        }
    };
    progress(DownloadProgress {
        label: format!("Verifying {}", file.display_name),
        done: downloaded,
        total: downloaded,
    });
    if downloaded != file.size || hash != file.sha256 {
        let _ = fs::remove_file(&part_path);
        return Err(format!(
            "{} arrived damaged. Try installing again.",
            file.display_name
        ));
    }

    replace_file(&part_path, destination).map_err(|error| error.to_string())
}

/// Streams `reader` into `path`, returning the byte count and SHA-256 of what was written.
fn write_hashed(
    reader: &mut impl Read,
    path: &Path,
    label: &str,
    total: u64,
    progress: &mut impl FnMut(DownloadProgress),
) -> Result<(u64, String), String> {
    let mut output = File::create(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut done = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| error.to_string())?;
        hasher.update(&buffer[..count]);
        done += count as u64;
        progress(DownloadProgress {
            label: label.to_owned(),
            done,
            total,
        });
    }
    output.sync_all().map_err(|error| error.to_string())?;
    Ok((done, format!("{:x}", hasher.finalize())))
}

fn verify_file(path: &Path, expected_hash: &str, expected_size: u64) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() != expected_size {
        return Err("size does not match".to_owned());
    }
    let hash = hash_file(path, "", expected_size, &mut |_| {})?;
    if hash != expected_hash {
        return Err("checksum does not match".to_owned());
    }
    Ok(())
}

/// The SHA-256 of the file at `path`, reporting progress under `label` as it reads.
fn hash_file(
    path: &Path,
    label: &str,
    total: u64,
    progress: &mut impl FnMut(DownloadProgress),
) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut done = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        done += count as u64;
        progress(DownloadProgress {
            label: label.to_owned(),
            done,
            total,
        });
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn format_isk(value: f64) -> String {
    if value >= 1_000_000_000.0 {
        format!("{:.1}b ISK", value / 1_000_000_000.0)
    } else if value >= 1_000_000.0 {
        format!("{:.1}m ISK", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1}k ISK", value / 1_000.0)
    } else {
        format!("{value:.0} ISK")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BriefingEvent, BriefingWorker, CORE_DOWNLOAD_SIZE, CORE_FILES, CoreFile, CoreStatus,
        FileStamp, LEGACY_MODEL_DIRECTORY_NAMES, LoadedCore, MODEL_DIRECTORY_NAME,
        MODEL_DOWNLOAD_SIZE, MODEL_FILE, MODEL_SHA256, MODEL_URL, TOKENIZER_FILE, TOKENIZER_SHA256,
        TOKENIZER_SIZE, TOKENIZER_URL, VERIFIED_MARKER_FILE, assess_system, assess_with_live_intel,
        attach_briefing, clean_briefing, confidence_label, core_status, core_status_in,
        download_size_label, download_verified, fact_sheet, import_file, import_path_hint_for,
        import_source_path, install_model, is_wormhole_space, keep_briefing, marker_contents,
        numbers_in, remove_cores_in, remove_directory_in, score_label, umbra_thread_count,
        validate_briefing, verify_core, verify_core_in,
    };
    use crate::live_intel::LiveContribution;
    use crate::model::{
        KillCounts, KillboardCoverage, SystemAssessment, SystemNode, SystemTrafficHistory,
        TrafficReading, TrafficSample,
    };
    use crate::util::{DAY_SECS, HOUR_SECS};
    use crate::zkill::{ZkillHistory, ZkillReport};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, SystemTime};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("nomadeer-umbra-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temp dir should be created");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn sha256(contents: &[u8]) -> String {
        format!("{:x}", Sha256::digest(contents))
    }

    fn core_file<'a>(name: &'a str, contents: &[u8], hash: &'a str) -> CoreFile<'a> {
        CoreFile {
            name,
            display_name: name,
            size: contents.len() as u64,
            sha256: hash,
        }
    }

    /// Twenty minutes into an hour, so data captured this hour is fresh.
    const NOW: u64 = 20_000 * HOUR_SECS + 1_200;
    const JITA: u64 = 30_000_142;
    const HOME_HOLE: u64 = 31_002_238;

    fn system(solar_system_id: u64, name: &str) -> SystemNode {
        SystemNode {
            id: format!("esi_sys_{solar_system_id}"),
            solar_system_id,
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
            // Values left on the map node must never be scored.
            jumps_1h: 9_999,
            ship_kills_1h: 99,
            pod_kills_1h: 99,
            npc_kills_1h: 99,
            intel_updated_at: 0,
            effect: String::new(),
            status: String::new(),
            owner: String::new(),
            notes: String::new(),
            created_at: 0,
            map_position: None,
        }
    }

    fn report(history: ZkillHistory) -> ZkillReport {
        ZkillReport {
            solar_system_id: 0,
            recent_kills: Vec::new(),
            history,
            fetched_killmails: Vec::new(),
            coverage: KillboardCoverage {
                solar_system_id: 0,
                since: 0,
                until: NOW,
            },
            interrupted: None,
            total_value: 0.0,
            solo_kills: 0,
            npc_kills: 0,
        }
    }

    /// Player kills twenty minutes ago, inside the hour a fresh ESI reading covers.
    fn kills_in_last_hour(kills: usize) -> ZkillHistory {
        ZkillHistory {
            kills_1h: kills,
            kills_4h: kills,
            kills_8h: kills,
            kills_24h: kills,
            kills_7d: kills,
            kill_times_4h: vec![NOW - 1_200; kills],
            ..ZkillHistory::default()
        }
    }

    /// Traffic captured this hour, `data_age` seconds before `NOW`.
    fn traffic(jumps: u32, kills: KillCounts, data_age: u64) -> SystemTrafficHistory {
        let hour = (NOW - data_age) / HOUR_SECS * HOUR_SECS;
        SystemTrafficHistory {
            latest_jumps: Some(TrafficReading {
                hour,
                data_time: NOW - data_age,
                value: jumps,
            }),
            latest_kills: Some(TrafficReading {
                hour,
                data_time: NOW - data_age,
                value: kills,
            }),
            samples: vec![TrafficSample {
                hour,
                jumps: Some(jumps),
                kills: Some(kills),
            }],
        }
    }

    fn player_kills(ship_kills: u32, pod_kills: u32) -> KillCounts {
        KillCounts {
            ship_kills,
            pod_kills,
            npc_kills: 0,
        }
    }

    fn test_assessment(
        threat: u8,
        activity: u8,
        confidence: &str,
        evidence: &[&str],
    ) -> SystemAssessment {
        SystemAssessment {
            system_id: "test-system".to_owned(),
            threat_score: threat,
            activity_score: activity,
            confidence: confidence.to_owned(),
            evidence: evidence.iter().map(|fact| (*fact).to_owned()).collect(),
            briefing: String::new(),
            model_generated: false,
            generated_at: 1,
        }
    }

    #[test]
    fn wormhole_space_is_told_apart_by_system_id() {
        assert!(!is_wormhole_space(&system(JITA, "Jita")));
        assert!(!is_wormhole_space(&system(30_001_198, "J-GAMP")));
        assert!(is_wormhole_space(&system(HOME_HOLE, "J123456")));
        assert!(is_wormhole_space(&system(31_000_005, "Thera")));
        assert!(is_wormhole_space(&system(31_000_792, "J135422")));
        assert!(is_wormhole_space(&system(31_000_001, "Sentinel MZ")));

        let mut manual = system(0, "J123456");
        assert!(!is_wormhole_space(&manual));
        manual.class = "C3".to_owned();
        assert!(is_wormhole_space(&manual));
        manual.class = "c16".to_owned();
        assert!(is_wormhole_space(&manual));
        manual.class = "C19".to_owned();
        assert!(!is_wormhole_space(&manual));
        manual.class = "C3".to_owned();
        manual.region_id = 10_000_002;
        assert!(!is_wormhole_space(&manual));
    }

    #[test]
    fn jita_is_scored_with_its_traffic() {
        let assessment = assess_system(
            &system(JITA, "Jita"),
            &report(ZkillHistory::default()),
            &traffic(2_000, player_kills(0, 0), 600),
            NOW,
        );
        assert!(assessment.evidence.contains(
            &"2000 jumps, 0 ship kills and 0 pod kills in the hour to 08:10 EVE time".to_owned()
        ));
        assert!(assessment.activity_score >= 80);
    }

    #[test]
    fn killboard_and_traffic_kills_are_not_added_together() {
        let fresh = traffic(60, player_kills(1, 1), 600);
        let wormhole = assess_system(
            &system(HOME_HOLE, "J123456"),
            &report(kills_in_last_hour(2)),
            &fresh,
            NOW,
        );
        let kspace = assess_system(
            &system(30_002_187, "Amamake"),
            &report(kills_in_last_hour(2)),
            &fresh,
            NOW,
        );
        assert_eq!(wormhole.threat_score, 40);
        assert!(kspace.threat_score <= wormhole.threat_score + 2);

        // Kills the killboard has not seen yet still count once.
        let unreported = assess_system(
            &system(30_002_187, "Amamake"),
            &report(ZkillHistory::default()),
            &traffic(10, player_kills(2, 1), 600),
            NOW,
        );
        assert_eq!(unreported.threat_score, 65);
    }

    #[test]
    fn esi_kills_the_killboard_places_hours_ago_count_once() {
        // One kill 80 minutes ago, inside ESI's hour that ended 50 minutes ago.
        let history = ZkillHistory {
            kills_4h: 1,
            kills_8h: 1,
            kills_24h: 1,
            kills_7d: 1,
            kill_times_4h: vec![NOW - 80 * 60],
            ..ZkillHistory::default()
        };
        let assessment = assess_system(
            &system(30_002_187, "Amamake"),
            &report(history),
            &traffic(60, player_kills(1, 0), 50 * 60),
            NOW,
        );
        assert_eq!(assessment.threat_score, 12);
        assert_eq!(
            assessment.evidence[0],
            "0 kills in the last hour, 1 in the last 4 hours and 1 in the last 24 hours"
        );
        assert!(assessment.evidence.contains(
            &"60 jumps, 1 ship kill and 0 pod kills in the hour to 07:30 EVE time".to_owned()
        ));
    }

    #[test]
    fn unlisted_esi_kills_are_scored_at_the_age_of_their_hour() {
        // ESI's hour ended 90 minutes ago, so its kills are not from the last hour.
        let assessment = assess_system(
            &system(30_002_187, "Amamake"),
            &report(ZkillHistory::default()),
            &traffic(40, player_kills(2, 0), 90 * 60),
            NOW,
        );
        assert_eq!(assessment.threat_score, 24);
        assert_eq!(
            assessment.evidence[0],
            "0 kills in the last hour, 2 in the last 4 hours and 2 in the last 24 hours"
        );
    }

    #[test]
    fn kills_only_esi_reports_lead_the_evidence() {
        let assessment = assess_system(
            &system(30_002_187, "Amamake"),
            &report(ZkillHistory::default()),
            &traffic(10, player_kills(2, 1), 600),
            NOW,
        );
        assert_eq!(
            assessment.evidence[0],
            "3 kills in the last hour, 3 in the last 4 hours and 3 in the last 24 hours"
        );
        assert_eq!(assessment.evidence[1], "3 kills not on the killboard yet");
        assert!(!assessment.briefing.contains("No kills"));
        assert!(assessment.briefing.contains("3 kills in the last hour"));
    }

    #[test]
    fn npc_losses_in_esi_kills_are_not_threat() {
        let history = ZkillHistory {
            npc_kills_24h: 3,
            npc_kills_7d: 3,
            npc_kill_times_4h: vec![NOW - 1_200; 3],
            ..ZkillHistory::default()
        };
        let assessment = assess_system(
            &system(30_002_187, "Amamake"),
            &report(history),
            &traffic(20, player_kills(3, 0), 600),
            NOW,
        );
        assert_eq!(assessment.threat_score, 0);
        assert!(assessment.activity_score > 0);
        assert_eq!(assessment.evidence[0], "No kills in the last 24 hours");
    }

    #[test]
    fn one_kill_in_a_quiet_system_is_not_a_camp() {
        let assessment = assess_system(
            &system(30_002_187, "Amamake"),
            &report(kills_in_last_hour(1)),
            &traffic(5, player_kills(1, 0), 600),
            NOW,
        );
        assert_eq!(assessment.threat_score, 22);
    }

    #[test]
    fn npc_losses_count_as_activity_not_threat() {
        let history = ZkillHistory {
            npc_kills_24h: 2,
            npc_kills_7d: 2,
            ..ZkillHistory::default()
        };
        let assessment = assess_system(
            &system(HOME_HOLE, "J123456"),
            &report(history),
            &SystemTrafficHistory::default(),
            NOW,
        );
        assert_eq!(assessment.threat_score, 0);
        assert!(assessment.activity_score > 0);
        assert!(
            assessment
                .evidence
                .contains(&"2 ships lost to NPCs in the last 24 hours".to_owned())
        );
    }

    #[test]
    fn stale_traffic_is_not_scored() {
        let assessment = assess_system(
            &system(30_002_187, "Amamake"),
            &report(ZkillHistory::default()),
            &traffic(5_000, player_kills(12, 6), 5 * HOUR_SECS),
            NOW,
        );
        assert_eq!(assessment.threat_score, 0);
        assert_eq!(assessment.activity_score, 0);
        assert_eq!(assessment.confidence, "Low");
        assert!(
            assessment
                .evidence
                .contains(&"Traffic data for this system is not available yet".to_owned())
        );
        assert!(
            !assessment
                .evidence
                .iter()
                .any(|fact| fact.contains("jumps"))
        );
    }

    /// 200 jumps this hour against 100 at this hour on each of the last 3 days.
    fn traffic_with_same_hour_baseline() -> SystemTrafficHistory {
        let mut history = traffic(200, player_kills(0, 0), 600);
        let latest_hour = history.samples[0].hour;
        let mut samples = (1..=3)
            .map(|days| TrafficSample {
                hour: latest_hour - days * DAY_SECS,
                jumps: Some(100),
                kills: Some(KillCounts::default()),
            })
            .collect::<Vec<_>>();
        samples.push(TrafficSample {
            hour: latest_hour - HOUR_SECS,
            jumps: Some(1_000),
            kills: Some(KillCounts::default()),
        });
        samples.sort_by_key(|sample| sample.hour);
        samples.extend(history.samples);
        history.samples = samples;
        history
    }

    #[test]
    fn usual_traffic_prefers_the_same_hour_of_day() {
        let assessment = assess_system(
            &system(30_002_187, "Amamake"),
            &report(ZkillHistory::default()),
            &traffic_with_same_hour_baseline(),
            NOW,
        );
        assert!(
            assessment
                .evidence
                .contains(&"Traffic is 100% above its usual level".to_owned())
        );
        assert!(assessment.evidence.contains(
            &"Usual traffic is based on 3 earlier samples at this time of day".to_owned()
        ));
        assert_eq!(assessment.confidence, "High");
    }

    #[test]
    fn two_same_hour_samples_keep_confidence_from_earlier_hours() {
        let mut history = traffic(200, player_kills(0, 0), 600);
        let latest_hour = history.samples[0].hour;
        // Fifty earlier hours, two of them at this time of day.
        let mut samples = (1..=50)
            .map(|hours| TrafficSample {
                hour: latest_hour - hours * HOUR_SECS,
                jumps: Some(100),
                kills: Some(KillCounts::default()),
            })
            .rev()
            .collect::<Vec<_>>();
        samples.extend(history.samples);
        history.samples = samples;
        let assessment = assess_system(
            &system(30_002_187, "Amamake"),
            &report(ZkillHistory::default()),
            &history,
            NOW,
        );
        assert_eq!(assessment.confidence, "Medium");

        // Three same-hour samples without fresh kills are still backed by four hours.
        let assessment = assess_system(
            &system(30_002_187, "Amamake"),
            &report(ZkillHistory::default()),
            &traffic_with_same_hour_baseline_without_kills(),
            NOW,
        );
        assert_eq!(assessment.confidence, "Medium");
    }

    fn traffic_with_same_hour_baseline_without_kills() -> SystemTrafficHistory {
        SystemTrafficHistory {
            latest_kills: None,
            ..traffic_with_same_hour_baseline()
        }
    }

    #[test]
    fn partly_read_killboard_lowers_confidence() {
        let traffic = traffic_with_same_hour_baseline();
        let history = ZkillHistory {
            partial_since: Some(NOW - 3 * DAY_SECS - 600),
            ..kills_in_last_hour(5)
        };
        let assessment = assess_system(&system(JITA, "Jita"), &report(history), &traffic, NOW);
        assert_eq!(assessment.confidence, "Medium");
        assert!(
            assessment
                .evidence
                .contains(&"At least 5 kills in the last 7 days".to_owned())
        );
        assert!(
            assessment
                .evidence
                .contains(&"Killboard history is only complete for the last 3 days".to_owned())
        );
        assert!(assessment.evidence[0].starts_with("5 kills in the last hour"));

        let history = ZkillHistory {
            partial_since: Some(NOW - 10 * HOUR_SECS),
            ..kills_in_last_hour(5)
        };
        let assessment = assess_system(&system(JITA, "Jita"), &report(history), &traffic, NOW);
        assert_eq!(assessment.confidence, "Low");
        assert!(assessment.evidence[0].starts_with("At least 5 kills in the last hour"));
        assert!(
            assessment
                .evidence
                .contains(&"Killboard history is only complete for the last 10 hours".to_owned())
        );
    }

    #[test]
    fn empty_wormhole_killboard_is_low_confidence() {
        let history = SystemTrafficHistory {
            samples: (0..48)
                .map(|hours| TrafficSample {
                    hour: NOW / HOUR_SECS * HOUR_SECS - hours * HOUR_SECS,
                    jumps: Some(0),
                    kills: Some(KillCounts::default()),
                })
                .rev()
                .collect(),
            ..SystemTrafficHistory::default()
        };
        let assessment = assess_system(
            &system(HOME_HOLE, "J123456"),
            &report(ZkillHistory::default()),
            &history,
            NOW,
        );
        assert_eq!(assessment.threat_score, 0);
        assert_eq!(assessment.confidence, "Low");
    }

    #[test]
    fn live_intel_adds_its_evidence_and_scores_and_lifts_low_confidence() {
        let live = LiveContribution {
            threat: 26.0,
            activity: 12.0,
            evidence: vec![
                "Local shows 4 pilots".to_owned(),
                "D-scan picked up 1 interdictor (1 on grid)".to_owned(),
            ],
        };
        let hole = system(HOME_HOLE, "J123456");
        let quiet = report(ZkillHistory::default());
        let traffic = SystemTrafficHistory::default();
        let without = assess_system(&hole, &quiet, &traffic, NOW);
        let with = assess_with_live_intel(&hole, &quiet, &traffic, &live, NOW);
        assert_eq!(with.threat_score, without.threat_score + 26);
        assert_eq!(with.activity_score, without.activity_score + 12);
        assert_eq!(
            &with.evidence[..without.evidence.len()],
            &without.evidence[..]
        );
        assert_eq!(&with.evidence[without.evidence.len()..], &live.evidence[..]);
        assert_eq!(without.confidence, "Low");
        assert_eq!(with.confidence, "Medium · live intel");
        assert_eq!(
            confidence_label(&with.confidence),
            "Medium confidence · live intel"
        );
        assert_eq!(confidence_label("High"), "High confidence");
        // Umbra may quote the live numbers, since they are in the evidence.
        assert!(
            validate_briefing(
                "Threat is Low with 4 pilots in local. Activity is Quiet with 1 interdictor on grid.",
                &with
            )
            .is_ok()
        );

        // A partly read killboard stays low, live intel or not.
        let partial = report(ZkillHistory {
            partial_since: Some(NOW - 6 * HOUR_SECS),
            ..ZkillHistory::default()
        });
        let with = assess_with_live_intel(&hole, &partial, &traffic, &live, NOW);
        assert_eq!(with.confidence, "Low · live intel");
    }

    #[test]
    fn clean_briefing_keeps_two_sentences() {
        assert_eq!(
            clean_briefing(" \"Threat is Low. Activity is Quiet. Extra rambling.\" "),
            "Threat is Low. Activity is Quiet."
        );
        assert_eq!(
            clean_briefing("Threat is High with 1.2b ISK lost. Activity is Low."),
            "Threat is High with 1.2b ISK lost. Activity is Low."
        );
        assert_eq!(clean_briefing("Threat is Low and"), "Threat is Low and");
    }

    #[test]
    fn fact_sheet_includes_labels_and_evidence() {
        let sheet = fact_sheet(&test_assessment(
            68,
            21,
            "Medium",
            &["1 kill in the last hour"],
        ));
        assert_eq!(
            sheet,
            "Threat: Elevated\nActivity: Low\nEvidence:\n- 1 kill in the last hour"
        );
    }

    #[test]
    fn validation_rejects_invented_numbers_and_missing_activity() {
        let assessment = test_assessment(
            68,
            21,
            "Medium",
            &["1 kill in the last hour, 2 in the last 4 hours and 3 in the last 24 hours"],
        );
        assert!(
            validate_briefing(
                "Threat is Elevated, with 1 kill in the last hour and 3 in 24 hours. Activity is Low, with Medium confidence.",
                &assessment
            )
            .is_ok()
        );
        assert!(
            validate_briefing(
                "Threat is Elevated, with 5 kills in the last hour. Activity is Low.",
                &assessment
            )
            .is_err()
        );
        assert!(validate_briefing("Threat is Elevated. Traffic is quiet.", &assessment).is_err());
    }

    #[test]
    fn numbers_in_keeps_decimals_and_drops_punctuation() {
        assert_eq!(
            numbers_in("3.4b ISK, 42% and 2140 jumps."),
            vec!["3.4", "42", "2140"]
        );
    }

    #[test]
    fn import_hint_uses_the_platform_path_style() {
        assert!(import_path_hint_for("windows").starts_with(r"C:\Users\"));
        assert!(import_path_hint_for("windows").ends_with(r"\qwen2.5-1.5b-instruct-q4_k_m.gguf"));
        assert!(import_path_hint_for("linux").starts_with("/home/"));
        assert!(import_path_hint_for("macos").starts_with("/Users/"));
    }

    #[test]
    fn import_paths_lose_surrounding_quotes_and_space() {
        let expected = PathBuf::from(r"C:\Users\pilot\Downloads\core.gguf");
        for input in [
            r"C:\Users\pilot\Downloads\core.gguf",
            r#"  "C:\Users\pilot\Downloads\core.gguf"  "#,
            r"'C:\Users\pilot\Downloads\core.gguf'",
        ] {
            assert_eq!(import_source_path(input), expected, "{input}");
        }
        // A lone or mismatched quote may be part of the name.
        assert_eq!(
            import_source_path("/home/pilot/it's.gguf"),
            PathBuf::from("/home/pilot/it's.gguf")
        );
        assert_eq!(
            import_source_path("\"/home/pilot/core.gguf'"),
            PathBuf::from("\"/home/pilot/core.gguf'")
        );
        assert_eq!(import_source_path("  "), PathBuf::new());
    }

    #[test]
    #[ignore = "requires the installed local model"]
    fn installed_model_briefing_scenarios() {
        if core_status() == CoreStatus::Unchecked {
            verify_core().expect("installed core should verify");
        }
        assert_eq!(
            core_status(),
            CoreStatus::Ready,
            "install the local model first"
        );
        let started = std::time::Instant::now();
        let mut core = LoadedCore::load().expect("installed core should load");
        println!("[{:.1}s] loaded", started.elapsed().as_secs_f32());
        let scenarios = [
            test_assessment(
                68,
                21,
                "Medium",
                &[
                    "1 kill in the last hour, 2 in the last 4 hours and 3 in the last 24 hours",
                    "3 kills in the last 7 days",
                    "Traffic is 42% below its usual level",
                ],
            ),
            test_assessment(
                4,
                0,
                "Low",
                &[
                    "No kills in the last 24 hours",
                    "No kills in the last 7 days",
                ],
            ),
            test_assessment(
                92,
                74,
                "High",
                &[
                    "4 kills in the last hour, 9 in the last 4 hours and 15 in the last 24 hours",
                    "41 kills in the last 7 days",
                    "6 kills at this time of day over the last 7 days",
                    "3.4b ISK destroyed in the last 24 hours",
                    "The most active attacker was on 7 kills in the last 24 hours",
                ],
            ),
            test_assessment(
                35,
                88,
                "High",
                &[
                    "0 kills in the last hour, 1 in the last 4 hours and 6 in the last 24 hours",
                    "52 kills in the last 7 days",
                    "2140 jumps, 0 ship kills and 0 pod kills in the last hour",
                    "Traffic is 18% above its usual level",
                    "Usual traffic is based on 5 earlier samples at this time of day",
                ],
            ),
        ];
        let mut failures = 0;
        for assessment in scenarios {
            let started = std::time::Instant::now();
            match core.brief(&assessment, &AtomicBool::new(false)) {
                Ok(briefing) => println!("[{:.1}s] {briefing}", started.elapsed().as_secs_f32()),
                Err(error) => {
                    failures += 1;
                    println!(
                        "[{:.1}s] REJECTED: {error}",
                        started.elapsed().as_secs_f32()
                    );
                }
            }
        }
        assert_eq!(failures, 0, "some briefings were rejected");
    }

    #[test]
    fn score_labels_cover_ranges() {
        assert_eq!(score_label(0), "Quiet");
        assert_eq!(score_label(39), "Low");
        assert_eq!(score_label(60), "Elevated");
        assert_eq!(score_label(100), "High");
    }

    #[test]
    #[ignore = "downloads the optional 1.1 GB model"]
    fn installed_model_generates_briefing() {
        install_model(|_| {}).expect("model should install");
        let assessment = SystemAssessment {
            system_id: "test-system".to_owned(),
            threat_score: 68,
            activity_score: 21,
            confidence: "Medium".to_owned(),
            evidence: vec![
                "1 kill in the last hour, 2 in the last 4 hours and 3 in the last 24 hours"
                    .to_owned(),
                "Traffic is 42% below its usual level".to_owned(),
            ],
            briefing: String::new(),
            model_generated: false,
            generated_at: 1,
        };
        let briefing = LoadedCore::load()
            .and_then(|mut core| core.brief(&assessment, &AtomicBool::new(false)))
            .expect("model should generate");
        println!("{briefing}");
        assert!(!briefing.trim().is_empty());
    }

    #[test]
    fn core_counts_as_ready_only_after_its_checksum_matched() {
        let dir = TempDir::new("marker");
        let core_bytes = b"umbra core weights".to_vec();
        let vocab_bytes = b"umbra vocabulary".to_vec();
        let (core_hash, vocab_hash) = (sha256(&core_bytes), sha256(&vocab_bytes));
        let files = [
            core_file("core.gguf", &core_bytes, &core_hash),
            core_file("vocab.json", &vocab_bytes, &vocab_hash),
        ];
        assert_eq!(core_status_in(&dir.path, &files), CoreStatus::Missing);

        fs::write(dir.path.join("core.gguf"), &core_bytes).unwrap();
        fs::write(dir.path.join("vocab.json"), &vocab_bytes).unwrap();
        assert_eq!(core_status_in(&dir.path, &files), CoreStatus::Unchecked);
        verify_core_in(&dir.path, &files).expect("matching files should verify");
        assert_eq!(core_status_in(&dir.path, &files), CoreStatus::Ready);

        // Same size, different bytes: the cheap check notices the change, the
        // checksum catches the damage.
        let mut damaged = core_bytes.clone();
        damaged[0] ^= 0xff;
        fs::write(dir.path.join("core.gguf"), &damaged).unwrap();
        fs::File::options()
            .write(true)
            .open(dir.path.join("core.gguf"))
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        assert_eq!(core_status_in(&dir.path, &files), CoreStatus::Unchecked);
        let error = verify_core_in(&dir.path, &files).expect_err("damaged core must fail");
        assert!(error.contains("Reinstall Umbra"), "{error}");
        assert!(!dir.path.join(VERIFIED_MARKER_FILE).exists());
        assert_eq!(core_status_in(&dir.path, &files), CoreStatus::Unchecked);
    }

    #[test]
    fn importing_the_installed_file_keeps_it() {
        let dir = TempDir::new("import");
        let contents = b"official umbra core".to_vec();
        let hash = sha256(&contents);
        let file = core_file("core.gguf", &contents, &hash);
        let source = dir.path.join("download.gguf");
        let installed = dir.path.join("core.gguf");
        fs::write(&source, &contents).unwrap();

        let mut reports = 0;
        import_file(&source, &installed, file, &mut |_| reports += 1).unwrap();
        assert_eq!(fs::read(&installed).unwrap(), contents);
        assert!(reports > 0);

        // Through a different spelling of the same path, too.
        let same = dir.path.join(".").join("core.gguf");
        import_file(&same, &installed, file, &mut |_| {}).unwrap();
        assert_eq!(fs::read(&installed).unwrap(), contents);

        // A wrong file is refused and the installed one is left alone.
        let wrong = dir.path.join("wrong.gguf");
        fs::write(&wrong, b"another file entirely").unwrap();
        assert!(import_file(&wrong, &installed, file, &mut |_| {}).is_err());
        let same_size = dir.path.join("same-size.gguf");
        fs::write(&same_size, b"OFFICIAL UMBRA CORE").unwrap();
        assert!(import_file(&same_size, &installed, file, &mut |_| {}).is_err());
        assert_eq!(fs::read(&installed).unwrap(), contents);
        assert!(!installed.with_extension("part").exists());
    }

    #[test]
    fn a_sound_file_already_in_place_is_checked_not_downloaded() {
        let dir = TempDir::new("in-place");
        let contents = b"official umbra core".to_vec();
        let hash = sha256(&contents);
        let file = core_file("core.gguf", &contents, &hash);
        let installed = dir.path.join("core.gguf");
        fs::write(&installed, &contents).unwrap();

        // Nothing listens here, so any download attempt would fail.
        let mut labels = Vec::new();
        download_verified("http://127.0.0.1:9/core.gguf", &installed, file, &mut |p| {
            labels.push(p.label);
        })
        .expect("the sound copy should be kept");
        assert_eq!(fs::read(&installed).unwrap(), contents);
        assert!(!labels.is_empty());
        assert!(labels.iter().all(|label| label == "Checking core.gguf"));
    }

    #[test]
    fn removing_umbra_clears_old_cores_and_nothing_else() {
        let dir = TempDir::new("remove");
        let root = dir.path.join("models");
        let current = root.join(MODEL_DIRECTORY_NAME);
        let legacy = root.join(LEGACY_MODEL_DIRECTORY_NAMES[0]);
        let unrelated = root.join("someone-elses-model");
        for directory in [&current, &legacy, &unrelated] {
            fs::create_dir_all(directory).unwrap();
            fs::write(directory.join("weights.gguf"), b"weights").unwrap();
        }
        fs::write(dir.path.join("outside.txt"), b"keep").unwrap();

        remove_cores_in(&root).unwrap();
        assert!(!current.exists());
        assert!(!legacy.exists());
        assert!(unrelated.join("weights.gguf").exists());
        assert!(dir.path.join("outside.txt").exists());
        // Removing again finds nothing to do.
        remove_cores_in(&root).unwrap();

        for name in ["..", "../models", "a/b", "/tmp", ""] {
            assert!(remove_directory_in(&root, name).is_err(), "{name}");
        }
        assert!(unrelated.exists());
        assert!(Path::new(&dir.path).join("outside.txt").exists());
    }

    fn next_event(events: &mpsc::Receiver<BriefingEvent>) -> BriefingEvent {
        events
            .recv_timeout(Duration::from_secs(5))
            .expect("worker should answer")
    }

    /// Skips the progress events and returns how the briefing ended.
    fn outcome(events: &mpsc::Receiver<BriefingEvent>) -> BriefingEvent {
        loop {
            match next_event(events) {
                BriefingEvent::Waking | BriefingEvent::Writing => {}
                event => return event,
            }
        }
    }

    #[test]
    fn briefing_worker_keeps_the_core_loaded_between_briefings() {
        let loads = Arc::new(AtomicUsize::new(0));
        let load_count = Arc::clone(&loads);
        let (tx, events) = mpsc::channel();
        let worker = BriefingWorker::spawn_with(
            move || {
                // The first load fails, as a damaged core would.
                match load_count.fetch_add(1, Ordering::SeqCst) {
                    0 => Err("unreadable".to_owned()),
                    _ => Ok(0_usize),
                }
            },
            |briefings: &mut usize, assessment: &SystemAssessment, _: &AtomicBool| {
                *briefings += 1;
                if assessment.threat_score == 99 {
                    panic!("model fell over");
                }
                Ok(format!(
                    "briefing {briefings} for {}",
                    assessment.threat_score
                ))
            },
            move |event| {
                let _ = tx.send(event);
            },
        );

        worker.request(test_assessment(10, 0, "Low", &[])).unwrap();
        assert!(matches!(next_event(&events), BriefingEvent::Waking));
        assert!(matches!(
            next_event(&events),
            BriefingEvent::Failed {
                needs_repair: true,
                ..
            }
        ));

        for threat in [20, 30] {
            worker
                .request(test_assessment(threat, 0, "Low", &[]))
                .unwrap();
        }
        let BriefingEvent::Written { briefing, source } = outcome(&events) else {
            panic!("first briefing should be written");
        };
        assert_eq!(
            (briefing.as_str(), source.threat_score),
            ("briefing 1 for 20", 20)
        );
        let BriefingEvent::Written { briefing, .. } = outcome(&events) else {
            panic!("second briefing should be written");
        };
        assert_eq!(briefing, "briefing 2 for 30");
        assert_eq!(loads.load(Ordering::SeqCst), 2);

        // A panic is reported, and the next briefing starts from a fresh core.
        worker.request(test_assessment(99, 0, "Low", &[])).unwrap();
        assert!(matches!(
            outcome(&events),
            BriefingEvent::Failed {
                needs_repair: false,
                ..
            }
        ));
        worker.request(test_assessment(40, 0, "Low", &[])).unwrap();
        let BriefingEvent::Written { briefing, .. } = outcome(&events) else {
            panic!("briefing after a panic should be written");
        };
        assert_eq!(briefing, "briefing 1 for 40");
        assert_eq!(loads.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn briefing_worker_can_be_pulled_off_a_briefing() {
        let (tx, events) = mpsc::channel();
        let worker = BriefingWorker::spawn_with(
            || Ok(()),
            |_: &mut (), assessment: &SystemAssessment, stop: &AtomicBool| {
                // Threat 1 runs until she is told to stop.
                if assessment.threat_score == 1 {
                    while !stop.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(1));
                    }
                    return Err("stopped".to_owned());
                }
                Ok(format!("briefing for {}", assessment.threat_score))
            },
            move |event| {
                let _ = tx.send(event);
            },
        );

        worker.request(test_assessment(1, 0, "Low", &[])).unwrap();
        assert!(matches!(next_event(&events), BriefingEvent::Waking));
        assert!(matches!(next_event(&events), BriefingEvent::Writing));
        worker.stop_current();
        worker.request(test_assessment(2, 0, "Low", &[])).unwrap();
        assert!(matches!(outcome(&events), BriefingEvent::Stopped));
        // The next briefing runs to the end on the core she already loaded.
        assert!(matches!(next_event(&events), BriefingEvent::Writing));
        let BriefingEvent::Written { briefing, .. } = outcome(&events) else {
            panic!("the next briefing should be written");
        };
        assert_eq!(briefing, "briefing for 2");
    }

    #[test]
    fn umbra_leaves_cores_free_for_the_game() {
        assert_eq!(umbra_thread_count(32), 15);
        assert_eq!(umbra_thread_count(16), 7);
        assert_eq!(umbra_thread_count(12), 5);
        assert_eq!(umbra_thread_count(8), 3);
        assert_eq!(umbra_thread_count(6), 2);
        assert_eq!(umbra_thread_count(4), 1);
        assert_eq!(umbra_thread_count(2), 1);
        assert_eq!(umbra_thread_count(1), 1);
        for logical_processors in 3..=64 {
            assert!(umbra_thread_count(logical_processors) <= logical_processors - 2);
        }
    }

    #[test]
    fn briefing_only_attaches_to_the_assessment_it_was_written_from() {
        let source = test_assessment(68, 21, "Medium", &["1 kill in the last hour"]);
        let mut same = source.clone();
        same.generated_at = 500;
        let briefed = attach_briefing(&same, &source, "Threat is Elevated.".to_owned())
            .expect("identical facts should take the briefing");
        assert!(briefed.model_generated);
        assert_eq!(briefed.briefing, "Threat is Elevated.");
        assert_eq!(briefed.generated_at, 500);

        let newer = test_assessment(80, 21, "Medium", &["3 kills in the last hour"]);
        assert!(attach_briefing(&newer, &source, "Threat is Elevated.".to_owned()).is_none());

        let mut refreshed = same.clone();
        keep_briefing(&mut refreshed, &briefed);
        assert!(refreshed.model_generated);
        assert_eq!(refreshed.briefing, "Threat is Elevated.");
        let mut changed = newer.clone();
        keep_briefing(&mut changed, &briefed);
        assert!(!changed.model_generated);
    }

    /// The ISPP defines tools/release.py generates for the Windows installer.
    fn windows_setup_defines() -> std::collections::HashMap<String, String> {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/release.py");
        let mut failures = Vec::new();
        for python in ["python3", "python"] {
            match std::process::Command::new(python)
                .arg(&script)
                .arg("inno-defines")
                .output()
            {
                Ok(output) if output.status.success() => {
                    return String::from_utf8(output.stdout)
                        .expect("defines should be UTF-8")
                        .lines()
                        .filter_map(|line| line.strip_prefix("#define "))
                        .map(|define| {
                            let (name, value) = define.split_once(' ').expect("name and value");
                            (name.to_owned(), value.trim_matches('"').to_owned())
                        })
                        .collect();
                }
                Ok(output) => failures.push(format!(
                    "{python}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Err(error) => failures.push(format!("{python}: {error}")),
            }
        }
        panic!(
            "tools/release.py inno-defines needs Python 3: {}",
            failures.join("; ")
        );
    }

    #[test]
    fn windows_setup_installs_umbra_where_nomadeer_looks() {
        let defines = windows_setup_defines();
        let define = |name: &str| {
            defines
                .get(name)
                .unwrap_or_else(|| panic!("{name} is not generated"))
                .as_str()
        };
        let expected = [
            ("UmbraDataDir", crate::paths::APP_DIR_NAME.to_owned()),
            ("UmbraModelsDir", crate::paths::MODELS_DIR_NAME.to_owned()),
            ("UmbraCoreDir", MODEL_DIRECTORY_NAME.to_owned()),
            ("UmbraMarkerFile", VERIFIED_MARKER_FILE.to_owned()),
            ("UmbraModelFile", MODEL_FILE.to_owned()),
            ("UmbraModelUrl", MODEL_URL.to_owned()),
            ("UmbraModelSha256", MODEL_SHA256.to_owned()),
            ("UmbraModelSize", MODEL_DOWNLOAD_SIZE.to_string()),
            ("UmbraTokenizerFile", TOKENIZER_FILE.to_owned()),
            ("UmbraTokenizerUrl", TOKENIZER_URL.to_owned()),
            ("UmbraTokenizerSha256", TOKENIZER_SHA256.to_owned()),
            ("UmbraTokenizerSize", TOKENIZER_SIZE.to_string()),
            ("UmbraDownloadSize", download_size_label()),
        ];
        for (name, value) in &expected {
            assert_eq!(define(name), value, "{name}");
        }
        assert_eq!(defines.len(), expected.len(), "{defines:?}");

        // Setup writes the verified marker itself, a line per file in this order;
        // it must read exactly as Nomadeer's own.
        let stamps = [
            FileStamp {
                len: MODEL_DOWNLOAD_SIZE,
                modified_nanos: 1_760_000_000_123_456_700,
            },
            FileStamp {
                len: TOKENIZER_SIZE,
                modified_nanos: 1_760_000_001_000_000_000,
            },
        ];
        let setup_marker = format!(
            "{} {} {} {}\n{} {} {} {}\n",
            define("UmbraModelFile"),
            define("UmbraModelSha256"),
            define("UmbraModelSize"),
            stamps[0].modified_nanos,
            define("UmbraTokenizerFile"),
            define("UmbraTokenizerSha256"),
            define("UmbraTokenizerSize"),
            stamps[1].modified_nanos,
        );
        assert_eq!(setup_marker, marker_contents(&CORE_FILES, &stamps));
        assert_eq!(
            Path::new(define("UmbraModelsDir"))
                .join(define("UmbraCoreDir"))
                .join(define("UmbraModelFile")),
            Path::new(crate::paths::MODELS_DIR_NAME)
                .join(MODEL_DIRECTORY_NAME)
                .join(MODEL_FILE)
        );
    }

    #[test]
    fn the_download_size_reads_in_gigabytes() {
        assert_eq!(CORE_DOWNLOAD_SIZE, 1_124_352_381);
        assert_eq!(download_size_label(), "1.1 GB");
    }
}
