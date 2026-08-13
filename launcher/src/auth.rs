use crate::util::now_epoch_secs;
use anyhow::{Context, Result, bail};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::elliptic_curve::rand_core::OsRng;
use reqwest::blocking::Client;
use reqwest::header::HeaderMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::thread::sleep;
use std::time::Duration;

const MS_DEVICE_CODE_URL: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const MS_TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const MS_LIVE_TOKEN_URL: &str = "https://login.live.com/oauth20_token.srf";
pub const MS_BROWSER_REDIRECT_URL: &str = "https://login.live.com/oauth20_desktop.srf";
pub const DEFAULT_MS_CLIENT_ID: &str = "00000000402b5328";
const MS_BROWSER_SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";
const XBL_AUTH_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub message: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone)]
pub struct OAuthToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
}

#[derive(Debug)]
pub struct BrowserLoginFlow {
    pub auth_url: String,
    pub verifier: String,
    pub state: String,
    session_id: String,
    device_token: String,
    proof_key: DeviceProofKey,
}

#[derive(Debug)]
struct DeviceProofKey {
    id: String,
    signing_key: SigningKey,
    x: String,
    y: String,
}

#[derive(Deserialize)]
struct SisuDeviceToken {
    #[serde(rename = "Token")]
    token: String,
}

#[derive(Deserialize)]
struct SisuRedirect {
    #[serde(rename = "MsaOauthRedirect")]
    msa_oauth_redirect: String,
}

#[derive(Deserialize)]
struct SisuAuthorizeResponse {
    #[serde(rename = "TitleToken")]
    title_token: SisuDeviceToken,
    #[serde(rename = "UserToken")]
    user_token: SisuDeviceToken,
}

#[derive(Debug, Clone)]
pub struct MinecraftAuth {
    pub access_token: String,
    pub expires_at: u64,
    pub uuid: String,
    pub username: String,
    pub xuid: Option<String>,
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    message: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

#[derive(Serialize)]
struct XblRequest<'a> {
    #[serde(rename = "Properties")]
    properties: XblProperties<'a>,
    #[serde(rename = "RelyingParty")]
    relying_party: &'a str,
    #[serde(rename = "TokenType")]
    token_type: &'a str,
}

#[derive(Serialize)]
struct XblProperties<'a> {
    #[serde(rename = "AuthMethod")]
    auth_method: &'a str,
    #[serde(rename = "SiteName")]
    site_name: &'a str,
    #[serde(rename = "RpsTicket")]
    rps_ticket: String,
}

#[derive(Deserialize)]
struct XblResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: DisplayClaims,
}

#[derive(Deserialize)]
struct DisplayClaims {
    xui: Vec<Xui>,
}

#[derive(Deserialize)]
struct Xui {
    #[serde(default)]
    uhs: String,
    #[serde(default)]
    xid: Option<String>,
    #[serde(default)]
    xuid: Option<String>,
}

#[derive(Serialize)]
struct XstsRequest<'a> {
    #[serde(rename = "Properties")]
    properties: XstsProperties<'a>,
    #[serde(rename = "RelyingParty")]
    relying_party: &'a str,
    #[serde(rename = "TokenType")]
    token_type: &'a str,
}

#[derive(Serialize)]
struct XstsProperties<'a> {
    #[serde(rename = "SandboxId")]
    sandbox_id: &'a str,
    #[serde(rename = "UserTokens")]
    user_tokens: Vec<&'a str>,
}

#[derive(Serialize)]
struct McLoginRequest<'a> {
    #[serde(rename = "identityToken")]
    identity_token: String,
    #[serde(
        rename = "ensureLegacyEnabled",
        skip_serializing_if = "Option::is_none"
    )]
    ensure_legacy_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<&'a str>,
}

