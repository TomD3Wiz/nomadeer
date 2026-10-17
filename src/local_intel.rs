use crate::model::{SystemActivitySnapshot, SystemAssessment, SystemNode};
use crate::util::now;
use crate::zkill::ZkillReport;
use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::quantized_qwen2::ModelWeights as Qwen2;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tokenizers::Tokenizer;

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
const MODEL_DIRECTORY: &str = "data/models/qwen2.5-1.5b-instruct-q4_k_m";
const BRIEFING_MAX_TOKENS: usize = 120;

#[derive(Debug, Clone)]
pub(crate) struct DownloadProgress {
    pub(crate) file: String,
    pub(crate) downloaded: u64,
    pub(crate) total: u64,
}

pub(crate) fn model_directory() -> PathBuf {
    PathBuf::from(MODEL_DIRECTORY)
}

pub(crate) fn model_path() -> PathBuf {
    model_directory().join(MODEL_FILE)
}

pub(crate) fn tokenizer_path() -> PathBuf {
    model_directory().join(TOKENIZER_FILE)
}

pub(crate) fn is_model_installed() -> bool {
    file_has_size(&model_path(), MODEL_DOWNLOAD_SIZE)
        && file_has_size(&tokenizer_path(), TOKENIZER_SIZE)
}

pub(crate) fn install_model(mut progress: impl FnMut(DownloadProgress)) -> Result<(), String> {
    fs::create_dir_all(model_directory()).map_err(|error| error.to_string())?;
    download_verified(
        MODEL_URL,
        &model_path(),
        MODEL_SHA256,
        MODEL_DOWNLOAD_SIZE,
        MODEL_FILE,
        &mut progress,
    )?;
    download_verified(
        TOKENIZER_URL,
        &tokenizer_path(),
        TOKENIZER_SHA256,
        TOKENIZER_SIZE,
        TOKENIZER_FILE,
        &mut progress,
    )
}

pub(crate) fn import_model(source: &Path) -> Result<(), String> {
    if !source.is_file() {
        return Err("Choose the downloaded Qwen2.5 GGUF model file".to_owned());
    }
    verify_file(source, MODEL_SHA256, MODEL_DOWNLOAD_SIZE)?;
    fs::create_dir_all(model_directory()).map_err(|error| error.to_string())?;
    fs::copy(source, model_path()).map_err(|error| error.to_string())?;

    if !file_has_size(&tokenizer_path(), TOKENIZER_SIZE) {
        download_verified(
            TOKENIZER_URL,
            &tokenizer_path(),
            TOKENIZER_SHA256,
            TOKENIZER_SIZE,
            TOKENIZER_FILE,
            &mut |_| {},
        )?;
    }
    Ok(())
}

