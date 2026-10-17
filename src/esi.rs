use crate::model::{
    Account, CharacterAffiliation, EsiSettings, KillCounts, TrafficFeed, TrafficHour,
    TrafficReading, TrafficRows,
};
use crate::util::{HOUR_SECS, hour_start, now, open_url, parse_http_date};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::blocking::{Client, Response as HttpResponse};
use reqwest::header::{DATE, EXPIRES, HeaderName, LAST_MODIFIED};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tiny_http::{Response, Server, StatusCode};
use url::Url;

const AUTHORIZE_URL: &str = "https://login.eveonline.com/v2/oauth/authorize/";
const TOKEN_URL: &str = "https://login.eveonline.com/v2/oauth/token";
const VERIFY_URL: &str = "https://login.eveonline.com/oauth/verify";
const ESI_BASE_URL: &str = "https://esi.evetech.net/latest";
const USER_AGENT: &str = "Nomadeer/0.1 private wormhole mapper";
const SYSTEM_JUMPS_PATH: &str = "/universe/system_jumps/";
const SYSTEM_KILLS_PATH: &str = "/universe/system_kills/";
const TRAFFIC_MIN_POLL: Duration = Duration::from_secs(60);
const TRAFFIC_MAX_RETRY: Duration = Duration::from_secs(15 * 60);
const TRAFFIC_EXPIRES_SLACK_SECS: u64 = 15;
/// Lets Nomadeer see whether a character is logged in.
pub(crate) const ONLINE_SCOPE: &str = "esi-location.read_online.v1";
/// ESI caches a character's online status for about a minute.
const ONLINE_CACHE_SECS: u64 = 60;
/// Longest Nomadeer trusts an online status, whatever ESI's headers say.
const MAX_ONLINE_CACHE_SECS: u64 = 5 * 60;

type FeedCacheLock<T> = OnceLock<Mutex<Option<FeedCache<T>>>>;

// Static system details never change, so they are cached for the whole session.
static SYSTEM_DETAILS_CACHE: OnceLock<Mutex<HashMap<u64, SystemIntel>>> = OnceLock::new();
static SYSTEM_JUMPS_FEED: FeedCacheLock<SystemJumpResponse> = OnceLock::new();
static SYSTEM_KILLS_FEED: FeedCacheLock<SystemKillResponse> = OnceLock::new();

/// A universe-wide activity feed as ESI served it, valid until `expires_at`.
#[derive(Debug)]
struct FeedCache<T> {
    hour: u64,
    data_time: u64,
    expires_at: u64,
    entries: Arc<Vec<T>>,
}

// Manual impl so cloning shares the entries without requiring `T: Clone`.
impl<T> Clone for FeedCache<T> {
    fn clone(&self) -> Self {
        Self {
            hour: self.hour,
            data_time: self.data_time,
            expires_at: self.expires_at,
            entries: Arc::clone(&self.entries),
        }
    }
}

impl<T> FeedCache<T> {
    fn reading<V>(&self, value: V) -> TrafficReading<V> {
        TrafficReading {
            hour: self.hour,
            data_time: self.data_time,
            value,
        }
    }
}

/// One hour of a universe-wide feed plus when ESI will publish the next one.
#[derive(Debug)]
pub(crate) struct TrafficCapture {
    pub(crate) traffic: TrafficHour,
    pub(crate) expires_at: u64,
}

#[derive(Debug)]
pub(crate) struct AccountAuthorization {
    pub(crate) account_id: String,
    pub(crate) character_id: String,
    pub(crate) character_name: String,
    pub(crate) owner_hash: String,
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) token_expires_at: u64,
}

#[derive(Debug)]
pub(crate) struct LocationSync {
    pub(crate) account_id: String,
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) token_expires_at: u64,
    pub(crate) system: SystemIntel,
}

/// Whether a character is logged in, valid for `cache_lifetime`.
#[derive(Debug)]
pub(crate) struct OnlineSync {
    pub(crate) account_id: String,
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) token_expires_at: u64,
    pub(crate) online: bool,
    pub(crate) cache_lifetime: Duration,
}

