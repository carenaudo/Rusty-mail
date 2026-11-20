//! Direct provider transports. All remote data remains untrusted.
use crate::{
    engine::{self, Mail, ServerConfig},
    Store,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use futures_util::TryStreamExt;
use reqwest::{Client, Method};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::{sync::OnceLock, time::Duration};
pub fn segment(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}
pub fn db<T>(
    store: &Store,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, String>,
) -> Result<T, String> {
    let db = store.0.lock().map_err(|_| "Storage unavailable")?;
    f(&db)
}
pub fn account(store: &Store, id: &str) -> Result<(String, String), String> {
    db(store, |db| {
        db.query_row(
            "SELECT provider,email FROM accounts WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "Account not found".into())
    })
}
pub fn client() -> Result<Client, String> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Cannot initialize HTTPS".to_string())?;
    Ok(CLIENT.get_or_init(|| client).clone())
}
pub async fn bytes(mut r: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if !r.status().is_success() {
        return Err(format!("Provider HTTP {}", r.status().as_u16()));
    }
    let mut out = Vec::new();
    while let Some(chunk) = r
        .chunk()
        .await
        .map_err(|_| "Provider download interrupted")?
    {
        if out.len() + chunk.len() > limit {
            return Err("Provider response exceeds download limit".into());
        }
        out.extend_from_slice(&chunk)
    }
    Ok(out)
}
pub async fn json_response(r: reqwest::Response) -> Result<Value, String> {
    serde_json::from_slice(&bytes(r, 36 * 1024 * 1024).await?)
        .map_err(|_| "Invalid provider response".into())
}
pub fn approved_url(provider: &str, url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && u.port_or_known_default() == Some(443)
            && match provider {
                "google" => {
                    u.host_str() == Some("gmail.googleapis.com")
                        && u.path().starts_with("/gmail/v1/users/me/")
                }
                "microsoft" => {
                    u.host_str() == Some("graph.microsoft.com") && u.path().starts_with("/v1.0/me/")
                }
                _ => false,
            }
    })
}
pub async fn token(store: &Store, id: &str, force: bool) -> Result<String, String> {
    let (provider, _) = account(store, id)?;
    let entry = keyring::Entry::new("dev.rustymail.oauth", id)
        .map_err(|_| "Credential store unavailable")?;
    let raw = entry
        .get_password()
        .map_err(|_| "Reconnect this account to authorize mail access")?;
    let old: Value = serde_json::from_str(&raw).map_err(|_| "Reconnect this account")?;
    if !force && old["_expires_at"].as_i64().unwrap_or(0) > engine::now() + 120 {
        if let Some(access) = old["access_token"].as_str() {
            return Ok(access.into());
        }
    }
    let (env, url) = match provider.as_str() {
        "google" => (
            "RUSTYMAIL_GOOGLE_CLIENT_ID",
            "https://oauth2.googleapis.com/token",
        ),
        "microsoft" => (
            "RUSTYMAIL_MICROSOFT_CLIENT_ID",
            "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        ),
        _ => return Err("Not an OAuth account".into()),
    };
    let refresh = old["refresh_token"]
        .as_str()
        .ok_or("Reconnect account: missing refresh authorization")?;
    let mut form = vec![
        (
            "client_id",
            std::env::var(env).map_err(|_| format!("Set {env}"))?,
        ),
        ("grant_type", "refresh_token".into()),
        ("refresh_token", refresh.into()),
    ];
    if provider == "google" {
        if let Ok(value) = std::env::var("RUSTYMAIL_GOOGLE_CLIENT_SECRET") {
            form.push(("client_secret", value))
        }
    }
    let mut next = json_response(
        client()?
            .post(url)
            .form(&form)
            .send()
            .await
            .map_err(|_| "Token renewal failed; check connectivity")?,
    )
    .await
    .map_err(|_| "Account authorization expired or was revoked. Reconnect this account.")?;
    if next["refresh_token"].as_str().is_none() {
        next["refresh_token"] = old["refresh_token"].clone()
    }
    next["_expires_at"] = json!(engine::now() + next["expires_in"].as_i64().unwrap_or(3600));
    let access = next["access_token"]
        .as_str()
        .ok_or("Reconnect account: no access token")?
        .to_string();
    entry
        .set_password(&next.to_string())
        .map_err(|_| "Cannot securely store renewed authorization")?;
    Ok(access)
}
pub async fn api(
    store: &Store,
    id: &str,
    method: Method,
    url: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let (provider, _) = account(store, id)?;
    if !approved_url(&provider, url) {
        return Err("Blocked unexpected provider URL".into());
    }
    let mut access = token(store, id, false).await?;
    for attempt in 0..2 {
        let mut request = client()?.request(method.clone(), url).bearer_auth(&access);
        if provider == "microsoft" {
            request = request.header(
                "Prefer",
                "IdType=\"ImmutableId\", outlook.body-content-type=\"text\"",
            )
        }
        if let Some(b) = body.as_ref() {
            request = request.json(b)
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Mail provider connection failed")?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && method == Method::GET
            && attempt == 0
        {
            access = token(store, id, true).await?;
            continue;
        }
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        return json_response(response).await;
    }
    Err("Reconnect account".into())
}
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
fn header(v: &Value, name: &str) -> String {
    v["payload"]["headers"]
        .as_array()
        .and_then(|a| {
            a.iter().find(|h| {
                h["name"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(name))
            })
        })
        .and_then(|h| h["value"].as_str())
        .unwrap_or("")
        .to_string()
}
pub fn gmail_mail(account_id: &str, email: &str, v: &Value) -> Result<Mail, String> {
    let remote = v["id"].as_str().ok_or("Message has no remote identity")?;
    let labels = strings(&v["labelIds"]);
    let from = header(v, "From");
    let mut address = from.clone();
    let mut sender = from.clone();
    if let Some((name, tail)) = from.split_once('<') {
        sender = name.trim().trim_matches('"').into();
        address = tail.trim_end_matches('>').into()
    }
    let folder = if labels.iter().any(|s| s == "TRASH") {
        "trash"
    } else if labels.iter().any(|s| s == "SPAM") {
        "spam"
    } else if labels.iter().any(|s| s == "INBOX") {
        "inbox"
    } else if labels.iter().any(|s| s == "DRAFT") {
        "drafts"
    } else if labels.iter().any(|s| s == "SENT") {
        "sent"
    } else {
        "archive"
    };
    let received = v["internalDate"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
        / 1000;
    Ok(Mail {
        id: engine::identity(account_id, remote),
        account_id: account_id.into(),
        remote_id: remote.into(),
        provider: "google".into(),
        thread_id: v["threadId"].as_str().unwrap_or("").into(),
        account: email.into(),
        sender,
        address,
        subject: header(v, "Subject"),
        body: v["snippet"].as_str().unwrap_or("").into(),
        category: if !header(v, "List-Id").is_empty() {
            "newsletters"
        } else {
            "people"
        }
        .into(),
        unread: labels.iter().any(|s| s == "UNREAD"),
        starred: labels.iter().any(|s| s == "STARRED"),
        folder: folder.into(),
        date: chrono::DateTime::from_timestamp(received, 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default(),
        received,
        labels,
        ..Default::default()
    })
}
pub async fn sync_google(store: &Store, id: &str) -> Result<usize, String> {
    let count = sync_google_messages(store, id).await?;
    let root = "https://gmail.googleapis.com/gmail/v1/users/me";
    if catalog_due(store, id)? {
        let response = api(store, id, Method::GET, &format!("{root}/labels"), None).await?;
        let rows = response["labels"]
            .as_array()
            .ok_or("Gmail omitted its label list")?;
        let mut labels = Vec::new();
        for item in rows.iter().filter(|item| item["type"] == "user") {
            labels.push(catalog_entry(item, "name")?);
        }
        save_catalog(store, id, "label", &labels)?;
    }
    Ok(count)
}

async fn sync_google_messages(store: &Store, id: &str) -> Result<usize, String> {
    let (_, email) = account(store, id)?;
    let root = "https://gmail.googleapis.com/gmail/v1/users/me";
    let mut history = db(store, |d| engine::state(d, id, "history"))?.filter(|s| !s.is_empty());
    if let Some(h) = history.clone() {
        let mut url = format!(
            "{root}/history?startHistoryId={}&maxResults=100",
            segment(&h)
        );
        let mut changed = std::collections::BTreeSet::new();
        let mut removed = std::collections::BTreeSet::new();
        let mut latest = h;
        loop {
            let value = match api(store, id, Method::GET, &url, None).await {
                Ok(v) => v,
                Err(e) if e == "Provider HTTP 404" => {
                    db(store, |d| engine::checkpoint(d, id, "history", ""))?;
                    history = None;
                    break;
                }
                Err(e) => return Err(e),
            };
            if let Some(records) = value["history"].as_array() {
                for record in records {
                    for field in ["messagesAdded", "labelsAdded", "labelsRemoved"] {
                        if let Some(entries) = record[field].as_array() {
                            for item in entries {
                                if let Some(remote) = item["message"]["id"].as_str() {
                                    changed.insert(remote.to_string());
                                }
                            }
                        }
                    }
                    if let Some(entries) = record["messagesDeleted"].as_array() {
                        for item in entries {
                            if let Some(remote) = item["message"]["id"].as_str() {
                                removed.insert(remote.to_string());
                            }
                        }
                    }
                }
            }
            latest = value["historyId"].as_str().unwrap_or(&latest).to_string();
            if let Some(page) = value["nextPageToken"].as_str() {
                url = format!(
                    "{root}/history?startHistoryId={}&maxResults=100&pageToken={}",
                    segment(&hclone(&history)),
                    segment(page)
                );
            } else {
                break;
            }
        }
        if history.is_some() {
            let mut mails = Vec::new();
            let ids: Vec<_> = changed.difference(&removed).cloned().collect();
            for remote in &ids {
                match api(
                    store,
                    id,
                    Method::GET,
                    &format!("{root}/messages/{}?format=metadata", segment(remote)),
                    None,
                )
                .await
                {
                    Ok(v) => mails.push(gmail_mail(id, &email, &v)?),
                    Err(e) if e == "Provider HTTP 404" => {
                        removed.insert(remote.clone());
                    }
                    Err(e) => return Err(e),
                }
            }
            let count = mails.len();
            db(store, |d| {
                let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
                for m in &mails {
                    put_remote(&tx, m)?;
                }
                for remote in &removed {
                    engine::remove(&tx, &engine::identity(id, remote))?
                }
                engine::checkpoint(&tx, id, "history", &latest)?;
                tx.commit().map_err(|e| e.to_string())
            })?;
            return Ok(count);
        }
    }
    let page = db(store, |d| engine::state(d, id, "page"))?.unwrap_or_default();
    if page.is_empty() {
        let profile = api(store, id, Method::GET, &format!("{root}/profile"), None).await?;
        db(store, |d| {
            engine::checkpoint(
                d,
                id,
                "bootstrap_history",
                profile["historyId"].as_str().unwrap_or(""),
            )
        })?;
    }
    let value = api(
        store,
        id,
        Method::GET,
        &format!(
            "{root}/messages?maxResults=100&includeSpamTrash=true&pageToken={}",
            segment(&page)
        ),
        None,
    )
    .await?;
    let mut mails = Vec::new();
    if let Some(items) = value["messages"].as_array() {
        for item in items {
            let remote = item["id"].as_str().ok_or("Invalid Gmail identity")?;
            let v = api(
                store,
                id,
                Method::GET,
                &format!("{root}/messages/{}?format=metadata", segment(remote)),
                None,
            )
            .await?;
            mails.push(gmail_mail(id, &email, &v)?);
        }
    }
    db(store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        for m in &mails {
            put_remote(&tx, m)?
        }
        if let Some(page) = value["nextPageToken"].as_str() {
            engine::checkpoint(&tx, id, "page", page)?;
        } else {
            let h = engine::state(&tx, id, "bootstrap_history")?.unwrap_or_default();
            engine::checkpoint(&tx, id, "history", &h)?;
            engine::checkpoint(&tx, id, "page", "")?;
        }
        tx.commit().map_err(|e| e.to_string())
    })?;
    Ok(mails.len())
}
fn hclone(h: &Option<String>) -> String {
    h.clone().unwrap_or_default()
}
pub fn graph_mail(account_id: &str, email: &str, folder: &str, v: &Value) -> Result<Mail, String> {
    let remote = v["id"].as_str().ok_or("Missing Graph identity")?;
    let received = v["receivedDateTime"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp())
        .unwrap_or(0);
    Ok(Mail {
        id: engine::identity(account_id, remote),
        account_id: account_id.into(),
        remote_id: remote.into(),
        provider: "microsoft".into(),
        thread_id: v["conversationId"].as_str().unwrap_or("").into(),
        account: email.into(),
        sender: v["from"]["emailAddress"]["name"]
            .as_str()
            .unwrap_or("")
            .into(),
        address: v["from"]["emailAddress"]["address"]
            .as_str()
            .unwrap_or("")
            .into(),
        subject: v["subject"].as_str().unwrap_or("(No subject)").into(),
        body: v["bodyPreview"].as_str().unwrap_or("").into(),
        category: "people".into(),
        unread: !v["isRead"].as_bool().unwrap_or(true),
        starred: v["flag"]["flagStatus"].as_str() == Some("flagged"),
        folder: folder.into(),
        date: v["receivedDateTime"].as_str().unwrap_or("").into(),
        received,
        labels: vec![folder.strip_prefix("mailbox:").unwrap_or(folder).into()],
        ..Default::default()
    })
}
const GRAPH_FOLDERS: [(&str, &str); 6] = [
    ("inbox", "inbox"),
    ("sentitems", "sent"),
    ("drafts", "drafts"),
    ("archive", "archive"),
    ("deleteditems", "trash"),
    ("junkemail", "spam"),
];

fn catalog_due(store: &Store, account: &str) -> Result<bool, String> {
    db(store, |db| {
        let previous = engine::state(db, account, "folder-catalog")?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        Ok(engine::now().saturating_sub(previous) >= 300)
    })
}

fn catalog_entry(item: &Value, name: &str) -> Result<(String, String), String> {
    let id = item["id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control))
        .ok_or("Invalid provider folder ID")?;
    let name: String = item[name]
        .as_str()
        .unwrap_or(id)
        .chars()
        .take(240)
        .collect();
    Ok((id.into(), name))
}

fn save_catalog(
    store: &Store,
    account: &str,
    special: &str,
    entries: &[(String, String)],
) -> Result<(), String> {
    db(store, |db| {
        let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
        if special == "custom" {
            let retained: std::collections::HashSet<_> =
                entries.iter().map(|(id, _)| id.as_str()).collect();
            let existing = catalog_rows(&tx, account, special)?;
            for (id, _) in existing {
                if !retained.contains(id.as_str()) {
                    let folder = format!("mailbox:{id}");
                    let mut query = tx
                        .prepare("SELECT id FROM mail WHERE account_id=?1 AND folder=?2")
                        .map_err(|e| e.to_string())?;
                    let messages = query
                        .query_map(params![account, folder], |row| row.get::<_, String>(0))
                        .map_err(|e| e.to_string())?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| e.to_string())?;
                    for message in messages {
                        engine::remove(&tx, &message)?;
                    }
                    tx.execute(
                        "DELETE FROM sync_state WHERE account_id=?1 AND key=?2",
                        params![account, format!("delta:custom:{id}")],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
        tx.execute(
            "DELETE FROM mailboxes WHERE account_id=?1 AND special=?2",
            params![account, special],
        )
        .map_err(|e| e.to_string())?;
        for (id, name) in entries {
            tx.execute(
                "INSERT INTO mailboxes VALUES(?1,?2,?3,?4)",
                params![account, id, name, special],
            )
            .map_err(|e| e.to_string())?;
        }
        engine::checkpoint(&tx, account, "folder-catalog", &engine::now().to_string())?;
        tx.commit().map_err(|e| e.to_string())
    })
}

fn catalog_rows(
    db: &rusqlite::Connection,
    account: &str,
    special: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut query = db
        .prepare("SELECT id,name FROM mailboxes WHERE account_id=?1 AND special=?2 ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map(params![account, special], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

async fn refresh_graph_catalog(store: &Store, account: &str, root: &str) -> Result<(), String> {
    let mut standard = std::collections::HashSet::new();
    for (remote, _) in GRAPH_FOLDERS {
        match api(
            store,
            account,
            Method::GET,
            &format!("{root}/mailFolders/{remote}?$select=id"),
            None,
        )
        .await
        {
            Ok(folder) => {
                standard.insert(catalog_entry(&folder, "id")?.0);
            }
            Err(error) if error == "Provider HTTP 404" => {}
            Err(error) => return Err(error),
        }
    }
    let fields = "$select=id,displayName,childFolderCount&$top=100";
    let mut pending = vec![(format!("{root}/mailFolders?{fields}"), String::new())];
    let mut visited = std::collections::HashSet::new();
    let mut folders = Vec::new();
    while let Some((url, parent)) = pending.pop() {
        if visited.len() >= 10_000 || !visited.insert(url.clone()) {
            return Err("Invalid or oversized Outlook folder hierarchy".into());
        }
        let response = api(store, account, Method::GET, &url, None).await?;
        for item in response["value"]
            .as_array()
            .ok_or("Outlook omitted its folder list")?
        {
            let (id, name) = catalog_entry(item, "displayName")?;
            if item["@odata.type"].as_str() == Some("#microsoft.graph.mailSearchFolder") {
                continue;
            }
            let path = if parent.is_empty() {
                name
            } else {
                format!("{parent} / {name}")
            };
            if item["childFolderCount"].as_u64().unwrap_or(0) > 0 {
                pending.push((
                    format!("{root}/mailFolders/{}/childFolders?{fields}", segment(&id)),
                    path.clone(),
                ));
            }
            if !standard.contains(&id) {
                folders.push((id, path));
            }
        }
        if let Some(next) = response["@odata.nextLink"].as_str() {
            pending.push((next.into(), parent));
        }
    }
    save_catalog(store, account, "custom", &folders)
}

pub async fn sync_graph(store: &Store, id: &str) -> Result<usize, String> {
    let (_, email) = account(store, id)?;
    let root = "https://graph.microsoft.com/v1.0/me";
    let mut count = 0;
    let discovery = if catalog_due(store, id)? {
        refresh_graph_catalog(store, id, root).await
    } else {
        Ok(())
    };
    let mut folders: Vec<_> = GRAPH_FOLDERS
        .into_iter()
        .map(|(remote, folder)| {
            (
                remote.to_owned(),
                folder.to_owned(),
                format!("delta:{remote}"),
            )
        })
        .collect();
    folders.extend(
        db(store, |db| catalog_rows(db, id, "custom"))?
            .into_iter()
            .map(|(id, _)| {
                (
                    segment(&id),
                    format!("mailbox:{id}"),
                    format!("delta:custom:{id}"),
                )
            }),
    );
    for (remote, folder, key) in folders {
        let initial=format!("{root}/mailFolders/{remote}/messages/delta?$select=id,conversationId,subject,from,receivedDateTime,isRead,flag,bodyPreview,parentFolderId&$top=100");
        let saved = db(store, |d| engine::state(d, id, &key))?.unwrap_or_else(|| initial.clone());
        let v = match api(store, id, Method::GET, &saved, None).await {
            Ok(v) => v,
            Err(e) if e == "Provider HTTP 410" || e == "Provider HTTP 404" => {
                match api(store, id, Method::GET, &initial, None).await {
                    Err(error) if error == "Provider HTTP 404" => continue,
                    result => result?,
                }
            }
            Err(e) => return Err(e),
        };
        db(store, |d| {
            let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
            if let Some(items) = v["value"].as_array() {
                for item in items {
                    let remote_id = item["id"].as_str().ok_or("Invalid Graph identity")?;
                    if item.get("@removed").is_some() {
                        let local = engine::identity(id, remote_id);
                        if let Ok(old) = engine::get(&tx, &local) {
                            if old.folder == folder {
                                engine::remove(&tx, &local)?;
                            }
                        }
                    } else {
                        put_remote(&tx, &graph_mail(id, &email, &folder, item)?)?;
                        count += 1;
                    }
                }
            }
            let next = v["@odata.nextLink"]
                .as_str()
                .or(v["@odata.deltaLink"].as_str())
                .ok_or("Graph omitted delta checkpoint")?;
            if !approved_url("microsoft", next) {
                return Err("Blocked Graph continuation URL".into());
            }
            engine::checkpoint(&tx, id, &key, next)?;
            tx.commit().map_err(|e| e.to_string())
        })?;
    }
    discovery?;
    Ok(count)
}
pub async fn raw_message(store: &Store, m: &Mail) -> Result<Vec<u8>, String> {
    let (provider, _) = account(store, &m.account_id)?;
    match provider.as_str() {
        "google" => {
            let v = api(
                store,
                &m.account_id,
                Method::GET,
                &format!(
                    "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}?format=raw",
                    segment(&m.remote_id)
                ),
                None,
            )
            .await?;
            engine::decode_url64(v["raw"].as_str().ok_or("No raw message returned")?)
        }
        "microsoft" => {
            let access = token(store, &m.account_id, false).await?;
            let r = client()?
                .get(format!(
                    "https://graph.microsoft.com/v1.0/me/messages/{}/$value",
                    segment(&m.remote_id)
                ))
                .header("Prefer", "IdType=\"ImmutableId\"")
                .bearer_auth(access)
                .send()
                .await
                .map_err(|_| "Message download failed")?;
            bytes(r, engine::MAX_RAW).await
        }
        "imap" => imap_raw(store, m).await,
        _ => Err("Unsupported provider".into()),
    }
}

type Imap = async_imap::Session<tokio_native_tls::TlsStream<tokio::net::TcpStream>>;
pub fn server_config(store: &Store, id: &str) -> Result<ServerConfig, String> {
    db(store, |d| {
        let s: String = d
            .query_row("SELECT config FROM account_config WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .map_err(|_| "Missing server configuration")?;
        serde_json::from_str(&s).map_err(|_| "Invalid server configuration".into())
    })
}
async fn imap_connect(store: &Store, id: &str) -> Result<Imap, String> {
    let c = server_config(store, id)?;
    c.validate()?;
    let password = keyring::Entry::new("dev.rustymail.imap", id)
        .map_err(|_| "Credential store unavailable")?
        .get_password()
        .map_err(|_| "Re-enter this server's mail credential")?;
    connect_credentials(&c, &password).await
}
pub async fn validate_imap_credentials(c: &ServerConfig, password: &str) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(45), async {
        let mut s = connect_credentials(c, password).await?;
        s.noop().await.map_err(|_| "IMAP connection test failed")?;
        s.logout().await.ok();
        Ok(())
    })
    .await
    .map_err(|_| "IMAP validation timed out")?
}
async fn connect_credentials(c: &ServerConfig, password: &str) -> Result<Imap, String> {
    let mut tls = native_tls::TlsConnector::builder();
    if let Some(pem) = &c.ca_pem {
        tls.add_root_certificate(
            native_tls::Certificate::from_pem(pem.as_bytes())
                .map_err(|_| "Invalid CA certificate")?,
        );
    }
    let socket = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::net::TcpStream::connect((c.imap_host.as_str(), c.imap_port)),
    )
    .await
    .map_err(|_| "IMAP connection timed out")?
    .map_err(|_| "Cannot connect to IMAP server")?;
    let stream =
        tokio_native_tls::TlsConnector::from(tls.build().map_err(|_| "Cannot initialize TLS")?)
            .connect(&c.imap_host, socket)
            .await
            .map_err(|_| "IMAP certificate or TLS validation failed")?;
    async_imap::Client::new(stream)
        .login(&c.username, password)
        .await
        .map_err(|_| "IMAP login rejected. Check the server's app password and username.".into())
}
fn put_remote(d: &rusqlite::Connection, m: &Mail) -> Result<(), String> {
    let pending:i64=d.query_row("SELECT count(*) FROM jobs WHERE kind='mutation' AND status IN('pending','running','failed') AND json_extract(payload,'$.id')=?1",[&m.id],|r|r.get(0)).map_err(|e|e.to_string())?;
    if pending == 0 {
        engine::put(d, m)?
    } else if let Ok(mut cached) = engine::get(d, &m.id) {
        // Sync may discover threading metadata while local flags/moves await
        // submission. Preserve those optimistic changes, but keep identity fresh.
        cached.provider = m.provider.clone();
        cached.thread_id = m.thread_id.clone();
        engine::put(d, &cached)?;
    }
    Ok(())
}
pub async fn sync_imap(store: &Store, id: &str) -> Result<usize, String> {
    sync_imap_mailboxes(store, id, None).await
}

pub async fn sync_imap_mailbox(store: &Store, id: &str, target: &str) -> Result<usize, String> {
    sync_imap_mailboxes(store, id, Some(target)).await
}

async fn sync_imap_mailboxes(
    store: &Store,
    id: &str,
    target: Option<&str>,
) -> Result<usize, String> {
    let c = server_config(store, id)?;
    let mut session = imap_connect(store, id).await?;
    let boxes: Vec<_> = session
        .list(None, Some("*"))
        .await
        .map_err(|_| "Cannot list IMAP folders")?
        .try_collect()
        .await
        .map_err(|_| "Cannot read IMAP folders")?;
    let mut count = 0;
    if target.is_none() {
        let listed: std::collections::HashSet<_> = boxes
            .iter()
            .filter(|name| {
                !name
                    .attributes()
                    .contains(&async_imap::types::NameAttribute::NoSelect)
            })
            .map(|name| name.name())
            .collect();
        db(store, |db| {
            let mut query = db
                .prepare("SELECT id FROM mailboxes WHERE account_id=?1")
                .map_err(|error| error.to_string())?;
            let cached = query
                .query_map([id], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            for mailbox in cached {
                if !listed.contains(mailbox.as_str()) {
                    db.execute(
                        "DELETE FROM mailboxes WHERE account_id=?1 AND id=?2",
                        params![id, mailbox],
                    )
                    .map_err(|error| error.to_string())?;
                }
            }
            Ok(())
        })?;
    }
    for name in boxes {
        let mailbox = name.name().to_string();
        if target.is_some_and(|target| target != mailbox)
            || name
                .attributes()
                .contains(&async_imap::types::NameAttribute::NoSelect)
            || mailbox.len() > 1024
            || mailbox.chars().any(char::is_control)
        {
            continue;
        }
        let selected = match session.examine(&mailbox).await {
            Ok(s) => s,
            Err(_) => continue,
        };
        let validity = selected.uid_validity.unwrap_or(0);
        let key = format!("uidvalidity:{mailbox}");
        let previous = db(store, |d| engine::state(d, id, &key))?;
        if previous.as_deref() != Some(&validity.to_string()) {
            db(store, |d| {
                let mut q=d.prepare("SELECT id FROM mail WHERE account_id=?1 AND id IN(SELECT mail_id FROM memberships WHERE mailbox=?2)").map_err(|e|e.to_string())?;
                let ids = q
                    .query_map(params![id, mailbox], |r| r.get::<_, String>(0))
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                for mid in ids {
                    engine::remove(d, &mid)?
                }
                engine::checkpoint(d, id, &key, &validity.to_string())
            })?;
        }
        let folder = if mailbox.eq_ignore_ascii_case("INBOX") {
            "inbox"
        } else {
            let l = mailbox.to_lowercase();
            if l.contains("sent") {
                "sent"
            } else if l.contains("trash") || l.contains("deleted") {
                "trash"
            } else if l.contains("junk") || l.contains("spam") {
                "spam"
            } else if l.contains("draft") {
                "drafts"
            } else {
                "archive"
            }
        };
        db(store, |d| {
            d.execute(
                "INSERT OR REPLACE INTO mailboxes VALUES(?1,?2,?3,?4)",
                params![id, mailbox, mailbox, folder],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })?;
        let all = session
            .uid_search("ALL")
            .await
            .map_err(|_| "IMAP UID search failed")?;
        let mut uids: Vec<_> = all.into_iter().collect();
        uids.sort_unstable();
        let existing = db(store, |d| {
            let mut q=d.prepare("SELECT id,remote_id FROM mail WHERE account_id=?1 AND id IN(SELECT mail_id FROM memberships WHERE mailbox=?2)").map_err(|e|e.to_string())?;
            let rows = q
                .query_map(params![id, mailbox], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            Ok(rows)
        })?;
        let remote_prefix = format!("{}:{validity}:", STANDARD.encode(mailbox.as_bytes()));
        let mut known = std::collections::HashSet::new();
        for (mid, remote) in existing {
            if let Some(uid) = remote
                .strip_prefix(&remote_prefix)
                .and_then(|s| s.parse::<u32>().ok())
            {
                if uids.binary_search(&uid).is_ok() {
                    known.insert(uid);
                } else {
                    db(store, |d| engine::remove(d, &mid))?;
                }
            }
        }
        // Bounded backfill, newest first. Refresh flags for the visible recent window too.
        let wanted: Vec<_> = uids
            .iter()
            .rev()
            .filter(|u| !known.contains(u))
            .take(100)
            .chain(uids.iter().rev().take(100))
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if wanted.is_empty() {
            continue;
        }
        let set = wanted
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let fetched: Vec<_> = session
            .uid_fetch(
                &set,
                "(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE LIST-ID)])",
            )
            .await
            .map_err(|_| "IMAP fetch failed")?
            .try_collect()
            .await
            .map_err(|_| "IMAP fetch interrupted")?;
        db(store, |d| {
            let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
            for f in &fetched {
                let uid = f.uid.ok_or("Missing IMAP UID")?;
                let raw = f.header().unwrap_or(b"\r\n");
                let parsed = mail_parser::MessageParser::default().parse(raw);
                let address = parsed
                    .as_ref()
                    .and_then(|p| p.from())
                    .and_then(|a| a.first())
                    .and_then(|a| a.address())
                    .unwrap_or("")
                    .to_string();
                let sender = parsed
                    .as_ref()
                    .and_then(|p| p.from())
                    .and_then(|a| a.first())
                    .and_then(|a| a.name())
                    .unwrap_or(&address)
                    .to_string();
                let received = parsed
                    .as_ref()
                    .and_then(|p| p.date())
                    .map(|d| d.to_timestamp())
                    .unwrap_or(0);
                let remote = format!("{remote_prefix}{uid}");
                let flags = f.flags().map(|f| format!("{f:?}")).collect::<Vec<_>>();
                let m = Mail {
                    id: engine::identity(id, &remote),
                    account_id: id.into(),
                    remote_id: remote,
                    provider: "imap".into(),
                    account: c.email.clone(),
                    sender,
                    address,
                    subject: parsed
                        .as_ref()
                        .and_then(|p| p.subject())
                        .unwrap_or("(No subject)")
                        .into(),
                    date: chrono::DateTime::from_timestamp(received, 0)
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_default(),
                    received,
                    category: "people".into(),
                    unread: !flags.iter().any(|f| f == "Seen"),
                    starred: flags.iter().any(|f| f == "Flagged"),
                    folder: folder.into(),
                    labels: vec![mailbox.clone()],
                    ..Default::default()
                };
                put_remote(&tx, &m)?;
                count += 1;
            }
            tx.commit().map_err(|e| e.to_string())
        })?;
    }
    session.logout().await.ok();
    Ok(count)
}
fn imap_location(remote: &str) -> Result<(String, u32, u32), String> {
    let fields: Vec<_> = remote.split(':').collect();
    if fields.len() != 3 {
        return Err("Invalid IMAP identity".into());
    }
    Ok((
        String::from_utf8(STANDARD.decode(fields[0]).map_err(|_| "Invalid mailbox")?)
            .map_err(|_| "Invalid mailbox")?,
        fields[1].parse().map_err(|_| "Invalid UIDVALIDITY")?,
        fields[2].parse().map_err(|_| "Invalid UID")?,
    ))
}
async fn imap_raw(store: &Store, m: &Mail) -> Result<Vec<u8>, String> {
    let (mailbox, validity, uid) = imap_location(&m.remote_id)?;
    let mut s = imap_connect(store, &m.account_id).await?;
    let selected = s
        .examine(&mailbox)
        .await
        .map_err(|_| "Cannot select mailbox")?;
    if selected.uid_validity != Some(validity) {
        return Err("Mailbox changed. Synchronize before opening this message.".into());
    }
    let size: Vec<_> = s
        .uid_fetch(uid.to_string(), "RFC822.SIZE")
        .await
        .map_err(|_| "Cannot inspect message size")?
        .try_collect()
        .await
        .map_err(|_| "Cannot inspect message size")?;
    if size.first().and_then(|f| f.size).unwrap_or(u32::MAX) as usize > engine::MAX_RAW {
        return Err("Message exceeds 25 MiB download limit".into());
    }
    let items: Vec<_> = s
        .uid_fetch(uid.to_string(), "BODY.PEEK[]")
        .await
        .map_err(|_| "Message download failed")?
        .try_collect()
        .await
        .map_err(|_| "Message download interrupted")?;
    let raw = items
        .first()
        .and_then(|f| f.body())
        .ok_or("Message no longer exists")?
        .to_vec();
    s.logout().await.ok();
    Ok(raw)
}
pub async fn set_gmail_label(
    store: &Store,
    message: &Mail,
    label: &str,
    enabled: bool,
) -> Result<(), String> {
    let (add, remove) = if enabled {
        (vec![label], vec![])
    } else {
        (vec![], vec![label])
    };
    api(
        store,
        &message.account_id,
        Method::POST,
        &format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}/modify",
            segment(&message.remote_id)
        ),
        Some(json!({"addLabelIds":add,"removeLabelIds":remove})),
    )
    .await?;
    Ok(())
}

pub async fn move_to_mailbox(
    store: &Store,
    message: &Mail,
    target: &str,
) -> Result<Option<Mail>, String> {
    let (provider, email) = account(store, &message.account_id)?;
    match provider.as_str() {
        "microsoft" => {
            let response = api(
                store,
                &message.account_id,
                Method::POST,
                &format!(
                    "https://graph.microsoft.com/v1.0/me/messages/{}/move",
                    segment(&message.remote_id)
                ),
                Some(json!({"destinationId":target})),
            )
            .await?;
            graph_mail(
                &message.account_id,
                &email,
                &format!("mailbox:{target}"),
                &response,
            )
            .map(Some)
        }
        "imap" => {
            let (source, validity, uid) = imap_location(&message.remote_id)?;
            if source == target {
                return Err("This message is already in that folder".into());
            }
            let mut session = imap_connect(store, &message.account_id).await?;
            let selected = session
                .select(&source)
                .await
                .map_err(|_| "Cannot select source folder")?;
            if selected.uid_validity != Some(validity) {
                return Err("Mailbox changed; synchronize first".into());
            }
            let exists = session
                .uid_search(format!("UID {uid}"))
                .await
                .map_err(|_| "Cannot find the source message")?;
            if !exists.contains(&uid) {
                return Err("This message has moved or been removed. Synchronize first.".into());
            }
            session
                .uid_mv(uid.to_string(), target)
                .await
                .map_err(|_| "IMAP move could not be confirmed. Synchronize before retrying.")?;
            session.logout().await.ok();
            Ok(None)
        }
        _ => Err("This account uses labels instead of folders".into()),
    }
}

pub async fn mutate(store: &Store, m: &Mail, action: &str) -> Result<(), String> {
    let (provider, _) = account(store, &m.account_id)?;
    match provider.as_str() {
        "google" => {
            let (add, remove) = match action {
                "archive" => (vec![], vec!["INBOX"]),
                "inbox" => (vec!["INBOX"], vec!["TRASH", "SPAM"]),
                "trash" => (vec!["TRASH"], vec!["INBOX"]),
                "spam" => (vec!["SPAM"], vec!["INBOX"]),
                "read" => (vec![], vec!["UNREAD"]),
                "unread" => (vec!["UNREAD"], vec![]),
                "star" => (vec!["STARRED"], vec![]),
                "unstar" => (vec![], vec!["STARRED"]),
                _ => return Err("Unsupported action".into()),
            };
            api(
                store,
                &m.account_id,
                Method::POST,
                &format!(
                    "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}/modify",
                    segment(&m.remote_id)
                ),
                Some(json!({"addLabelIds":add,"removeLabelIds":remove})),
            )
            .await?;
        }
        "microsoft" => {
            let url = format!(
                "https://graph.microsoft.com/v1.0/me/messages/{}",
                segment(&m.remote_id)
            );
            match action {
                "read" | "unread" => {
                    api(
                        store,
                        &m.account_id,
                        Method::PATCH,
                        &url,
                        Some(json!({"isRead":action=="read"})),
                    )
                    .await?;
                }
                "star" | "unstar" => {
                    api(store,&m.account_id,Method::PATCH,&url,Some(json!({"flag":{"flagStatus":if action=="star"{"flagged"}else{"notFlagged"}}}))).await?;
                }
                _ => {
                    let dest = match action {
                        "inbox" => "inbox",
                        "archive" => "archive",
                        "trash" => "deleteditems",
                        "spam" => "junkemail",
                        _ => return Err("Unsupported action".into()),
                    };
                    api(
                        store,
                        &m.account_id,
                        Method::POST,
                        &format!("{url}/move"),
                        Some(json!({"destinationId":dest})),
                    )
                    .await?;
                }
            }
        }
        "imap" => {
            let (mailbox, validity, uid) = imap_location(&m.remote_id)?;
            let mut s = imap_connect(store, &m.account_id).await?;
            let selected = s
                .select(&mailbox)
                .await
                .map_err(|_| "Cannot select mailbox")?;
            if selected.uid_validity != Some(validity) {
                return Err("Mailbox changed; synchronize first".into());
            }
            match action {
                "read" | "unread" | "star" | "unstar" => {
                    let flag = match action {
                        "read" => "+FLAGS.SILENT (\\Seen)",
                        "unread" => "-FLAGS.SILENT (\\Seen)",
                        "star" => "+FLAGS.SILENT (\\Flagged)",
                        _ => "-FLAGS.SILENT (\\Flagged)",
                    };
                    let _: Vec<_> = s
                        .uid_store(uid.to_string(), flag)
                        .await
                        .map_err(|_| "Flag update failed")?
                        .try_collect()
                        .await
                        .map_err(|_| "Flag update failed")?;
                }
                _ => {
                    let target=db(store,|d|d.query_row("SELECT id FROM mailboxes WHERE account_id=?1 AND special=?2 LIMIT 1",params![m.account_id,action],|r|r.get::<_,String>(0)).optional().map_err(|e|e.to_string()))?.ok_or("No matching server folder. Configure the server folder before moving mail.")?;
                    s.uid_mv(uid.to_string(), &target).await.map_err(|_| {
                        "Server must support safe UID MOVE; message was not deleted"
                    })?;
                }
            }
            s.logout().await.ok();
        }
        _ => return Err("Unsupported provider".into()),
    }
    Ok(())
}

pub fn build_mime(store: &Store, draft: &engine::Draft, job_id: &str) -> Result<Vec<u8>, String> {
    use lettre::message::{header::ContentType, Attachment, Mailbox, MultiPart, SinglePart};
    let (provider, email) = account(store, &draft.account_id)?;
    crate::compose::validate_draft(draft)?;
    let mut builder = lettre::Message::builder()
        .from(
            email
                .parse::<Mailbox>()
                .map_err(|_| "Invalid sending account")?,
        )
        .subject(&draft.subject)
        .message_id(Some(format!("<{job_id}@rustymail.local>")));
    let [to, cc, bcc] = crate::compose::recipient_sets(draft)?;
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err("Enter between 1 and 100 recipients".into());
    }
    for m in to {
        builder = builder.to(m)
    }
    for m in cc {
        builder = builder.cc(m)
    }
    for m in bcc {
        builder = builder.bcc(m)
    }
    // SMTP takes a separate envelope; provider MIME APIs require the Bcc header
    // for delivery and strip it from recipient-visible copies themselves.
    if provider != "imap" {
        builder = builder.keep_bcc()
    }
    if let Some(id) = &draft.reply_message_id {
        builder =
            builder.in_reply_to(crate::compose::message_id(id).ok_or("Invalid reply Message-ID")?);
    }
    if !draft.reply_references.is_empty() {
        if draft.reply_references.len() > 20 {
            return Err("Too many reply references".into());
        }
        let refs = draft
            .reply_references
            .iter()
            .map(|s| crate::compose::message_id(s).ok_or("Invalid References header"))
            .collect::<Result<Vec<_>, _>>()?;
        builder = builder.references(refs.join(" "));
    }
    let mut parts = MultiPart::mixed().singlepart(SinglePart::plain(draft.body.clone()));
    let mut total = 0;
    for id in &draft.attachments {
        let (name, mime, data): (String, String, Vec<u8>) = db(store, |d| {
            d.query_row(
                "SELECT f.name,f.mime,f.data FROM files f JOIN outgoing_owners o ON o.file_id=f.id WHERE f.id=?1 AND f.message_id='outgoing' AND o.account_id=?2",
                params![id, draft.account_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|_| "Outgoing attachment missing or belongs to another account; reattach it".into())
        })?;
        total += data.len();
        if total > 18 * 1024 * 1024 {
            return Err("Attachments exceed 18 MiB total limit".into());
        }
        parts = parts.singlepart(
            Attachment::new(engine::safe_filename(&name)).body(
                data,
                mime.parse::<ContentType>().unwrap_or(
                    ContentType::parse("application/octet-stream")
                        .map_err(|_| "Invalid attachment type")?,
                ),
            ),
        );
    }
    let raw = builder
        .multipart(parts)
        .map_err(|_| "Cannot build MIME message")?
        .formatted();
    if raw.len() > engine::MAX_RAW {
        return Err("Encoded message exceeds 25 MiB".into());
    }
    Ok(raw)
}
pub fn graph_recipients(draft: &engine::Draft) -> Result<Value, String> {
    let sets = crate::compose::recipient_sets(draft)?;
    let names = ["toRecipients", "ccRecipients", "bccRecipients"];
    let mut out = json!({});
    for (name, list) in names.into_iter().zip(sets) {
        out[name]=json!(list.into_iter().map(|m|json!({"emailAddress":{"address":m.email.to_string(),"name":m.name.unwrap_or_default()}})).collect::<Vec<_>>())
    }
    Ok(out)
}
pub fn verify_graph_recipients(expected: &Value, actual: &Value) -> Result<(), String> {
    for key in ["toRecipients", "ccRecipients", "bccRecipients"] {
        let addresses = |v: &Value| -> Result<std::collections::BTreeSet<String>, String> {
            v[key]
                .as_array()
                .ok_or("Provider omitted recipient verification")?
                .iter()
                .map(|m| {
                    m["emailAddress"]["address"]
                        .as_str()
                        .map(|s| s.to_ascii_lowercase())
                        .ok_or_else(|| "Invalid provider recipient response".into())
                })
                .collect()
        };
        if addresses(expected)? != addresses(actual)? {
            return Err("Provider reply recipients differ from the reviewed draft; the provider draft was not sent".into());
        }
    }
    Ok(())
}
pub fn gmail_send_payload(draft: &engine::Draft, raw: &[u8]) -> Value {
    let mut value = json!({"raw":URL_SAFE_NO_PAD.encode(raw)});
    if let Some(thread) = &draft.provider_thread_id {
        value["threadId"] = json!(thread)
    }
    value
}
pub async fn send(
    store: &Store,
    id: &str,
    draft: &engine::Draft,
    raw: Vec<u8>,
) -> Result<(), String> {
    let (provider, _) = account(store, &draft.account_id)?;
    match provider.as_str() {
        "google" => {
            api(
                store,
                &draft.account_id,
                Method::POST,
                "https://gmail.googleapis.com/gmail/v1/users/me/messages/send",
                Some(gmail_send_payload(draft, &raw)),
            )
            .await?;
        }
        "microsoft" => {
            if let Some(source) = &draft.provider_reply_id {
                let key = format!("graph-reply:{id}");
                let existing = db(store, |d| engine::state(d, &draft.account_id, &key))?;
                let remote = if let Some(remote) = existing {
                    remote
                } else {
                    let access = token(store, &draft.account_id, false).await?;
                    let response = client()?
                        .post(format!(
                            "https://graph.microsoft.com/v1.0/me/messages/{}/createReply",
                            segment(source)
                        ))
                        .bearer_auth(access)
                        .header("Prefer", "IdType=\"ImmutableId\"")
                        .header("Content-Type", "text/plain")
                        .body(STANDARD.encode(&raw))
                        .send()
                        .await
                        .map_err(|_| "Creating the provider reply draft failed")?;
                    let value: Value = serde_json::from_slice(&bytes(response, 65536).await?)
                        .map_err(|_| "Invalid reply draft response")?;
                    let remote = value["id"]
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() <= 8192)
                        .ok_or("Provider omitted reply draft identity")?
                        .to_string();
                    db(store, |d| {
                        engine::checkpoint(d, &draft.account_id, &key, &remote)
                    })?;
                    remote
                };
                let recipients = graph_recipients(draft)?;
                let updated = api(
                    store,
                    &draft.account_id,
                    Method::PATCH,
                    &format!(
                        "https://graph.microsoft.com/v1.0/me/messages/{}",
                        segment(&remote)
                    ),
                    Some(recipients.clone()),
                )
                .await?;
                verify_graph_recipients(&recipients, &updated)?;
                api(
                    store,
                    &draft.account_id,
                    Method::POST,
                    &format!(
                        "https://graph.microsoft.com/v1.0/me/messages/{}/send",
                        segment(&remote)
                    ),
                    None,
                )
                .await?;
            } else {
                let access = token(store, &draft.account_id, false).await?;
                let r = client()?
                    .post("https://graph.microsoft.com/v1.0/me/sendMail")
                    .bearer_auth(access)
                    .header("Content-Type", "text/plain")
                    .body(STANDARD.encode(raw))
                    .send()
                    .await
                    .map_err(|_| "Submission connection failed")?;
                bytes(r, 65536).await?;
            }
        }
        "imap" => {
            let c = server_config(store, &draft.account_id)?;
            let password = keyring::Entry::new("dev.rustymail.smtp", &draft.account_id)
                .map_err(|_| "Credential store unavailable")?
                .get_password()
                .map_err(|_| "Missing SMTP credential")?;
            let recipients = crate::compose::recipient_sets(draft)?
                .into_iter()
                .flatten()
                .map(|m| m.email.to_string())
                .collect::<Vec<_>>()
                .join(",");
            smtp_submit(c, password, recipients, raw).await?;
        }
        _ => return Err("Unsupported provider".into()),
    }
    let _ = id;
    Ok(())
}

async fn smtp_submit(
    config: ServerConfig,
    password: String,
    recipients: String,
    raw: Vec<u8>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        use lettre::{
            address::Envelope,
            transport::smtp::{
                authentication::Credentials,
                client::{Certificate, Tls, TlsParameters},
            },
            Transport,
        };
        let mut tls = TlsParameters::builder(config.smtp_host.clone());
        if let Some(pem) = config.ca_pem {
            tls = tls.add_root_certificate(
                Certificate::from_pem(pem.as_bytes()).map_err(|_| "Invalid custom CA")?,
            )
        }
        let tls = tls.build().map_err(|_| "Cannot initialize SMTP TLS")?;
        let smtp = lettre::SmtpTransport::builder_dangerous(&config.smtp_host)
            .port(config.smtp_port)
            .tls(if config.smtp_starttls {
                Tls::Required(tls)
            } else {
                Tls::Wrapper(tls)
            })
            .credentials(Credentials::new(config.smtp_username, password))
            .timeout(Some(Duration::from_secs(60)))
            .build();
        let to = recipients
            .split(',')
            .map(str::trim)
            .map(|s| {
                s.parse::<lettre::message::Mailbox>()
                    .map(|m| m.email)
                    .map_err(|_| "Invalid recipient")
            })
            .collect::<Result<Vec<_>, _>>()?;
        smtp.send_raw(
            &Envelope::new(
                Some(config.email.parse().map_err(|_| "Invalid sender")?),
                to,
            )
            .map_err(|_| "Invalid envelope")?,
            &raw,
        )
        .map_err(|_| "SMTP submission failed or its result is unknown")?;
        Ok(())
    })
    .await
    .map_err(|_| "SMTP task stopped")?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn smtp_submission_and_lost_acknowledgement() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio_rustls::{
            rustls::{self, pki_types::PrivatePkcs8KeyDer},
            TlsAcceptor,
        };
        for lose_ack in [false, true] {
            let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
            let pem = cert.cert.pem();
            let tls = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.cert.der().clone()],
                PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
            )
            .unwrap();
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let stream = TlsAcceptor::from(std::sync::Arc::new(tls))
                    .accept(socket)
                    .await
                    .unwrap();
                let mut io = BufReader::new(stream);
                io.get_mut()
                    .write_all(b"220 localhost synthetic ESMTP\r\n")
                    .await
                    .unwrap();
                let mut line = String::new();
                let mut data = false;
                let mut received = String::new();
                loop {
                    line.clear();
                    if io.read_line(&mut line).await.unwrap_or(0) == 0 {
                        break;
                    }
                    if data {
                        if line == ".\r\n" {
                            if lose_ack {
                                break;
                            }
                            data = false;
                            io.get_mut().write_all(b"250 Accepted\r\n").await.unwrap();
                        } else {
                            received.push_str(&line)
                        }
                        continue;
                    }
                    let response: &[u8] = if line.starts_with("EHLO") {
                        b"250-localhost\r\n250-AUTH PLAIN LOGIN\r\n250 SIZE 26214400\r\n".as_slice()
                    } else if line.starts_with("AUTH") {
                        b"235 Authenticated\r\n"
                    } else if line.starts_with("DATA") {
                        data = true;
                        b"354 Send data\r\n"
                    } else if line.starts_with("QUIT") {
                        let _ = io.get_mut().write_all(b"221 Bye\r\n").await;
                        break;
                    } else {
                        b"250 OK\r\n"
                    };
                    io.get_mut().write_all(response).await.unwrap();
                }
                received
            });
            let config = ServerConfig {
                email: "sender@example.com".into(),
                imap_host: "127.0.0.1".into(),
                imap_port: 993,
                username: "fixture".into(),
                smtp_host: "127.0.0.1".into(),
                smtp_port: port,
                smtp_username: "fixture".into(),
                smtp_starttls: false,
                ca_pem: Some(pem),
            };
            let result=smtp_submit(config,"fixture-password".into(),"receiver@example.com".into(),b"From: sender@example.com\r\nTo: receiver@example.com\r\nSubject: Synthetic transport test\r\n\r\nSynthetic body\r\n".to_vec()).await;
            let received = server.await.unwrap();
            assert!(received.contains("Subject: Synthetic transport test"));
            assert_eq!(result.is_err(), lose_ack);
        }
    }
    #[test]
    fn continuation_urls_cannot_exfiltrate_tokens() {
        assert!(approved_url(
            "microsoft",
            "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$skiptoken=x"
        ));
        for url in [
            "https://evil.example/v1.0/me/mail",
            "http://graph.microsoft.com/v1.0/me/mail",
            "https://graph.microsoft.com.evil/v1.0/me/mail",
            "https://user@graph.microsoft.com/v1.0/me/mail",
        ] {
            assert!(!approved_url("microsoft", url));
        }
    }
    #[test]
    fn gmail_labels_and_identity() {
        let v = json!({"id":"42","labelIds":["INBOX","UNREAD","STARRED"],"internalDate":"1000","payload":{"headers":[{"name":"From","value":"A <a@example.com>"},{"name":"Subject","value":"Hello"}]}});
        let m = gmail_mail("a", "me@example.com", &v).unwrap();
        assert!(m.unread && m.starred);
        assert_eq!(m.folder, "inbox");
        assert_eq!(m.received, 1);
        assert_ne!(m.id, gmail_mail("b", "other@example.com", &v).unwrap().id);
    }

    #[test]
    fn threading_metadata_refresh_preserves_pending_local_changes() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        engine::migrate(&db).unwrap();
        let v = json!({"id":"42","threadId":"t","labelIds":["INBOX","UNREAD"]});
        let remote = gmail_mail("a", "me@example.com", &v).unwrap();
        let mut cached = remote.clone();
        cached.thread_id.clear();
        cached.provider.clear();
        cached.folder = "archive".into();
        cached.unread = false;
        engine::put(&db, &cached).unwrap();
        db.execute(
            "INSERT INTO jobs VALUES('job','a','mutation',?1,'pending','',0,0)",
            [json!({"id":cached.id,"action":"archive"}).to_string()],
        )
        .unwrap();
        put_remote(&db, &remote).unwrap();
        let saved = engine::get(&db, &cached.id).unwrap();
        assert_eq!(saved.thread_id, "t");
        assert_eq!(saved.provider, "google");
        assert_eq!(saved.folder, "archive");
        assert!(!saved.unread);
    }

    #[tokio::test]
    async fn imap_tls_accepts_explicit_ca_and_rejects_untrusted_server() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio_rustls::{
            rustls::{self, pki_types::PrivatePkcs8KeyDer},
            TlsAcceptor,
        };
        async fn server() -> (u16, String, tokio::task::JoinHandle<bool>) {
            let cert =
                rcgen::generate_simple_self_signed(vec!["127.0.0.1".into(), "localhost".into()])
                    .unwrap();
            let pem = cert.cert.pem();
            let config = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.cert.der().clone()],
                PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
            )
            .unwrap();
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            let task = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let Ok(stream) = TlsAcceptor::from(std::sync::Arc::new(config))
                    .accept(socket)
                    .await
                else {
                    return false;
                };
                let mut io = BufReader::new(stream);
                if io
                    .get_mut()
                    .write_all(b"* OK synthetic IMAP ready\r\n")
                    .await
                    .is_err()
                {
                    return false;
                }
                let mut line = String::new();
                let mut login = false;
                loop {
                    line.clear();
                    if io.read_line(&mut line).await.unwrap_or(0) == 0 {
                        break;
                    }
                    let tag = line.split_whitespace().next().unwrap_or("a1").to_string();
                    if line.contains("LOGIN") {
                        login = true
                    }
                    let logout = line.contains("LOGOUT");
                    let response = if logout {
                        format!("* BYE closing\r\n{tag} OK LOGOUT completed\r\n")
                    } else {
                        format!("{tag} OK completed\r\n")
                    };
                    if io.get_mut().write_all(response.as_bytes()).await.is_err() {
                        break;
                    }
                    if logout {
                        break;
                    }
                }
                login
            });
            (port, pem, task)
        }
        let (port, pem, trusted) = server().await;
        let mut config = ServerConfig {
            email: "synthetic@example.com".into(),
            imap_host: "127.0.0.1".into(),
            imap_port: port,
            username: "synthetic".into(),
            smtp_host: "127.0.0.1".into(),
            smtp_port: 465,
            smtp_username: "synthetic".into(),
            smtp_starttls: false,
            ca_pem: Some(pem),
        };
        validate_imap_credentials(&config, "synthetic-app-password")
            .await
            .unwrap();
        assert!(trusted.await.unwrap());
        let (port, _, untrusted) = server().await;
        config.imap_port = port;
        config.ca_pem = None;
        assert!(validate_imap_credentials(&config, "synthetic-app-password")
            .await
            .is_err());
        assert!(!untrusted.await.unwrap());
    }
}

#[cfg(test)]
mod compose_provider_tests {
    use super::*;
    #[test]
    fn graph_reply_verifies_exact_reviewed_recipient_sets() {
        let d = engine::Draft {
            to: "To <to@example.com>".into(),
            cc: "copy@example.com".into(),
            bcc: "private@example.com".into(),
            ..Default::default()
        };
        let expected = graph_recipients(&d).unwrap();
        assert!(verify_graph_recipients(&expected, &expected).is_ok());
        let mut wrong = expected.clone();
        wrong["toRecipients"] = json!([{"emailAddress":{"address":"unexpected@example.com"}}]);
        assert!(verify_graph_recipients(&expected, &wrong).is_err());
        let mut missing = expected.clone();
        missing.as_object_mut().unwrap().remove("bccRecipients");
        assert!(verify_graph_recipients(&expected, &missing).is_err());
    }
}