#[derive(Deserialize)]
struct McLoginResponse {
    access_token: String,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct McProfile {
    id: String,
    name: String,
}

fn random_url_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|err| anyhow::anyhow!("failed to generate OAuth secret: {err}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn generate_device_proof_key() -> Result<DeviceProofKey> {
    let mut id = [0_u8; 16];
    getrandom::fill(&mut id)
        .map_err(|err| anyhow::anyhow!("failed to generate Xbox device ID: {err}"))?;
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    let id = format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        id[0],
        id[1],
        id[2],
        id[3],
        id[4],
        id[5],
        id[6],
        id[7],
        id[8],
        id[9],
        id[10],
        id[11],
        id[12],
        id[13],
        id[14],
        id[15]
    );
    let signing_key = SigningKey::random(&mut OsRng);
    let point = VerifyingKey::from(&signing_key).to_encoded_point(false);
    let x = point.x().context("failed to read Xbox device public key")?;
    let y = point.y().context("failed to read Xbox device public key")?;
    Ok(DeviceProofKey {
        id,
        signing_key,
        x: URL_SAFE_NO_PAD.encode(x),
        y: URL_SAFE_NO_PAD.encode(y),
    })
}

fn signed_xbox_post<T: DeserializeOwned>(
    url: &str,
    path: &str,
    body: Value,
    key: &DeviceProofKey,
) -> Result<(HeaderMap, T)> {
    let body = serde_json::to_vec(&body).context("failed to serialize Xbox request")?;
    let windows_time = (now_epoch_secs() + 11_644_473_600) * 10_000_000;
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&1_u32.to_be_bytes());
    buffer.push(0);
    buffer.extend_from_slice(&windows_time.to_be_bytes());
    buffer.push(0);
    buffer.extend_from_slice(b"POST");
    buffer.push(0);
    buffer.extend_from_slice(path.as_bytes());
    buffer.push(0);
    buffer.push(0);
    buffer.extend_from_slice(&body);
    buffer.push(0);

    let signature: Signature = key.signing_key.sign(&buffer);
    let mut signature_bytes = Vec::with_capacity(76);
    signature_bytes.extend_from_slice(&1_i32.to_be_bytes());
    signature_bytes.extend_from_slice(&windows_time.to_be_bytes());
    signature_bytes.extend_from_slice(&signature.r().to_bytes());
    signature_bytes.extend_from_slice(&signature.s().to_bytes());

    let mut request = Client::new()
        .post(url)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Accept", "application/json")
        .header("Signature", STANDARD.encode(signature_bytes));
    if url != "https://sisu.xboxlive.com/authorize" {
        request = request.header("x-xbl-contract-version", "1");
    }
    let response = request
        .body(body)
        .send()
        .context("Xbox signed request failed")?;
    let status = response.status();
    let headers = response.headers().clone();
    let raw = response.text().context("failed to read Xbox response")?;
    if !status.is_success() {
        bail!("Xbox authentication failed: {status} {raw}");
    }
    let parsed = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse Xbox response: {raw}"))?;
    Ok((headers, parsed))
}