/// A character's access and refresh tokens, refreshed when about to expire.
struct Tokens {
    access_token: String,
    refresh_token: String,
    token_expires_at: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SystemIntel {
    pub(crate) solar_system_id: u64,
    pub(crate) name: String,
    pub(crate) security_class: String,
    pub(crate) security_status: f32,
    pub(crate) constellation_id: u64,
    pub(crate) constellation_name: String,
    pub(crate) region_id: u64,
    pub(crate) region_name: String,
    pub(crate) planet_count: u32,
    pub(crate) stargate_count: u32,
    pub(crate) station_count: u32,
    /// `None` when ESI's activity feed could not be read; keep the previous values then.
    pub(crate) jumps: Option<TrafficReading<u32>>,
    pub(crate) kills: Option<TrafficReading<KillCounts>>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct VerifyResponse {
    #[serde(rename = "CharacterID")]
    character_id: u64,
    #[serde(rename = "CharacterName")]
    character_name: String,
    #[serde(rename = "CharacterOwnerHash")]
    character_owner_hash: String,
}

/// ESI also sends `last_login`, `last_logout` and `logins`; only `online` is used.
#[derive(Debug, Deserialize)]
struct OnlineResponse {
    online: bool,
}

#[derive(Debug, Deserialize)]
struct LocationResponse {
    solar_system_id: u64,
}

#[derive(Debug, Deserialize)]
struct SystemResponse {
    name: String,
    constellation_id: u64,
    security_class: Option<String>,
    security_status: Option<f32>,
    planets: Option<Vec<SystemPlanetResponse>>,
    stargates: Option<Vec<u64>>,
    stations: Option<Vec<u64>>,
}

#[derive(Debug, Deserialize)]
struct SystemPlanetResponse {
    #[allow(dead_code)]
    planet_id: u64,
}

#[derive(Debug, Deserialize)]
struct ConstellationResponse {
    name: String,
    region_id: u64,
}

#[derive(Debug, Deserialize)]
struct RegionResponse {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SystemJumpResponse {
    system_id: u64,
    ship_jumps: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct SystemKillResponse {
    system_id: u64,
    ship_kills: u32,
    pod_kills: u32,
    npc_kills: u32,
}

pub(crate) fn authorize_account(
    settings: EsiSettings,
    account_id: String,
) -> Result<AccountAuthorization, String> {
    validate_settings(&settings)?;

    let redirect_uri = redirect_uri(settings.callback_port);
    let state = random_urlsafe(32)?;
    let verifier = random_urlsafe(64)?;
    let challenge = pkce_challenge(&verifier);
    let auth_url = authorization_url(&settings, &redirect_uri, &state, &challenge)?;

    let server = Server::http(("127.0.0.1", settings.callback_port))
        .map_err(|error| format!("Could not start local OAuth callback server: {error}"))?;

    open_url(auth_url.as_str())?;

    let code = wait_for_code(&server, &state)?;
    let token = exchange_code(&settings, &redirect_uri, &code, &verifier)?;
    let verify = verify_token(&token.access_token)?;
    let refresh_token = token
        .refresh_token
        .ok_or_else(|| "ESI did not return a refresh token".to_owned())?;

    Ok(AccountAuthorization {
        account_id,
        character_id: verify.character_id.to_string(),
        character_name: verify.character_name,
        owner_hash: verify.character_owner_hash,
        access_token: token.access_token,
        refresh_token,
        token_expires_at: now() + token.expires_in,
    })
}

pub(crate) fn sync_location(
    settings: EsiSettings,
    account: Account,
) -> Result<LocationSync, String> {
    validate_settings(&settings)?;
    let tokens = fresh_tokens(&settings, &account)?;
    let character_id = character_id(&account)?;
    let location = fetch_location(character_id, &tokens.access_token)?;
    let system = fetch_system_intel(location.solar_system_id)?;

    Ok(LocationSync {
        account_id: account.id,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        token_expires_at: tokens.token_expires_at,
        system,
    })
}

/// Asks ESI whether the character is logged in. Needs [`ONLINE_SCOPE`].
pub(crate) fn sync_online(settings: EsiSettings, account: Account) -> Result<OnlineSync, String> {
    validate_settings(&settings)?;
    let tokens = fresh_tokens(&settings, &account)?;
    let character_id = character_id(&account)?;
    let url = format!("{ESI_BASE_URL}/characters/{character_id}/online/?datasource=tranquility");
    let response = get_response(&url, Some(&tokens.access_token))?;
    let cache_lifetime = online_cache_lifetime(
        header_time(&response, DATE),
        header_time(&response, EXPIRES),
    );
    let online = response
        .json::<OnlineResponse>()
        .map_err(|error| format!("Could not parse ESI response: {error}"))?
        .online;

    Ok(OnlineSync {
        account_id: account.id,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        token_expires_at: tokens.token_expires_at,
        online,
        cache_lifetime,
    })
}

/// How long until ESI has a newer online status. Both times come from the response
/// itself, so a wrong local clock does not matter. About a minute when ESI does not
/// say, and never more than a few minutes.
fn online_cache_lifetime(date: Option<u64>, expires: Option<u64>) -> Duration {
    let secs = match (date, expires) {
        (Some(date), Some(expires)) => expires.saturating_sub(date),
        _ => ONLINE_CACHE_SECS,
    };
    Duration::from_secs(secs.min(MAX_ONLINE_CACHE_SECS))
}

fn fresh_tokens(settings: &EsiSettings, account: &Account) -> Result<Tokens, String> {
    let mut tokens = Tokens {
        access_token: account
            .access_token
            .clone()
            .ok_or_else(|| "Account is missing an ESI access token".to_owned())?,
        refresh_token: account
            .refresh_token
            .clone()
            .ok_or_else(|| "Account is missing an ESI refresh token".to_owned())?,
        token_expires_at: account.token_expires_at.unwrap_or_default(),
    };

    if tokens.token_expires_at <= now() + 60 {
        let refreshed = refresh_token_pair(settings, &tokens.refresh_token)?;
        tokens.access_token = refreshed.access_token;
        if let Some(new_refresh_token) = refreshed.refresh_token {
            tokens.refresh_token = new_refresh_token;
        }
        tokens.token_expires_at = now() + refreshed.expires_in;
    }
    Ok(tokens)
}

fn character_id(account: &Account) -> Result<u64, String> {
    account
        .character_id
        .parse::<u64>()
        .map_err(|_| "Account does not have a valid EVE character ID".to_owned())
}

/// The scopes an SSO access token was granted, read from its JWT `scp` claim. The
/// token came from EVE's SSO to Nomadeer, so its signature is not checked here.
/// `None` when the token cannot be read.
pub(crate) fn token_scopes(access_token: &str) -> Option<Vec<String>> {
    let mut parts = access_token.split('.');
    let (_header, payload, _signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let payload = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims = serde_json::from_slice::<serde_json::Value>(&payload).ok()?;
    match claims.as_object()?.get("scp") {
        // A token granted a single scope carries it as a plain string.
        Some(serde_json::Value::String(scope)) => {
            Some(scope.split_whitespace().map(str::to_owned).collect())
        }
        Some(serde_json::Value::Array(scopes)) => scopes
            .iter()
            .map(|scope| scope.as_str().map(str::to_owned))
            .collect(),
        None => Some(Vec::new()),
        Some(_) => None,
    }
}

/// Whether the access token was granted `scope`; `None` when it cannot be read.
pub(crate) fn token_has_scope(access_token: &str, scope: &str) -> Option<bool> {
    token_scopes(access_token).map(|scopes| scopes.iter().any(|granted| granted == scope))
}

/// A space-separated scope list that also asks for `scope`.
pub(crate) fn with_scope(scopes: &str, scope: &str) -> String {
    let scopes = scopes.trim();
    if scopes
        .split_whitespace()
        .any(|requested| requested == scope)
    {
        scopes.to_owned()
    } else if scopes.is_empty() {
        scope.to_owned()
    } else {
        format!("{scopes} {scope}")
    }
}

fn validate_settings(settings: &EsiSettings) -> Result<(), String> {
    if settings.client_id.trim().is_empty() {
        return Err("Set your EVE application Client ID in Settings first".to_owned());
    }
    if settings.callback_port == 0 {
        return Err("Callback port must be greater than zero".to_owned());
    }
    if settings.scopes.trim().is_empty() {
        return Err("At least one ESI scope is required".to_owned());
    }
    Ok(())
}

fn redirect_uri(port: u16) -> String {
    format!("http://localhost:{port}/callback")
}

fn random_urlsafe(bytes: usize) -> Result<String, String> {
    let mut data = vec![0_u8; bytes];
    getrandom::fill(&mut data)
        .map_err(|error| format!("Could not generate OAuth state: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(data))
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn authorization_url(
    settings: &EsiSettings,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> Result<Url, String> {
    let mut url = Url::parse(AUTHORIZE_URL).map_err(|error| error.to_string())?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("client_id", settings.client_id.trim())
        .append_pair("scope", settings.scopes.trim())
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url)
}

fn wait_for_code(server: &Server, expected_state: &str) -> Result<String, String> {
    let request = server
        .recv_timeout(Duration::from_secs(180))
        .map_err(|error| format!("OAuth callback failed: {error}"))?
        .ok_or_else(|| "Timed out waiting for ESI authorization callback".to_owned())?;

    let url = Url::parse(&format!("http://localhost{}", request.url()))
        .map_err(|error| format!("Could not parse OAuth callback: {error}"))?;
    let mut code = None;
    let mut state = None;
    let mut oauth_error = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.to_string()),
            "state" => state = Some(value.to_string()),
            "error" => oauth_error = Some(value.to_string()),
            _ => {}
        }
    }

    let message = if oauth_error.is_some() {
        "Nomadeer could not complete ESI authorization. You can close this tab."
    } else {
        "Nomadeer received ESI authorization. You can close this tab."
    };
    let _ = request.respond(Response::from_string(message).with_status_code(StatusCode(200)));

    if let Some(error) = oauth_error {
        return Err(format!("ESI authorization failed: {error}"));
    }
    if state.as_deref() != Some(expected_state) {
        return Err("ESI authorization state did not match; refusing callback".to_owned());
    }
    code.ok_or_else(|| "ESI authorization callback did not include a code".to_owned())
}

fn exchange_code(
    settings: &EsiSettings,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
) -> Result<TokenResponse, String> {
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", settings.client_id.trim()),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
    ];
    post_token_form(&params)
}

fn refresh_token_pair(
    settings: &EsiSettings,
    refresh_token: &str,
) -> Result<TokenResponse, String> {
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", settings.client_id.trim()),
    ];
    post_token_form(&params)
}

fn post_token_form(params: &[(&str, &str)]) -> Result<TokenResponse, String> {
    let response = Client::new()
        .post(TOKEN_URL)
        .header("User-Agent", USER_AGENT)
        .form(params)
        .send()
        .map_err(|error| format!("ESI token request failed: {error}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .unwrap_or_else(|_| "<unreadable body>".to_owned());
        return Err(format!("ESI token request returned {status}: {body}"));
    }

    response
        .json::<TokenResponse>()
        .map_err(|error| format!("Could not parse ESI token response: {error}"))
}

fn verify_token(access_token: &str) -> Result<VerifyResponse, String> {
    get_json(VERIFY_URL, Some(access_token))
}

fn fetch_location(character_id: u64, access_token: &str) -> Result<LocationResponse, String> {
    let url = format!("{ESI_BASE_URL}/characters/{character_id}/location/?datasource=tranquility");
    get_json(&url, Some(access_token))
}

fn fetch_system(solar_system_id: u64) -> Result<SystemResponse, String> {
    let url = format!("{ESI_BASE_URL}/universe/systems/{solar_system_id}/?datasource=tranquility");
    get_json(&url, None)
}

fn fetch_system_intel(solar_system_id: u64) -> Result<SystemIntel, String> {
    let mut intel = fetch_system_details(solar_system_id)?;
    intel.jumps = cached_feed(&SYSTEM_JUMPS_FEED, SYSTEM_JUMPS_PATH, true)
        .ok()
        .map(|feed| {
            let jumps = feed
                .entries
                .iter()
                .find(|jump| jump.system_id == solar_system_id)
                .map_or(0, |jump| jump.ship_jumps);
            feed.reading(jumps)
        });
    intel.kills = cached_feed(&SYSTEM_KILLS_FEED, SYSTEM_KILLS_PATH, true)
        .ok()
        .map(|feed| {
            let kills = feed
                .entries
                .iter()
                .find(|kill| kill.system_id == solar_system_id)
                .map(kill_counts)
                .unwrap_or_default();
            feed.reading(kills)
        });
    Ok(intel)
}

fn fetch_system_details(solar_system_id: u64) -> Result<SystemIntel, String> {
    let cache = SYSTEM_DETAILS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock()
        && let Some(intel) = cache.get(&solar_system_id)
    {
        return Ok(intel.clone());
    }

    let system = fetch_system(solar_system_id)?;
    let constellation = fetch_constellation(system.constellation_id)?;
    let region = fetch_region(constellation.region_id)?;

    let intel = SystemIntel {
        solar_system_id,
        name: system.name,
        security_class: system
            .security_class
            .unwrap_or_else(|| "unknown".to_owned()),
        security_status: system.security_status.unwrap_or_default(),
        constellation_id: system.constellation_id,
        constellation_name: constellation.name,
        region_id: constellation.region_id,
        region_name: region.name,
        planet_count: system
            .planets
            .map(|items| items.len() as u32)
            .unwrap_or_default(),
        stargate_count: system
            .stargates
            .map(|items| items.len() as u32)
            .unwrap_or_default(),
        station_count: system
            .stations
            .map(|items| items.len() as u32)
            .unwrap_or_default(),
        jumps: None,
        kills: None,
    };

    if let Ok(mut cache) = cache.lock() {
        cache.insert(solar_system_id, intel.clone());
    }
    Ok(intel)
}

fn fetch_constellation(constellation_id: u64) -> Result<ConstellationResponse, String> {
    let url = format!(
        "{ESI_BASE_URL}/universe/constellations/{constellation_id}/?datasource=tranquility"
    );
    get_json(&url, None)
}

fn fetch_region(region_id: u64) -> Result<RegionResponse, String> {
    let url = format!("{ESI_BASE_URL}/universe/regions/{region_id}/?datasource=tranquility");
    get_json(&url, None)
}

/// Reads one universe-wide activity feed, reusing ESI's response until it expires.
pub(crate) fn fetch_traffic_feed(feed: TrafficFeed) -> Result<TrafficCapture, String> {
    let (hour, data_time, expires_at, rows) = match feed {
        TrafficFeed::Jumps => {
            let feed = cached_feed(&SYSTEM_JUMPS_FEED, SYSTEM_JUMPS_PATH, false)?;
            let rows = feed
                .entries
                .iter()
                .filter(|jump| jump.ship_jumps > 0)
                .map(|jump| (jump.system_id, jump.ship_jumps))
                .collect();
            (
                feed.hour,
                feed.data_time,
                feed.expires_at,
                TrafficRows::Jumps(rows),
            )
        }
        TrafficFeed::Kills => {
            let feed = cached_feed(&SYSTEM_KILLS_FEED, SYSTEM_KILLS_PATH, false)?;
            let rows = feed
                .entries
                .iter()
                .map(|kill| (kill.system_id, kill_counts(kill)))
                .filter(|(_, kills)| *kills != KillCounts::default())
                .collect();
            (
                feed.hour,
                feed.data_time,
                feed.expires_at,
                TrafficRows::Kills(rows),
            )
        }
    };
    Ok(TrafficCapture {
        traffic: TrafficHour {
            hour,
            data_time,
            rows,
        },
        expires_at,
    })
}

/// How long to wait before polling a feed again after ESI served it.
pub(crate) fn traffic_next_poll_delay(expires_at: u64, current_time: u64) -> Duration {
    let wait = expires_at.saturating_sub(current_time) + TRAFFIC_EXPIRES_SLACK_SECS;
    Duration::from_secs(wait).max(TRAFFIC_MIN_POLL)
}

/// Backoff after `failures` consecutive failed polls of a feed.
pub(crate) fn traffic_retry_delay(failures: u32) -> Duration {
    let factor = 1_u32 << failures.saturating_sub(1).min(8);
    (TRAFFIC_MIN_POLL * factor).min(TRAFFIC_MAX_RETRY)
}

fn kill_counts(kill: &SystemKillResponse) -> KillCounts {
    KillCounts {
        ship_kills: kill.ship_kills,
        pod_kills: kill.pod_kills,
        npc_kills: kill.npc_kills,
    }
}

/// Returns the cached feed while ESI says it is current, otherwise fetches it again.
/// With `allow_stale`, a failed refresh falls back to the last good copy, which still
/// carries its real data time.
fn cached_feed<T>(
    cache: &'static FeedCacheLock<T>,
    path: &str,
    allow_stale: bool,
) -> Result<FeedCache<T>, String>
where
    T: DeserializeOwned,
{
    let cache = cache.get_or_init(|| Mutex::new(None));
    let cached = cache.lock().ok().and_then(|cache| cache.clone());
    if let Some(cached) = &cached
        && now() < cached.expires_at
    {
        return Ok(cached.clone());
    }

    match fetch_feed(path) {
        Ok(fresh) => {
            if let Ok(mut cache) = cache.lock() {
                *cache = Some(fresh.clone());
            }
            Ok(fresh)
        }
        Err(error) => match cached {
            Some(stale) if allow_stale => Ok(stale),
            _ => Err(error),
        },
    }
}

fn fetch_feed<T>(path: &str) -> Result<FeedCache<T>, String>
where
    T: DeserializeOwned,
{
    let response = get_response(
        &format!("{ESI_BASE_URL}{path}?datasource=tranquility"),
        None,
    )?;
    let last_modified = header_time(&response, LAST_MODIFIED);
    let expires = header_time(&response, EXPIRES);
    let (data_time, expires_at) = feed_times(last_modified, expires)?;
    let entries = response
        .json::<Vec<T>>()
        .map_err(|error| format!("Could not parse ESI response: {error}"))?;
    Ok(FeedCache {
        hour: hour_start(data_time),
        data_time,
        expires_at,
        entries: Arc::new(entries),
    })
}

fn header_time(response: &HttpResponse, header: HeaderName) -> Option<u64> {
    response
        .headers()
        .get(header)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_http_date)
}

/// Works out when a feed's data was published and when ESI will replace it.
fn feed_times(last_modified: Option<u64>, expires: Option<u64>) -> Result<(u64, u64), String> {
    let data_time = last_modified
        .or_else(|| expires.map(|expires| expires.saturating_sub(HOUR_SECS)))
        .ok_or_else(|| "ESI did not say when this activity data was published".to_owned())?;
    let expires_at = expires.unwrap_or(data_time + HOUR_SECS);
    Ok((data_time, expires_at))
}

fn get_json<T>(url: &str, access_token: Option<&str>) -> Result<T, String>
where
    T: DeserializeOwned,
{
    get_response(url, access_token)?
        .json::<T>()
        .map_err(|error| format!("Could not parse ESI response: {error}"))
}

fn get_response(url: &str, access_token: Option<&str>) -> Result<HttpResponse, String> {
    let client = Client::new();
    let mut request = client.get(url).header("User-Agent", USER_AGENT);
    if let Some(access_token) = access_token {
        request = request.bearer_auth(access_token);
    }

    let response = request
        .send()
        .map_err(|error| format!("ESI request failed: {error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .unwrap_or_else(|_| "<unreadable body>".to_owned());
        return Err(format!("ESI request returned {status}: {body}"));
    }

    Ok(response)
}

// ---------------------------------------------------------------------------------
// Name and type lookups for pasted local and d-scans. Requests are batched to ESI's
// limits, and every lookup steps back while ESI's error budget is nearly spent or it
// has asked Nomadeer to slow down.

/// Names per `/universe/ids/` request.
pub(crate) const MAX_NAMES_PER_ID_LOOKUP: usize = 500;
/// Character IDs per `/characters/affiliation/` request.
pub(crate) const MAX_IDS_PER_AFFILIATION: usize = 1000;
/// IDs per `/universe/names/` request.
pub(crate) const MAX_IDS_PER_NAME_LOOKUP: usize = 1000;
/// Lookups pause once ESI's error budget is down to this many errors.
const ERROR_LIMIT_MARGIN: u32 = 20;
/// How long lookups pause when ESI throttles without saying for how long.
const DEFAULT_LOOKUP_PAUSE_SECS: u64 = 60;
/// ESI caches affiliations for an hour; Nomadeer keeps them between a minute and
/// six hours, whatever its headers say.
const DEFAULT_AFFILIATION_SECS: u64 = 60 * 60;
const MAX_AFFILIATION_SECS: u64 = 6 * 60 * 60;

/// Until when lookups wait, in Unix seconds; 0 when they need not.
static LOOKUPS_PAUSED_UNTIL: Mutex<u64> = Mutex::new(0);
static LOOKUP_CLIENT: OnceLock<Client> = OnceLock::new();

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct IdAndName {
    pub(crate) id: u64,
    pub(crate) name: String,
}

/// What `/universe/ids/` matched; categories Nomadeer does not ask about are dropped.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct UniverseIds {
    #[serde(default)]
    pub(crate) characters: Vec<IdAndName>,
    #[serde(default)]
    pub(crate) inventory_types: Vec<IdAndName>,
}

#[derive(Debug, Deserialize)]
struct AffiliationResponse {
    character_id: u64,
    corporation_id: u64,
    alliance_id: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct NamedEntity {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) category: String,
}

#[derive(Debug, Deserialize)]
struct TypeResponse {
    name: String,
    group_id: u32,
}

#[derive(Debug, Deserialize)]
struct GroupResponse {
    name: String,
    category_id: u32,
}

/// Exact-name matches for up to [`MAX_NAMES_PER_ID_LOOKUP`] names.
pub(crate) fn universe_ids(names: &[String]) -> Result<UniverseIds, String> {
    if names.is_empty() {
        return Ok(UniverseIds::default());
    }
    let url = format!("{ESI_BASE_URL}/universe/ids/?datasource=tranquility&language=en");
    let response = send_lookup(lookup_client().post(url).json(names))?;
    lookup_json(response)
}

/// Corporations and alliances of up to [`MAX_IDS_PER_AFFILIATION`] characters, and
/// until when they can be reused.
pub(crate) fn character_affiliations(
    character_ids: &[u64],
) -> Result<(Vec<CharacterAffiliation>, u64), String> {
    if character_ids.is_empty() {
        return Ok((Vec::new(), now()));
    }
    let url = format!("{ESI_BASE_URL}/characters/affiliation/?datasource=tranquility");
    let response = send_lookup(lookup_client().post(url).json(character_ids))?;
    let lifetime = affiliation_lifetime(
        header_time(&response, DATE),
        header_time(&response, EXPIRES),
    );
    let affiliations = lookup_json::<Vec<AffiliationResponse>>(response)?
        .into_iter()
        .map(|affiliation| CharacterAffiliation {
            character_id: affiliation.character_id,
            corporation_id: affiliation.corporation_id,
            alliance_id: affiliation.alliance_id.unwrap_or_default(),
        })
        .collect();
    Ok((affiliations, now() + lifetime))
}

/// Names of up to [`MAX_IDS_PER_NAME_LOOKUP`] IDs. ESI refuses the whole request
/// when one ID is unknown, so a refused batch is split until the bad IDs are found
/// and left out.
pub(crate) fn universe_names(ids: &[u64]) -> Result<Vec<NamedEntity>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!("{ESI_BASE_URL}/universe/names/?datasource=tranquility");
    let response = send_lookup(lookup_client().post(url).json(ids))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        if ids.len() == 1 {
            return Ok(Vec::new());
        }
        let (first, second) = ids.split_at(ids.len() / 2);
        let mut names = universe_names(first)?;
        names.extend(universe_names(second)?);
        return Ok(names);
    }
    lookup_json(response)
}

/// An item type's name and group.
pub(crate) fn type_and_group(type_id: u32) -> Result<(String, u32), String> {
    let url =
        format!("{ESI_BASE_URL}/universe/types/{type_id}/?datasource=tranquility&language=en");
    let response = lookup_json::<TypeResponse>(send_lookup(lookup_client().get(url))?)?;
    Ok((response.name, response.group_id))
}

/// An item group's name and category.
pub(crate) fn group_and_category(group_id: u32) -> Result<(String, u32), String> {
    let url =
        format!("{ESI_BASE_URL}/universe/groups/{group_id}/?datasource=tranquility&language=en");
    let response = lookup_json::<GroupResponse>(send_lookup(lookup_client().get(url))?)?;
    Ok((response.name, response.category_id))
}

fn lookup_client() -> &'static Client {
    LOOKUP_CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

/// Sends a lookup unless ESI asked Nomadeer to wait, and notes how much of ESI's
/// error budget is left. Any answer but a throttle comes back for the caller to read.
fn send_lookup(request: reqwest::blocking::RequestBuilder) -> Result<HttpResponse, String> {
    let current_time = now();
    let paused_until = LOOKUPS_PAUSED_UNTIL.lock().map_or(0, |until| *until);
    if paused_until > current_time {
        return Err(format!(
            "ESI asked Nomadeer to slow down; lookups resume in {}s",
            paused_until - current_time
        ));
    }
    let response = request
        .header("User-Agent", USER_AGENT)
        .send()
        .map_err(|error| format!("ESI request failed: {error}"))?;
    let header_number = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
    };
    let pause = lookup_pause(
        response.status().as_u16(),
        header_number("x-esi-error-limit-remain"),
        header_number("x-esi-error-limit-reset"),
        header_number("retry-after"),
    );
    if let Some(pause) = pause
        && let Ok(mut until) = LOOKUPS_PAUSED_UNTIL.lock()
    {
        *until = (*until).max(current_time + pause);
    }
    if matches!(response.status().as_u16(), 420 | 429) {
        return Err("ESI asked Nomadeer to slow down; try again in a minute".to_owned());
    }
    Ok(response)
}

