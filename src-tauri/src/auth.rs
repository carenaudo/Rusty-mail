//! Native-browser OAuth. Provider passwords and tokens never enter the webview.
use crate::Store;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;

#[derive(Default)]
pub struct AuthState {
    active: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
}
struct Guard(Arc<AtomicBool>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
#[derive(Serialize)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub email: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Google,
    Microsoft,
}
impl Provider {
    fn config(
        &self,
    ) -> (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ) {
        match self {
            Self::Google => (
                "google",
                "RUSTYMAIL_GOOGLE_CLIENT_ID",
                "https://accounts.google.com/o/oauth2/v2/auth",
                "https://oauth2.googleapis.com/token",
                "https://www.googleapis.com/auth/gmail.modify",
            ),
            Self::Microsoft => (
                "microsoft",
                "RUSTYMAIL_MICROSOFT_CLIENT_ID",
                "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
                "https://login.microsoftonline.com/common/oauth2/v2.0/token",
                "offline_access User.Read Mail.ReadWrite Mail.Send",
            ),
        }
    }
}
fn random() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Accept only a single-use code for this exact loopback callback and CSRF state.
fn callback(request: &str, authority: &str, state: &str) -> Result<String, String> {
    if request.len() > 8192 || !request.ends_with("\r\n\r\n") {
        return Err("Incomplete callback request".into());
    }
    let mut lines = request.split("\r\n");
    let parts: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
    if parts.len() != 3 || parts[0] != "GET" || parts[2] != "HTTP/1.1" {
        return Err("Invalid callback request".into());
    }
    let hosts: Vec<_> = lines
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| k.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.trim())
        .collect();
    if hosts != [authority] {
        return Err("Invalid callback host".into());
    }
    if !parts[1].starts_with("/oauth/callback?") {
        return Err("Invalid callback path".into());
    }
    let url = reqwest::Url::parse(&format!("http://{authority}{}", parts[1]))
        .map_err(|_| "Invalid callback")?;
    if url.path() != "/oauth/callback" {
        return Err("Invalid callback path".into());
    }
    let params: Vec<_> = url.query_pairs().collect();
    let states: Vec<_> = params.iter().filter(|(k, _)| k == "state").collect();
    if states.len() != 1 || !bool::from(states[0].1.as_bytes().ct_eq(state.as_bytes())) {
        return Err("Invalid sign-in state".into());
    }
    if params.iter().any(|(k, _)| k == "error") {
        return Err("Sign-in was declined by the provider".into());
    }
    let codes: Vec<_> = params.iter().filter(|(k, _)| k == "code").collect();
    if codes.len() != 1 || codes[0].1.is_empty() {
        return Err("Missing or duplicate authorization code".into());
    }
    Ok(codes[0].1.to_string())
}
fn wait_for_code(
    listener: TcpListener,
    authority: String,
    state: String,
    cancelled: Arc<AtomicBool>,
) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|_| "Could not initialize callback listener")?;
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        if cancelled.load(Ordering::SeqCst) {
            return Err("Sign-in cancelled".into());
        }
        match listener.accept() {
            Ok((mut stream, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 1024];
                let read_deadline = Instant::now() + Duration::from_secs(2);
                while bytes.len() < 8192
                    && Instant::now() < read_deadline
                    && !bytes.windows(4).any(|w| w == b"\r\n\r\n")
                {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                    }
                }
                let result = std::str::from_utf8(&bytes)
                    .map_err(|_| "Invalid callback encoding".to_string())
                    .and_then(|r| callback(r, &authority, &state));
                let declined =
                    matches!(&result, Err(e) if e == "Sign-in was declined by the provider");
                let code = result.ok();
                let (status, text) = if code.is_some() {
                    (
                        "200 OK",
                        "Authorization received. Return to Rusty Mail to finish sign-in.",
                    )
                } else {
                    (
                        "400 Bad Request",
                        "Invalid sign-in response. Return to Rusty Mail or try again.",
                    )
                };
                let response=format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{text}",text.len());
                let _ = stream.write_all(response.as_bytes());
                if let Some(code) = code {
                    return Ok(code);
                }
                if declined {
                    return Err("Sign-in was declined by the provider".into());
                }
                // Invalid traffic cannot consume the pending authorization attempt.
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(_) => return Err("Sign-in listener failed".into()),
        }
    }
    Err("Sign-in timed out. Please try again.".into())
}
async fn bounded_json(mut response: reqwest::Response) -> Result<serde_json::Value, String> {
    if !response.status().is_success() {
        return Err(format!(
            "Provider returned HTTP {}. Check the app registration and permissions.",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Provider response failed")?
    {
        if bytes.len() + chunk.len() > 65536 {
            return Err("Provider response too large".into());
        }
        bytes.extend_from_slice(&chunk)
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid provider response".into())
}
#[tauri::command]
pub fn cancel_sign_in(auth: tauri::State<AuthState>) {
    auth.cancelled.store(true, Ordering::SeqCst);
}
#[tauri::command]
pub fn list_accounts(store: tauri::State<Store>) -> Result<Vec<Account>, String> {
    let db = store.0.lock().map_err(|_| "Storage unavailable")?;
    let mut query = db
        .prepare("SELECT id,provider,email FROM accounts ORDER BY email")
        .map_err(|_| "Account storage unavailable")?;
    let result = query
        .query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                provider: r.get(1)?,
                email: r.get(2)?,
            })
        })
        .map_err(|_| "Cannot read accounts")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Cannot read accounts".into());
    result
}
#[tauri::command]
pub async fn sign_in(
    provider: Provider,
    auth: tauri::State<'_, AuthState>,
    store: tauri::State<'_, Store>,
) -> Result<Account, String> {
    if auth.active.swap(true, Ordering::SeqCst) {
        return Err("A sign-in is already in progress".into());
    }
    let _guard = Guard(auth.active.clone());
    auth.cancelled.store(false, Ordering::SeqCst);
    let (name, env, authorize, token, scopes) = provider.config();
    let client_id=std::env::var(env).ok().filter(|s|!s.trim().is_empty()).ok_or_else(||format!("Set {env} using a registered desktop/public OAuth application. No password is required here."))?;
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "Cannot open local sign-in callback")?;
    let port = listener
        .local_addr()
        .map_err(|_| "Callback unavailable")?
        .port();
    let host = if name == "microsoft" {
        "localhost"
    } else {
        "127.0.0.1"
    };
    let authority = format!("{host}:{port}");
    let redirect = format!("http://{authority}/oauth/callback");
    let state = random();
    let verifier = random();
    let mut url = reqwest::Url::parse(authorize).map_err(|_| "Invalid provider")?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", client_id.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("response_type", "code"),
        ("scope", scopes),
        ("state", state.as_str()),
        ("code_challenge", challenge(&verifier).as_str()),
        ("code_challenge_method", "S256"),
    ]);
    if name == "google" {
        url.query_pairs_mut().extend_pairs([
            ("access_type", "offline"),
            ("prompt", "consent select_account"),
        ]);
    } else {
        url.query_pairs_mut()
            .append_pair("prompt", "select_account");
    }
    open::that(url.as_str()).map_err(|_| "Could not open the system browser")?;
    let cancelled = auth.cancelled.clone();
    let code = tauri::async_runtime::spawn_blocking(move || {
        wait_for_code(listener, authority, state, cancelled)
    })
    .await
    .map_err(|_| "Sign-in listener stopped")??;
    if auth.cancelled.load(Ordering::SeqCst) {
        return Err("Sign-in cancelled".into());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Cannot initialize secure connection")?;
    let mut form = vec![
        ("client_id", client_id),
        ("grant_type", "authorization_code".into()),
        ("code", code),
        ("redirect_uri", redirect),
        ("code_verifier", verifier),
    ];
    // Google desktop credentials may include this public-client value. It is not a confidential secret.
    if name == "google" {
        if let Ok(value) = std::env::var("RUSTYMAIL_GOOGLE_CLIENT_SECRET") {
            form.push(("client_secret", value));
        }
    }
    let tokens = bounded_json(
        client
            .post(token)
            .form(&form)
            .send()
            .await
            .map_err(|_| "Secure token exchange failed")?,
    )
    .await?;
    let access = tokens["access_token"]
        .as_str()
        .ok_or("Provider returned no access token")?;
    if tokens["refresh_token"]
        .as_str()
        .filter(|v| !v.is_empty())
        .is_none()
    {
        return Err(
            "Provider returned no offline authorization. Please reconnect and approve access."
                .into(),
        );
    }
    let profile_url = if name == "google" {
        "https://gmail.googleapis.com/gmail/v1/users/me/profile"
    } else {
        "https://graph.microsoft.com/v1.0/me?$select=id,mail,userPrincipalName"
    };
    let profile = bounded_json(
        client
            .get(profile_url)
            .bearer_auth(access)
            .send()
            .await
            .map_err(|_| "Could not identify the mailbox")?,
    )
    .await?;
    let email = if name == "google" {
        profile["emailAddress"].as_str()
    } else {
        profile["mail"]
            .as_str()
            .or(profile["userPrincipalName"].as_str())
    }
    .filter(|v| !v.is_empty())
    .ok_or("Provider returned no mailbox address")?
    .to_string();
    let identity = if name == "google" {
        email.as_str()
    } else {
        profile["id"].as_str().ok_or("Missing account identity")?
    };
    let id = format!(
        "{name}:{}",
        hex::encode(Sha256::digest(identity.as_bytes()))
    );
    if auth.cancelled.load(Ordering::SeqCst) {
        return Err("Sign-in cancelled".into());
    }
    keyring::Entry::new("dev.rustymail.oauth", &id)
        .map_err(|_| "Credential storage unavailable")?
        .set_password(&tokens.to_string())
        .map_err(|_| "Could not securely store authorization")?;
    store
        .0
        .lock()
        .map_err(|_| "Storage unavailable")?
        .execute(
            "INSERT OR REPLACE INTO accounts(id,provider,email) VALUES(?1,?2,?3)",
            rusqlite::params![id, name, email],
        )
        .map_err(|_| "Could not save account")?;
    Ok(Account {
        id,
        provider: name.into(),
        email,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pkce_rfc7636() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        )
    }
    #[test]
    fn callback_constraints() {
        let good =
            "GET /oauth/callback?code=test&state=expected HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n";
        assert_eq!(
            callback(good, "127.0.0.1:1234", "expected").unwrap(),
            "test"
        );
        assert!(callback(good, "127.0.0.1:9999", "expected").is_err());
        assert!(callback(good, "127.0.0.1:1234", "other").is_err());
        assert!(callback(
            &good.replace("state=expected", "state=expected&state=expected"),
            "127.0.0.1:1234",
            "expected"
        )
        .is_err());
        assert!(callback(
            &good.replace("/oauth/callback", "//evil.example/oauth/callback"),
            "127.0.0.1:1234",
            "expected"
        )
        .is_err());
    }
}