fn request_sisu_device_token(key: &DeviceProofKey) -> Result<String> {
    let (_, token): (_, SisuDeviceToken) = signed_xbox_post(
        "https://device.auth.xboxlive.com/device/authenticate",
        "/device/authenticate",
        serde_json::json!({
            "Properties": {
                "AuthMethod": "ProofOfPossession",
                "Id": format!("{{{}}}", key.id),
                "DeviceType": "Win32",
                "Version": "10.16.0",
                "ProofKey": { "kty": "EC", "x": key.x, "y": key.y, "crv": "P-256", "alg": "ES256", "use": "sig" }
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }),
        key,
    )?;
    Ok(token.token)
}

fn begin_sisu_login(
    key: &DeviceProofKey,
    device_token: &str,
    challenge: &str,
    state: &str,
) -> Result<(String, String)> {
    let (headers, redirect): (_, SisuRedirect) = signed_xbox_post(
        "https://sisu.xboxlive.com/authenticate",
        "/authenticate",
        serde_json::json!({
            "AppId": DEFAULT_MS_CLIENT_ID,
            "DeviceToken": device_token,
            "Offers": [MS_BROWSER_SCOPE],
            "Query": { "code_challenge": challenge, "code_challenge_method": "S256", "state": state, "prompt": "select_account" },
            "RedirectUri": MS_BROWSER_REDIRECT_URL,
            "Sandbox": "RETAIL",
            "TokenType": "code",
            "TitleId": "1794566092"
        }),
        key,
    )?;
    let session_id = headers
        .get("x-sessionid")
        .and_then(|value| value.to_str().ok())
        .context("Xbox Sisu response did not include a session ID")?;
    Ok((redirect.msa_oauth_redirect, session_id.to_string()))
}

pub fn begin_browser_login() -> Result<BrowserLoginFlow> {
    let verifier = random_url_token()?;
    let state = random_url_token()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let proof_key = generate_device_proof_key()?;
    let device_token = request_sisu_device_token(&proof_key)?;
    let (auth_url, session_id) = begin_sisu_login(&proof_key, &device_token, &challenge, &state)?;
    Ok(BrowserLoginFlow {
        auth_url,
        verifier,
        state,
        session_id,
        device_token,
        proof_key,
    })
}

pub fn exchange_browser_code(
    code: &str,
    state: &str,
    flow: &BrowserLoginFlow,
) -> Result<OAuthToken> {
    if state != flow.state {
        bail!("Microsoft sign-in state mismatch");
    }

    let params = [
        ("client_id", DEFAULT_MS_CLIENT_ID),
        ("code", code),
        ("code_verifier", flow.verifier.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", MS_BROWSER_REDIRECT_URL),
        ("scope", MS_BROWSER_SCOPE),
    ];
    let resp = Client::new()
        .post(MS_LIVE_TOKEN_URL)
        .form(&params)
        .send()
        .context("failed to exchange Microsoft authorization code")?;
    if !resp.status().is_success() {
        return Err(format_oauth_error("Microsoft sign-in failed", resp));
    }
    let data: TokenResponse = resp
        .json()
        .context("failed to parse Microsoft token response")?;
    let refresh_token = data
        .refresh_token
        .context("Microsoft refresh token missing")?;
    Ok(OAuthToken {
        access_token: data.access_token,
        refresh_token,
        expires_at: now_epoch_secs() + data.expires_in,
    })
}

fn sisu_authorize(
    access_token: &str,
    session_id: Option<&str>,
    device_token: &str,
    key: &DeviceProofKey,
) -> Result<SisuAuthorizeResponse> {
    let (_, response) = signed_xbox_post(
        "https://sisu.xboxlive.com/authorize",
        "/authorize",
        serde_json::json!({
            "AccessToken": format!("t={access_token}"),
            "AppId": DEFAULT_MS_CLIENT_ID,
            "DeviceToken": device_token,
            "ProofKey": { "kty": "EC", "x": key.x, "y": key.y, "crv": "P-256", "alg": "ES256", "use": "sig" },
            "Sandbox": "RETAIL",
            "SessionId": session_id,
            "SiteName": "user.auth.xboxlive.com",
            "RelyingParty": "http://xboxlive.com",
            "UseModernGamertag": true
        }),
        key,
    )?;
    Ok(response)
}

fn sisu_xsts_authorize(
    authorization: SisuAuthorizeResponse,
    device_token: &str,
    key: &DeviceProofKey,
) -> Result<XblResponse> {
    let (_, response) = signed_xbox_post(
        XSTS_AUTH_URL,
        "/xsts/authorize",
        serde_json::json!({
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [authorization.user_token.token],
                "DeviceToken": device_token,
                "TitleToken": authorization.title_token.token
            }
        }),
        key,
    )?;
    Ok(response)
}

fn minecraft_launcher_login(xsts: XblResponse) -> Result<MinecraftAuth> {
    let xui = xsts
        .display_claims
        .xui
        .into_iter()
        .next()
        .context("missing Xbox user hash")?;
    let response = Client::new()
        .post("https://api.minecraftservices.com/launcher/login")
        .header("Accept", "application/json")
        .header("User-Agent", "VelGrinor Launcher")
        .json(&serde_json::json!({
            "platform": "PC_LAUNCHER",
            "xtoken": format!("XBL3.0 x={};{}", xui.uhs, xsts.token)
        }))
        .send()
        .context("failed Minecraft launcher login request")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        bail!("Minecraft launcher login failed: {status} {body}");
    }
    let token: McLoginResponse = response
        .json()
        .context("failed to parse Minecraft launcher token")?;
    let profile = minecraft_profile(&token.access_token)?;
    Ok(MinecraftAuth {
        access_token: token.access_token,
        expires_at: now_epoch_secs() + token.expires_in.unwrap_or(24 * 60 * 60),
        uuid: profile.id,
        username: profile.name,
        xuid: xui.xuid.or(xui.xid),
    })
}