/// How long lookups should pause after a response, if at all: when ESI throttles
/// (420 for the error limit, 429 for rate limits), or when its error budget is
/// nearly spent.
fn lookup_pause(
    status: u16,
    error_limit_remain: Option<u64>,
    error_limit_reset: Option<u64>,
    retry_after: Option<u64>,
) -> Option<u64> {
    if matches!(status, 420 | 429) {
        return Some(
            retry_after
                .or(error_limit_reset)
                .unwrap_or(DEFAULT_LOOKUP_PAUSE_SECS)
                .max(1),
        );
    }
    match error_limit_remain {
        Some(remain) if remain <= u64::from(ERROR_LIMIT_MARGIN) => Some(
            error_limit_reset
                .unwrap_or(DEFAULT_LOOKUP_PAUSE_SECS)
                .max(1),
        ),
        _ => None,
    }
}

fn lookup_json<T>(response: HttpResponse) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let status = response.status();
    if !status.is_success() {
        return Err(format!("ESI lookup returned {status}"));
    }
    response
        .json::<T>()
        .map_err(|error| format!("Could not parse ESI response: {error}"))
}

/// How long ESI's affiliation answer holds, from its own Date and Expires headers.
fn affiliation_lifetime(date: Option<u64>, expires: Option<u64>) -> u64 {
    match (date, expires) {
        (Some(date), Some(expires)) => expires.saturating_sub(date),
        _ => DEFAULT_AFFILIATION_SECS,
    }
    .clamp(60, MAX_AFFILIATION_SECS)
}