pub(crate) fn remove_model() -> Result<(), String> {
    let directory = model_directory();
    if directory.exists() {
        fs::remove_dir_all(directory).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) fn assess_system(
    system: &SystemNode,
    report: &ZkillReport,
    snapshots: &[SystemActivitySnapshot],
) -> SystemAssessment {
    let history = &report.history;
    let is_wormhole = system.name.starts_with('J');

    let kills_1_to_4 = history.kills_4h.saturating_sub(history.kills_1h);
    let kills_4_to_8 = history.kills_8h.saturating_sub(history.kills_4h);
    let kills_8_to_24 = history.kills_24h.saturating_sub(history.kills_8h);
    let mut threat = history.kills_1h as f32 * 20.0
        + kills_1_to_4 as f32 * 12.0
        + kills_4_to_8 as f32 * 7.0
        + kills_8_to_24 as f32 * 3.0
        + history.solo_24h as f32 * 4.0;
    threat += history.busiest_attacker_24h.saturating_sub(1) as f32 * 5.0;
    threat += history.busiest_corporation_24h.saturating_sub(2) as f32 * 2.0;
    threat += (history.isk_destroyed_24h / 500_000_000.0).min(12.0) as f32;
    threat += (history.kills_current_hour_7d as f32 * 1.5).min(8.0);

    if !is_wormhole {
        let player_kills = system.ship_kills_1h + system.pod_kills_1h;
        threat += (player_kills as f32 * 6.0).min(24.0);
        if system.jumps_1h > 0 {
            let kills_per_thousand = player_kills as f32 * 1_000.0 / system.jumps_1h as f32;
            threat += (kills_per_thousand * 2.5).min(18.0);
        }
    }

    let mut activity = if is_wormhole {
        (history.kills_24h as f32 * 12.0 + history.kills_7d as f32 * 1.5).min(100.0)
    } else {
        let jump_activity = if system.jumps_1h == 0 {
            0.0
        } else {
            (system.jumps_1h as f32).log10() * 28.0
        };
        let combat_activity = (system.ship_kills_1h + system.pod_kills_1h) as f32 * 5.0;
        let npc_activity = (system.npc_kills_1h as f32).sqrt().min(20.0);
        (jump_activity + combat_activity + npc_activity).min(100.0)
    };

    let system_snapshots = snapshots
        .iter()
        .filter(|snapshot| snapshot.system_id == system.id)
        .collect::<Vec<_>>();
    let assessment_time = now();
    let current_hour_start = assessment_time / 3_600 * 3_600;
    let current_utc_hour = current_hour_start / 3_600 % 24;
    let historical_snapshots = system_snapshots
        .iter()
        .copied()
        .filter(|snapshot| snapshot.captured_at < current_hour_start)
        .collect::<Vec<_>>();
    let same_hour_snapshots = historical_snapshots
        .iter()
        .copied()
        .filter(|snapshot| snapshot.captured_at / 3_600 % 24 == current_utc_hour)
        .collect::<Vec<_>>();
    let baseline_source = if same_hour_snapshots.len() >= 2 {
        &same_hour_snapshots
    } else {
        &historical_snapshots
    };
    let baseline_jumps = if baseline_source.is_empty() {
        None
    } else {
        Some(
            baseline_source
                .iter()
                .map(|snapshot| snapshot.jumps_1h as f32)
                .sum::<f32>()
                / baseline_source.len() as f32,
        )
    };

    if !is_wormhole
        && let Some(baseline) = baseline_jumps
        && baseline >= 1.0
    {
        let relative_traffic = system.jumps_1h as f32 / baseline;
        let baseline_adjustment = ((relative_traffic - 1.0) * 20.0).clamp(-15.0, 20.0);
        activity = (activity + baseline_adjustment).clamp(0.0, 100.0);
    }

    let threat_score = threat.clamp(0.0, 100.0).round() as u8;
    let activity_score = activity.clamp(0.0, 100.0).round() as u8;
    let confidence = if system_snapshots.len() >= 24 {
        "High"
    } else if system_snapshots.len() >= 4 || history.kills_7d > 0 {
        "Medium"
    } else {
        "Low"
    }
    .to_owned();

    let mut evidence = Vec::new();
    evidence.push(if history.kills_24h == 0 {
        "No kills in the last 24 hours".to_owned()
    } else {
        format!(
            "{} in the last hour, {} in the last 4 hours and {} in the last 24 hours",
            count_label(history.kills_1h, "kill"),
            history.kills_4h,
            history.kills_24h
        )
    });
    evidence.push(if history.kills_7d == 0 {
        "No kills in the last 7 days".to_owned()
    } else {
        format!(
            "{} in the last 7 days",
            count_label(history.kills_7d, "kill")
        )
    });
    if history.kills_current_hour_7d > 0 {
        evidence.push(format!(
            "{} at this time of day over the last 7 days",
            count_label(history.kills_current_hour_7d, "kill")
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
        evidence.push(format!(
            "{}, {} and {} in the last hour",
            count_label(system.jumps_1h as usize, "jump"),
            count_label(system.ship_kills_1h as usize, "ship kill"),
            count_label(system.pod_kills_1h as usize, "pod kill")
        ));
        if let Some(baseline) = baseline_jumps
            && baseline >= 1.0
        {
            let difference = (system.jumps_1h as f32 - baseline) / baseline * 100.0;
            evidence.push(format!(
                "Traffic is {:.0}% {} its usual level",
                difference.abs(),
                if difference >= 0.0 { "above" } else { "below" }
            ));
            if same_hour_snapshots.len() >= 2 {
                evidence.push(format!(
                    "Usual traffic is based on {} earlier samples at this time of day",
                    same_hour_snapshots.len()
                ));
            }
        }
    }

    let briefing = template_briefing(threat_score, activity_score, &evidence);
    SystemAssessment {
        system_id: system.id.clone(),
        threat_score,
        activity_score,
        confidence,
        evidence,
        briefing,
        model_generated: false,
        generated_at: now(),
    }
}

pub(crate) fn generate_briefing(assessment: &SystemAssessment) -> Result<String, String> {
    if !is_model_installed() {
        return Err("Install the Local Intel Model first".to_owned());
    }

    let tokenizer = Tokenizer::from_file(tokenizer_path()).map_err(|error| error.to_string())?;
    let mut file = File::open(model_path()).map_err(|error| error.to_string())?;
    let content = gguf_file::Content::read(&mut file).map_err(|error| error.to_string())?;
    let device = Device::Cpu;
    let mut model =
        Qwen2::from_gguf(content, &mut file, &device).map_err(|error| error.to_string())?;

    let reply_prefix = format!("Threat is {}", score_label(assessment.threat_score));
    let prompt = format!("{}{reply_prefix}", briefing_prompt(assessment));
    let tokens = tokenizer
        .encode(prompt, true)
        .map_err(|error| error.to_string())?;
    let input_ids = tokens.get_ids();
    if input_ids.is_empty() {
        return Err("The local model received an empty prompt".to_owned());
    }

    let input = Tensor::new(input_ids, &device)
        .and_then(|tensor| tensor.unsqueeze(0))
        .map_err(|error| error.to_string())?;
    let logits = model
        .forward(&input, 0)
        .and_then(|logits| logits.squeeze(0))
        .map_err(|error| error.to_string())?;
    // Greedy decoding keeps briefings reproducible and stops a small model from
    // drifting away from the supplied facts.
    let mut processor = LogitsProcessor::from_sampling(assessment.generated_at, Sampling::ArgMax);
    let mut next_token = processor
        .sample(&logits)
        .map_err(|error| error.to_string())?;
    let vocab = tokenizer.get_vocab(true);
    let stop_tokens = ["<|im_end|>", "<|endoftext|>"]
        .iter()
        .filter_map(|token| vocab.get(*token).copied())
        .collect::<Vec<_>>();
    if stop_tokens.is_empty() {
        return Err("Local model tokenizer has no end token".to_owned());
    }
    let mut generated = Vec::new();

    for index in 0..BRIEFING_MAX_TOKENS {
        if stop_tokens.contains(&next_token) {
            break;
        }
        generated.push(next_token);
        let input = Tensor::new(&[next_token], &device)
            .and_then(|tensor| tensor.unsqueeze(0))
            .map_err(|error| error.to_string())?;
        let logits = model
            .forward(&input, input_ids.len() + index)
            .and_then(|logits| logits.squeeze(0))
            .map_err(|error| error.to_string())?;
        next_token = processor
            .sample(&logits)
            .map_err(|error| error.to_string())?;
    }

    let decoded = tokenizer
        .decode(&generated, true)
        .map_err(|error| error.to_string())?;
    let briefing = clean_briefing(&format!("{reply_prefix}{decoded}"));
    if briefing == reply_prefix {
        return Err("Local model returned an empty briefing".to_owned());
    }
    validate_briefing(&briefing, assessment)?;
    Ok(briefing)
}

fn briefing_prompt(assessment: &SystemAssessment) -> String {
    let facts = fact_sheet(assessment);
    format!(
        "<|im_start|>system\n{BRIEFING_INSTRUCTIONS}<|im_end|>\n\
         <|im_start|>user\n{facts}<|im_end|>\n\
         <|im_start|>assistant\n"
    )
}

const BRIEFING_INSTRUCTIONS: &str = "You are a concise EVE Online intel officer. Turn the fact sheet into a two-sentence briefing for a pilot.
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
        return Err("Local model briefing left out the activity level".to_owned());
    }

    let facts = fact_sheet(assessment);
    let known_numbers = numbers_in(&facts);
    if let Some(number) = numbers_in(briefing)
        .into_iter()
        .find(|number| !known_numbers.contains(number))
    {
        return Err(format!(
            "Local model briefing used {number}, which is not in the evidence"
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

fn count_label(count: usize, noun: &str) -> String {
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
    expected_hash: &str,
    expected_size: u64,
    display_name: &str,
    progress: &mut impl FnMut(DownloadProgress),
) -> Result<(), String> {
    if destination.exists() && verify_file(destination, expected_hash, expected_size).is_ok() {
        return Ok(());
    }

    let part_path = destination.with_extension("part");
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60 * 30))
        .build()
        .map_err(|error| error.to_string())?
        .get(url)
        .header("User-Agent", "Nomadeer/0.1 local intel model installer")
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Model host returned {} while downloading {display_name}",
            response.status()
        ));
    }

    let total = response.content_length().unwrap_or(expected_size);
    let mut reader = response;
    let mut output = File::create(&part_path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0_u64;
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
        downloaded += count as u64;
        progress(DownloadProgress {
            file: display_name.to_owned(),
            downloaded,
            total,
        });
    }
    output.sync_all().map_err(|error| error.to_string())?;

    if downloaded != expected_size {
        let _ = fs::remove_file(&part_path);
        return Err(format!(
            "Downloaded {display_name} has the wrong size ({downloaded} bytes)"
        ));
    }
    let hash = format!("{:x}", hasher.finalize());
    if hash != expected_hash {
        let _ = fs::remove_file(&part_path);
        return Err(format!("Downloaded {display_name} failed verification"));
    }

    fs::rename(part_path, destination).map_err(|error| error.to_string())
}

fn verify_file(path: &Path, expected_hash: &str, expected_size: u64) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() != expected_size {
        return Err("Model file size does not match".to_owned());
    }
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let hash = format!("{:x}", hasher.finalize());
    if hash != expected_hash {
        return Err("Model checksum does not match".to_owned());
    }
    Ok(())
}

fn file_has_size(path: &Path, expected_size: u64) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.len() == expected_size)
        .unwrap_or(false)
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
        clean_briefing, fact_sheet, generate_briefing, install_model, is_model_installed,
        numbers_in, score_label, validate_briefing,
    };
    use crate::model::SystemAssessment;

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
    #[ignore = "requires the installed local model"]
    fn installed_model_briefing_scenarios() {
        assert!(is_model_installed(), "install the local model first");
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
            match generate_briefing(&assessment) {
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
        let briefing = generate_briefing(&assessment).expect("model should generate");
        println!("{briefing}");
        assert!(!briefing.trim().is_empty());
    }
}