fn exchange_sisu_for_minecraft(
    access_token: &str,
    session_id: Option<&str>,
    device_token: &str,
    key: &DeviceProofKey,
) -> Result<MinecraftAuth> {
    let authorization = sisu_authorize(access_token, session_id, device_token, key)?;
    let xsts = sisu_xsts_authorize(authorization, device_token, key)?;
    minecraft_launcher_login(xsts)
}

pub fn finish_browser_login(
    code: &str,
    state: &str,
    flow: &BrowserLoginFlow,
) -> Result<(OAuthToken, MinecraftAuth)> {
    let token = exchange_browser_code(code, state, flow)?;
    let minecraft = exchange_sisu_for_minecraft(
        &token.access_token,
        Some(&flow.session_id),
        &flow.device_token,
        &flow.proof_key,
    )?;
    Ok((token, minecraft))
}

pub fn exchange_for_minecraft_sisu(access_token: &str) -> Result<MinecraftAuth> {
    let key = generate_device_proof_key()?;
    let device_token = request_sisu_device_token(&key)?;
    exchange_sisu_for_minecraft(access_token, None, &device_token, &key)
}

pub fn request_device_code(client_id: &str, client_secret: Option<&str>) -> Result<DeviceCode> {
    let client = Client::new();
    let scope = "XboxLive.signin offline_access";
    let mut params = vec![("client_id", client_id), ("scope", scope)];
    if let Some(secret) = client_secret {
        params.push(("client_secret", secret));
    }

    let resp = client
        .post(MS_DEVICE_CODE_URL)
        .form(&params)
        .send()
        .context("failed to request device code")?;

    if !resp.status().is_success() {
        return Err(format_oauth_error("device code request failed", resp));
    }

    let data: DeviceCodeResponse = resp
        .json()
        .context("failed to parse device code response")?;
    Ok(DeviceCode {
        device_code: data.device_code,
        user_code: data.user_code,
        verification_uri: data.verification_uri,
        message: data.message,
        expires_in: data.expires_in,
        interval: data.interval,
    })
}