#[cfg(test)]
mod tests {
    use super::{
        ONLINE_SCOPE, affiliation_lifetime, feed_times, lookup_pause, online_cache_lifetime,
        token_has_scope, token_scopes, traffic_next_poll_delay, traffic_retry_delay, with_scope,
    };
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use std::time::Duration;

    fn jwt(claims: &str) -> String {
        format!(
            "{}.{}.signature",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#),
            URL_SAFE_NO_PAD.encode(claims)
        )
    }

    #[test]
    fn token_scopes_read_the_scp_claim_as_array_or_string() {
        let token = jwt(&format!(
            r#"{{"scp":["publicData","{ONLINE_SCOPE}"],"sub":"CHARACTER:EVE:1"}}"#
        ));
        assert_eq!(
            token_scopes(&token),
            Some(vec!["publicData".to_owned(), ONLINE_SCOPE.to_owned()])
        );
        assert_eq!(token_has_scope(&token, ONLINE_SCOPE), Some(true));

        let single = jwt(r#"{"scp":"esi-location.read_location.v1"}"#);
        assert_eq!(
            token_scopes(&single),
            Some(vec!["esi-location.read_location.v1".to_owned()])
        );
        assert_eq!(token_has_scope(&single, ONLINE_SCOPE), Some(false));

        let unscoped = jwt(r#"{"sub":"CHARACTER:EVE:1"}"#);
        assert_eq!(token_has_scope(&unscoped, ONLINE_SCOPE), Some(false));

        // Padding some encoders leave on is tolerated.
        let padded = format!(
            "e30.{}=.sig",
            URL_SAFE_NO_PAD.encode(r#"{"scp":["publicData"]}"#)
        );
        assert_eq!(token_scopes(&padded), Some(vec!["publicData".to_owned()]));
    }

    #[test]
    fn malformed_tokens_have_no_known_scopes() {
        assert_eq!(token_scopes(""), None);
        assert_eq!(token_scopes("not-a-jwt"), None);
        assert_eq!(token_scopes("a.b"), None);
        assert_eq!(token_scopes("a.b.c.d"), None);
        assert_eq!(token_scopes("e30.!!!.sig"), None);
        assert_eq!(token_has_scope(&jwt("not json"), ONLINE_SCOPE), None);
        assert_eq!(token_scopes(&jwt(r#"["publicData"]"#)), None);
        assert_eq!(token_scopes(&jwt(r#"{"scp":42}"#)), None);
        assert_eq!(token_scopes(&jwt(r#"{"scp":["publicData",7]}"#)), None);
    }

    #[test]
    fn with_scope_adds_a_scope_once() {
        assert_eq!(
            with_scope("publicData", ONLINE_SCOPE),
            format!("publicData {ONLINE_SCOPE}")
        );
        assert_eq!(with_scope("  ", ONLINE_SCOPE), ONLINE_SCOPE);
        let both = format!("publicData {ONLINE_SCOPE}");
        assert_eq!(with_scope(&format!(" {both} "), ONLINE_SCOPE), both);
    }

    #[test]
    fn online_status_lasts_as_esi_says_within_limits() {
        let secs = Duration::from_secs;
        assert_eq!(online_cache_lifetime(Some(1_000), Some(1_030)), secs(30));
        // Without both headers, about a minute.
        assert_eq!(online_cache_lifetime(None, Some(1_030)), secs(60));
        assert_eq!(online_cache_lifetime(Some(1_000), None), secs(60));
        assert_eq!(online_cache_lifetime(None, None), secs(60));
        // Already expired, or absurdly far off.
        assert_eq!(online_cache_lifetime(Some(1_000), Some(900)), secs(0));
        assert_eq!(online_cache_lifetime(Some(1_000), Some(37_000)), secs(300));
    }

    #[test]
    fn feed_times_prefer_last_modified() {
        assert_eq!(feed_times(Some(10_000), Some(12_000)), Ok((10_000, 12_000)));
        assert_eq!(feed_times(None, Some(12_000)), Ok((8_400, 12_000)));
        assert_eq!(feed_times(Some(10_000), None), Ok((10_000, 13_600)));
        assert!(feed_times(None, None).is_err());
    }

    #[test]
    fn traffic_polls_wait_for_expiry() {
        assert_eq!(
            traffic_next_poll_delay(2_000, 1_000),
            Duration::from_secs(1_015)
        );
        assert_eq!(traffic_next_poll_delay(900, 1_000), Duration::from_secs(60));
    }

    #[test]
    fn traffic_retries_back_off_and_cap() {
        assert_eq!(traffic_retry_delay(1), Duration::from_secs(60));
        assert_eq!(traffic_retry_delay(2), Duration::from_secs(120));
        assert_eq!(traffic_retry_delay(4), Duration::from_secs(480));
        assert_eq!(traffic_retry_delay(5), Duration::from_secs(900));
        assert_eq!(traffic_retry_delay(40), Duration::from_secs(900));
    }

    #[test]
    fn lookups_pause_when_esi_throttles_or_errors_run_low() {
        // Throttled: as long as ESI says, else until the error window resets.
        assert_eq!(lookup_pause(420, Some(0), Some(37), None), Some(37));
        assert_eq!(lookup_pause(429, None, None, Some(5)), Some(5));
        assert_eq!(lookup_pause(429, None, None, None), Some(60));
        // A healthy budget, even after an error, needs no pause.
        assert_eq!(lookup_pause(200, Some(100), Some(40), None), None);
        assert_eq!(lookup_pause(404, Some(99), Some(40), None), None);
        assert_eq!(lookup_pause(200, None, None, None), None);
        // Nearly spent: wait out the window.
        assert_eq!(lookup_pause(200, Some(20), Some(12), None), Some(12));
        assert_eq!(lookup_pause(502, Some(3), None, None), Some(60));
    }

    #[test]
    fn affiliations_are_kept_as_long_as_esi_says_within_limits() {
        assert_eq!(affiliation_lifetime(Some(1_000), Some(4_600)), 3_600);
        assert_eq!(affiliation_lifetime(None, Some(4_600)), 3_600);
        assert_eq!(affiliation_lifetime(Some(1_000), Some(1_010)), 60);
        assert_eq!(affiliation_lifetime(Some(1_000), Some(100_000)), 21_600);
    }
}
