use crate::model::{Account, EsiSettings};
use crate::util::now;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::blocking::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tiny_http::{Response, Server, StatusCode};
use url::Url;

const AUTHORIZE_URL: &str = "https://login.eveonline.com/v2/oauth/authorize/";
const TOKEN_URL: &str = "https://login.eveonline.com/v2/oauth/token";
const VERIFY_URL: &str = "https://login.eveonline.com/oauth/verify";
const ESI_BASE_URL: &str = "https://esi.evetech.net/latest";
const USER_AGENT: &str = "Nomadeer/0.1 private wormhole mapper";

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
    pub(crate) solar_system_id: u64,
    pub(crate) solar_system_name: String,
    pub(crate) solar_system_class: String,
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

#[derive(Debug, Deserialize)]
struct LocationResponse {
    solar_system_id: u64,
}

#[derive(Debug, Deserialize)]
struct SystemResponse {
    name: String,
    security_class: Option<String>,
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

    webbrowser::open(auth_url.as_str())
        .map_err(|error| format!("Could not open browser for ESI login: {error}"))?;

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
    let mut access_token = account
        .access_token
        .clone()
        .ok_or_else(|| "Account is missing an ESI access token".to_owned())?;
    let mut refresh_token = account
        .refresh_token
        .clone()
        .ok_or_else(|| "Account is missing an ESI refresh token".to_owned())?;
    let mut token_expires_at = account.token_expires_at.unwrap_or_default();

    if token_expires_at <= now() + 60 {
        let refreshed = refresh_token_pair(&settings, &refresh_token)?;
        access_token = refreshed.access_token;
        if let Some(new_refresh_token) = refreshed.refresh_token {
            refresh_token = new_refresh_token;
        }
        token_expires_at = now() + refreshed.expires_in;
    }

    let character_id = account
        .character_id
        .parse::<u64>()
        .map_err(|_| "Account does not have a valid EVE character ID".to_owned())?;
    let location = fetch_location(character_id, &access_token)?;
    let system = fetch_system(location.solar_system_id)?;

    Ok(LocationSync {
        account_id: account.id,
        access_token,
        refresh_token,
        token_expires_at,
        solar_system_id: location.solar_system_id,
        solar_system_name: system.name,
        solar_system_class: system
            .security_class
            .unwrap_or_else(|| "unknown".to_owned()),
    })
}

fn validate_settings(settings: &EsiSettings) -> Result<(), String> {
    if settings.client_id.trim().is_empty() {
        return Err("Set your EVE application Client ID in ESI settings first".to_owned());
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

fn get_json<T>(url: &str, access_token: Option<&str>) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
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

    response
        .json::<T>()
        .map_err(|error| format!("Could not parse ESI response: {error}"))
}