pub fn poll_device_code(
    client_id: &str,
    client_secret: Option<&str>,
    device: &DeviceCode,
) -> Result<OAuthToken> {
    let client = Client::new();
    let mut interval = device.interval;
    let deadline = now_epoch_secs() + device.expires_in;

    loop {
        if now_epoch_secs() >= deadline {
            bail!("device code expired; please try again");
        }

        let mut params = vec![
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", client_id),
            ("device_code", device.device_code.as_str()),
        ];
        if let Some(secret) = client_secret {
            params.push(("client_secret", secret));
        }

        let resp = client
            .post(MS_TOKEN_URL)
            .form(&params)
            .send()
            .context("failed to poll token endpoint")?;

        if resp.status().is_success() {
            let data: TokenResponse = resp.json().context("failed to parse token response")?;
            let refresh_token = data
                .refresh_token
                .context("refresh token missing; ensure offline_access scope")?;
            let expires_at = now_epoch_secs() + data.expires_in;
            return Ok(OAuthToken {
                access_token: data.access_token,
                refresh_token,
                expires_at,
            });
        }

        let err_body: Value = resp.json().unwrap_or(Value::Null);
        let error = err_body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_error");

        match error {
            "authorization_pending" => {}
            "slow_down" => interval += 5,
            "authorization_declined" => bail!("authorization was declined"),
            "expired_token" => bail!("device code expired; please try again"),
            _ => {
                let desc = err_body
                    .get("error_description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                bail!("token polling failed: {error}: {desc}");
            }
        }

        sleep(Duration::from_secs(interval));
    }
}

pub fn refresh_msa_token(
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
) -> Result<OAuthToken> {
    let client = Client::new();
    let mut params = vec![
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token),
    ];
    if client_id == DEFAULT_MS_CLIENT_ID {
        params.push(("redirect_uri", MS_BROWSER_REDIRECT_URL));
        params.push(("scope", MS_BROWSER_SCOPE));
    }
    if let Some(secret) = client_secret {
        params.push(("client_secret", secret));
    }

    let resp = client
        .post(if client_id == DEFAULT_MS_CLIENT_ID {
            MS_LIVE_TOKEN_URL
        } else {
            MS_TOKEN_URL
        })
        .form(&params)
        .send()
        .context("failed to refresh token")?;

    if !resp.status().is_success() {
        return Err(format_oauth_error("refresh failed", resp));
    }

    let data: TokenResponse = resp.json().context("failed to parse refresh response")?;
    let refresh_token = data
        .refresh_token
        .unwrap_or_else(|| refresh_token.to_string());
    let expires_at = now_epoch_secs() + data.expires_in;

    Ok(OAuthToken {
        access_token: data.access_token,
        refresh_token,
        expires_at,
    })
}

pub fn exchange_for_minecraft(ms_access_token: &str) -> Result<MinecraftAuth> {
    let (xbl_token, user_hash, xuid) = xbox_live_auth(ms_access_token)?;
    let (xsts_token, xsts_uhs, xsts_xuid) = xsts_auth(&xbl_token)?;
    let uhs = if !xsts_uhs.is_empty() {
        xsts_uhs
    } else {
        user_hash
    };
    let xuid = xsts_xuid.or(xuid);

    let mc_token = minecraft_login(&xsts_token, &uhs)?;
    let profile = minecraft_profile(&mc_token.access_token)?;

    Ok(MinecraftAuth {
        access_token: mc_token.access_token,
        expires_at: mc_token.expires_at,
        uuid: profile.id,
        username: profile.name,
        xuid,
    })
}

fn xbox_live_auth(ms_access_token: &str) -> Result<(String, String, Option<String>)> {
    let client = Client::new();
    let body = XblRequest {
        properties: XblProperties {
            auth_method: "RPS",
            site_name: "user.auth.xboxlive.com",
            rps_ticket: format!("d={ms_access_token}"),
        },
        relying_party: "http://auth.xboxlive.com",
        token_type: "JWT",
    };

    let resp = client
        .post(XBL_AUTH_URL)
        .json(&body)
        .send()
        .context("failed xbox live auth request")?;

    if !resp.status().is_success() {
        return Err(format_xbox_error("xbox live auth failed", resp));
    }

    let data: XblResponse = resp.json().context("failed to parse xbox live response")?;
    let xui = data
        .display_claims
        .xui
        .into_iter()
        .next()
        .context("missing xbox user hash")?;
    let xuid = xui.xuid.or(xui.xid);
    Ok((data.token, xui.uhs, xuid))
}

fn xsts_auth(xbl_token: &str) -> Result<(String, String, Option<String>)> {
    let client = Client::new();
    let body = XstsRequest {
        properties: XstsProperties {
            sandbox_id: "RETAIL",
            user_tokens: vec![xbl_token],
        },
        relying_party: "rp://api.minecraftservices.com/",
        token_type: "JWT",
    };

    let resp = client
        .post(XSTS_AUTH_URL)
        .json(&body)
        .send()
        .context("failed xsts auth request")?;

    if !resp.status().is_success() {
        return Err(format_xsts_error("xsts auth failed", resp));
    }

    let data: XblResponse = resp.json().context("failed to parse xsts response")?;
    let xui = data
        .display_claims
        .xui
        .into_iter()
        .next()
        .context("missing xsts user hash")?;
    let xuid = xui.xuid.or(xui.xid);
    Ok((data.token, xui.uhs, xuid))
}

fn minecraft_login(xsts_token: &str, user_hash: &str) -> Result<MinecraftToken> {
    let client = Client::new();
    let identity_token = format!("XBL3.0 x={user_hash};{xsts_token}");
    let body = McLoginRequest {
        identity_token,
        ensure_legacy_enabled: None,
        platform: None,
    };

    let resp = client
        .post(MC_LOGIN_URL)
        .json(&body)
        .send()
        .context("failed minecraft login request")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp
            .text()
            .unwrap_or_else(|_| "<failed to read body>".to_string());
        return Err(anyhow::anyhow!("minecraft login failed: {status} {body}"));
    }

    let data: McLoginResponse = resp.json().context("failed to parse minecraft login")?;
    let expires_in = data.expires_in.unwrap_or(24 * 60 * 60);
    Ok(MinecraftToken {
        access_token: data.access_token,
        expires_at: now_epoch_secs() + expires_in,
    })
}

fn minecraft_profile(access_token: &str) -> Result<McProfile> {
    let client = Client::new();
    let resp = client
        .get(MC_PROFILE_URL)
        .bearer_auth(access_token)
        .send()
        .context("failed minecraft profile request")?
        .error_for_status()
        .context("minecraft profile request failed (does the account own Minecraft?)")?;
    let profile: McProfile = resp.json().context("failed to parse minecraft profile")?;
    Ok(profile)
}

struct MinecraftToken {
    access_token: String,
    expires_at: u64,
}

fn format_oauth_error(prefix: &str, resp: reqwest::blocking::Response) -> anyhow::Error {
    let status = resp.status();
    let body = resp.json::<Value>().unwrap_or(Value::Null);
    let error = body
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown_error");
    let desc = body
        .get("error_description")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error");
    anyhow::anyhow!("{prefix}: {status} {error}: {desc}")
}

fn format_xbox_error(prefix: &str, resp: reqwest::blocking::Response) -> anyhow::Error {
    let status = resp.status();
    let body = resp.json::<Value>().unwrap_or(Value::Null);
    let message = body
        .get("Message")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error");
    anyhow::anyhow!("{prefix}: {status} {message}")
}

fn format_xsts_error(prefix: &str, resp: reqwest::blocking::Response) -> anyhow::Error {
    let status = resp.status();
    let body = resp.json::<Value>().unwrap_or(Value::Null);
    let message = body
        .get("Message")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error");
    let xerr = body.get("XErr").and_then(|v| v.as_i64());
    let hint = match xerr {
        Some(2148916233) => Some(
            "This account has no Xbox Live account. Sign in at https://xbox.com and accept terms, then retry.",
        ),
        Some(2148916235) => Some(
            "This account is in a family or underage. Update Xbox privacy/family settings, then retry.",
        ),
        Some(2148916236) => {
            Some("This account is blocked by region. Check account region settings, then retry.")
        }
        _ => None,
    };

    if let Some(hint) = hint {
        anyhow::anyhow!("{prefix}: {status} {message} (XErr={xerr:?}). {hint}")
    } else {
        anyhow::anyhow!("{prefix}: {status} {message} (XErr={xerr:?})")
    }
}
