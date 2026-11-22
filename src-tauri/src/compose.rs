//! Recipient policy and composition work happen on demand, never on a mailbox page.
use crate::{
    engine::{self, Attachment, Draft, Mail},
    providers::{self, db},
    Store,
};
use lettre::message::{Mailbox, Mailboxes};
use rusqlite::params;
use serde::Serialize;
use std::collections::HashSet;

#[tauri::command]
pub fn get_signatures(
    store: tauri::State<Store>,
) -> Result<std::collections::HashMap<String, String>, String> {
    db(&store, |db| {
        let mut query = db
            .prepare("SELECT account_id,value FROM sync_state WHERE key='signature'")
            .map_err(|error| error.to_string())?;
        let rows = query
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<_, _>>()
            .map_err(|error| error.to_string())
    })
}

#[tauri::command]
pub fn set_signature(
    store: tauri::State<Store>,
    account_id: String,
    signature: String,
) -> Result<String, String> {
    providers::account(&store, &account_id)?;
    let signature = signature.trim().to_owned();
    validate_signature(&signature)?;
    db(&store, |db| {
        engine::checkpoint(db, &account_id, "signature", &signature)
    })?;
    Ok(signature)
}

fn validate_signature(signature: &str) -> Result<(), String> {
    if signature.len() > 4096
        || signature
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err("Signatures must fit in 4 KiB and contain plain text".into());
    }
    Ok(())
}

#[derive(Serialize)]
pub struct RecentRecipient {
    address: String,
    formatted: String,
}

#[tauri::command]
pub fn recent_recipients(
    store: tauri::State<Store>,
    account_id: String,
) -> Result<Vec<RecentRecipient>, String> {
    providers::account(&store, &account_id)?;
    db(&store, |db| recent_for_account(db, &account_id))
}

fn recent_for_account(
    db: &rusqlite::Connection,
    account_id: &str,
) -> Result<Vec<RecentRecipient>, String> {
    let mut query = db
        .prepare(
            "SELECT json_extract(payload,'$.to'),json_extract(payload,'$.cc'),json_extract(payload,'$.bcc')
             FROM jobs WHERE account_id=?1 AND kind='send' AND status='done' AND json_valid(payload)
             ORDER BY rowid DESC LIMIT 30",
        )
        .map_err(|error| error.to_string())?;
    let rows = query
        .query_map([account_id], |row| {
            Ok(Draft {
                to: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                cc: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                bcc: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                ..Default::default()
            })
        })
        .map_err(|error| error.to_string())?;
    let mut recent = Vec::new();
    let mut seen = HashSet::new();
    for row in rows {
        let draft = row.map_err(|error| error.to_string())?;
        let Ok(sets) = recipient_sets(&draft) else {
            continue;
        };
        for mailbox in sets.into_iter().flatten() {
            let address = mailbox.email.to_string();
            if seen.insert(address.to_ascii_lowercase()) {
                recent.push(RecentRecipient {
                    address,
                    formatted: mailbox.to_string(),
                });
                if recent.len() == 100 {
                    return Ok(recent);
                }
            }
        }
    }
    Ok(recent)
}

pub fn parse_recipients(input: &str) -> Result<Vec<Mailbox>, String> {
    if input.len() > 8192 || input.chars().any(char::is_control) {
        return Err(
            "Recipient fields must be under 8 KiB and contain no control characters".into(),
        );
    }
    if input.trim().is_empty() {
        return Ok(vec![]);
    }
    let list: Mailboxes = input.parse().map_err(|_| {
        "Enter valid addresses separated by commas; quote display names containing commas"
    })?;
    let result: Vec<_> = list.iter().cloned().collect();
    if result.len() > 100 {
        return Err("Use at most 100 recipients in total".into());
    }
    Ok(result)
}
pub fn recipient_sets(draft: &Draft) -> Result<[Vec<Mailbox>; 3], String> {
    let mut seen = HashSet::new();
    let mut sets = [vec![], vec![], vec![]];
    for (i, input) in [&draft.to, &draft.cc, &draft.bcc].iter().enumerate() {
        for mailbox in parse_recipients(input)? {
            if seen.insert(mailbox.email.to_string().to_ascii_lowercase()) {
                sets[i].push(mailbox)
            }
        }
    }
    if sets.iter().map(Vec::len).sum::<usize>() > 100 {
        return Err("Use at most 100 recipients in total".into());
    }
    Ok(sets)
}
pub fn validate_draft(draft: &Draft) -> Result<(), String> {
    validate_draft_storage(draft)?;
    recipient_sets(draft)?;
    Ok(())
}
/// Autosave must retain an unfinished address while the user is typing it.
pub fn validate_draft_storage(draft: &Draft) -> Result<(), String> {
    if [&draft.to, &draft.cc, &draft.bcc]
        .iter()
        .any(|s| s.len() > 8192 || s.chars().any(char::is_control))
    {
        return Err("Recipient fields exceed storage limits".into());
    }
    validate_signature(&draft.signature)?;
    if draft.body.len()
        > 1_000_000usize.saturating_sub(if draft.signature.trim().is_empty() {
            0
        } else {
            draft.signature.len() + 6
        })
        || draft.subject.len() > 4096
        || draft.subject.contains(['\r', '\n'])
        || draft.attachments.len() > 20
        || draft.attachments.iter().any(|id| id.len() > 128)
        || !matches!(
            draft.mode.as_str(),
            "" | "new" | "reply" | "reply_all" | "forward"
        )
    {
        return Err("Draft exceeds supported limits".into());
    }
    Ok(())
}
pub fn message_id(raw: &str) -> Option<String> {
    let id = raw.trim().trim_start_matches('<').trim_end_matches('>');
    if id.len() > 254
        || !id.contains('@')
        || id
            .bytes()
            .any(|b| b <= 32 || b >= 127 || b"<>(),;:\\[]\"".contains(&b))
    {
        return None;
    }
    Some(format!("<{id}>"))
}
pub fn reference_ids<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut root = None;
    let mut tail = std::collections::VecDeque::new();
    for value in values {
        if let Some(id) = message_id(value) {
            if root.is_none() {
                root = Some(id)
            } else {
                tail.push_back(id);
                if tail.len() > 19 {
                    tail.pop_front();
                }
            }
        }
    }
    let mut out: Vec<_> = root.into_iter().chain(tail).collect();
    let mut seen = HashSet::new();
    out.retain(|v| seen.insert(v.clone()));
    out
}
fn refs(source: &Mail) -> Vec<String> {
    reference_ids(
        source
            .references
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(source.message_id.as_str())),
    )
}
fn prefixed(subject: &str, prefix: &str) -> String {
    let trimmed = subject.trim();
    let head = trimmed
        .split_once(':')
        .map(|(s, _)| s.trim().to_ascii_lowercase());
    if (prefix == "Re" && head.as_deref() == Some("re"))
        || (prefix == "Fwd" && matches!(head.as_deref(), Some("fw" | "fwd")))
    {
        trimmed.into()
    } else {
        format!("{prefix}: {trimmed}")
    }
}
fn own_addresses(store: &Store) -> Result<HashSet<String>, String> {
    db(store, |d| {
        let mut q = d
            .prepare("SELECT email FROM accounts UNION SELECT address FROM account_aliases")
            .map_err(|e| e.to_string())?;
        let rows = q
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|v| v.map(|s| s.to_ascii_lowercase()).map_err(|e| e.to_string()))
            .collect()
    })
}
#[derive(Serialize)]
pub struct ComposeSeed {
    pub draft: Draft,
    pub available_attachments: Vec<Attachment>,
}
pub fn seed(store: &Store, m: &Mail, mode: &str) -> Result<ComposeSeed, String> {
    if !m.loaded || !matches!(mode, "reply" | "reply_all" | "forward") {
        return Err("Open the message before replying or forwarding".into());
    }
    if mode == "reply_all" && m.recipients_truncated {
        return Err("This message has too many recipients for automatic Reply all; use Reply and review recipients manually".into());
    }
    let own = own_addresses(store)?;
    let mut seen = own.clone();
    let mut to = Vec::new();
    let mut cc = Vec::new();
    let sender_owned = own.contains(&m.address.to_ascii_lowercase()) || m.folder == "sent";
    let primary = if sender_owned {
        m.to.clone()
    } else if !m.reply_to.is_empty() {
        m.reply_to.clone()
    } else {
        vec![m.address.clone()]
    };
    let mut add = |target: &mut Vec<String>, values: &[String]| {
        for value in values {
            if let Ok(values) = parse_recipients(value) {
                for m in values {
                    if seen.insert(m.email.to_string().to_ascii_lowercase()) {
                        target.push(m.to_string())
                    }
                }
            }
        }
    };
    if mode != "forward" {
        add(&mut to, &primary);
        if mode == "reply_all" {
            add(&mut to, &m.to);
            add(&mut cc, &m.cc)
        }
    }
    // Stream the quote into one bounded buffer, including newline-heavy mail.
    let mut body = if mode == "forward" {
        format!("\n\n---------- Forwarded message ----------\nFrom: {} <{}>\nDate: {}\nSubject: {}\nTo: {}\nCc: {}\n\n",m.sender,m.address,m.date,m.subject,m.to.join(", "),m.cc.join(", "))
    } else {
        format!("\n\nOn {}, {} <{}> wrote:\n", m.date, m.sender, m.address)
    };
    let append = |out: &mut String, text: &str| {
        let mut end = text.len().min(900_000usize.saturating_sub(out.len()));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&text[..end]);
        end == text.len()
    };
    let complete = if mode == "forward" {
        append(&mut body, &m.body)
    } else {
        let mut complete = true;
        for (index, line) in m.body.lines().enumerate() {
            if (index > 0 && !append(&mut body, "\n"))
                || !append(&mut body, "> ")
                || !append(&mut body, line)
            {
                complete = false;
                break;
            }
        }
        complete
    };
    if !complete {
        body.push_str("\n[Quoted content shortened]");
    }
    let draft = Draft {
        account_id: m.account_id.clone(),
        to: to.join(", "),
        cc: cc.join(", "),
        subject: prefixed(&m.subject, if mode == "forward" { "Fwd" } else { "Re" }),
        body,
        mode: mode.into(),
        source_message_id: Some(m.id.clone()),
        ..Default::default()
    };
    Ok(ComposeSeed {
        draft,
        available_attachments: if mode == "forward" {
            m.attachments.clone()
        } else {
            vec![]
        },
    })
}
/// Freeze provider context at enqueue time, alongside the immutable MIME bytes.
pub fn prepare_send(store: &Store, mut draft: Draft) -> Result<Draft, String> {
    validate_draft(&draft)?;
    if !draft.signature.trim().is_empty() {
        draft.body.push_str("\n\n-- \n");
        draft.body.push_str(&draft.signature);
        draft.signature.clear();
    }
    draft.provider_thread_id = None;
    draft.provider_reply_id = None;
    draft.reply_references.clear();
    if let Some(id) = &draft.source_message_id {
        let m = db(store, |d| engine::get(d, id))?;
        if m.account_id != draft.account_id {
            return Err("The source message belongs to a different sending account".into());
        }
        draft.reply_message_id = None;
        if matches!(draft.mode.as_str(), "reply" | "reply_all") {
            draft.reply_message_id = message_id(&m.message_id);
            draft.reply_references = refs(&m);
            let (provider, _) = providers::account(store, &draft.account_id)?;
            if provider == "google"
                && draft.reply_message_id.is_some()
                && !m.thread_id.is_empty()
                && (draft.subject == m.subject || draft.subject == prefixed(&m.subject, "Re"))
            {
                draft.provider_thread_id = Some(m.thread_id)
            }
            if provider == "microsoft" {
                draft.provider_reply_id = Some(m.remote_id)
            }
        }
    } else if let Some(id) = draft.reply_message_id.clone() {
        draft.reply_message_id = Some(message_id(&id).ok_or("Invalid reply Message-ID")?)
    }
    if draft.mode == "forward" {
        draft.reply_message_id = None;
        draft.reply_references.clear()
    }
    Ok(draft)
}
#[tauri::command]
pub async fn prepare_compose(
    store: tauri::State<'_, Store>,
    runtime: tauri::State<'_, std::sync::Arc<crate::live::Runtime>>,
    id: String,
    mode: String,
) -> Result<ComposeSeed, String> {
    let m = crate::live::get_mail_inner(&store, &runtime, &id).await?;
    seed(&store, &m, &mode)
}
#[tauri::command]
pub fn get_account_aliases(
    store: tauri::State<Store>,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    db(&store, |d| {
        let mut result = std::collections::BTreeMap::<String, String>::new();
        let mut q = d
            .prepare("SELECT account_id,address FROM account_aliases ORDER BY account_id,address")
            .map_err(|e| e.to_string())?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, address) = row.map_err(|e| e.to_string())?;
            let value = result.entry(id).or_default();
            if !value.is_empty() {
                value.push_str(", ")
            }
            value.push_str(&address)
        }
        Ok(result)
    })
}
pub fn save_aliases_inner(store: &Store, account_id: &str, addresses: &str) -> Result<(), String> {
    providers::account(store, account_id)?;
    let values = parse_recipients(addresses)?;
    if values.len() > 20 {
        return Err("Use at most 20 aliases per account".into());
    }
    db(store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM account_aliases WHERE account_id=?1",
            [account_id],
        )
        .map_err(|e| e.to_string())?;
        for m in values {
            tx.execute(
                "INSERT OR IGNORE INTO account_aliases VALUES(?1,?2)",
                params![account_id, m.email.to_string().to_ascii_lowercase()],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    })
}
#[tauri::command]
pub fn save_account_aliases(
    store: tauri::State<Store>,
    account_id: String,
    addresses: String,
) -> Result<(), String> {
    save_aliases_inner(&store, &account_id, &addresses)
}
pub fn copy_attachments_inner(
    store: &Store,
    message_id: &str,
    account_id: &str,
    ids: &[String],
    draft_id: Option<&str>,
) -> Result<Vec<Attachment>, String> {
    providers::account(store, account_id)?;
    if ids.len() > 20 {
        return Err("Use at most 20 attachments".into());
    }
    db(store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        let m = engine::get(&tx, message_id)?;
        if m.account_id != account_id {
            return Err("Forwarded attachments must use the source account".into());
        }
        let mut total = 0;
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for id in ids {
            if !seen.insert(id) {
                continue;
            }
            let (name, mime, data): (String, String, Vec<u8>) = tx
                .query_row(
                    "SELECT name,mime,data FROM files WHERE id=?1 AND message_id=?2",
                    params![id, message_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(|_| {
                    "Forward attachment is no longer cached; reopen the original message"
                })?;
            total += data.len();
            if total > 18 * 1024 * 1024 {
                return Err("Forward attachments exceed 18 MiB".into());
            }
            result.push(crate::outgoing::store_file(
                &tx, account_id, draft_id, name, mime, data,
            )?);
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result)
    })
}
#[tauri::command]
pub fn copy_forward_attachments(
    store: tauri::State<Store>,
    message_id: String,
    account_id: String,
    ids: Vec<String>,
    draft_id: String,
) -> Result<Vec<Attachment>, String> {
    crate::outgoing::validate_draft_id(&draft_id)?;
    copy_attachments_inner(&store, &message_id, &account_id, &ids, Some(&draft_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    fn store(provider: &str) -> Store {
        let d = rusqlite::Connection::open_in_memory().unwrap();
        engine::migrate(&d).unwrap();
        d.execute_batch("CREATE TABLE accounts(id TEXT PRIMARY KEY,provider TEXT,email TEXT); INSERT INTO accounts VALUES('a','imap','me@example.com'),('b','imap','other-self@example.com');").unwrap();
        d.execute("UPDATE accounts SET provider=?1 WHERE id='a'", [provider])
            .unwrap();
        Store(Arc::new(Mutex::new(d)))
    }
    fn original(store: &Store) -> Mail {
        db(store,|d|{engine::parse_raw(d,Mail{id:"source".into(),account_id:"a".into(),remote_id:"remote-42".into(),provider:"google".into(),thread_id:"native-thread".into(),folder:"inbox".into(),..Default::default()},b"From: Sender <sender@example.com>\r\nReply-To: Reply Desk <desk@example.com>\r\nTo: Me <me@example.com>, \"Doe, Jane\" <jane@example.com>, alias@example.com, other-self@example.com\r\nCc: Jane <JANE@example.com>, Copy <copy@example.com>\r\nBcc: private-recipient@example.com\r\nSubject: Original topic\r\nMessage-ID: <parent@example.com>\r\nReferences: <root@example.com> <earlier@example.com>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nOriginal content.")}).unwrap()
    }
    #[test]
    fn reply_all_honors_reply_to_own_addresses_and_aliases() {
        let store = store("google");
        save_aliases_inner(&store, "a", "Alias <alias@example.com>").unwrap();
        let m = original(&store);
        let reply = seed(&store, &m, "reply").unwrap().draft;
        assert_eq!(
            parse_recipients(&reply.to).unwrap()[0].email.to_string(),
            "desk@example.com"
        );
        assert!(reply.cc.is_empty());
        let d = seed(&store, &m, "reply_all").unwrap().draft;
        let sets = recipient_sets(&d).unwrap();
        assert_eq!(
            sets[0]
                .iter()
                .map(|m| m.email.to_string().to_lowercase())
                .collect::<Vec<_>>(),
            ["desk@example.com", "jane@example.com"]
        );
        assert_eq!(sets[1][0].email.to_string(), "copy@example.com");
        assert_eq!(sets[1].len(), 1);
        assert!(d.bcc.is_empty());
        assert!(!d.body.contains("private-recipient"));
        assert_eq!(d.subject, "Re: Original topic");
        assert!(d.body.contains("> Original content."));
    }
    #[test]
    fn sent_followup_uses_original_recipients_not_self() {
        let store = store("imap");
        let mut m = original(&store);
        m.address = "ME@example.com".into();
        m.folder = "sent".into();
        let d = seed(&store, &m, "reply").unwrap().draft;
        assert!(!d.to.contains("me@example.com"));
        assert!(d.to.contains("jane@example.com"));
        assert!(!d.to.contains("desk@example.com"));
    }
    #[test]
    fn quoted_comma_names_and_bcc_only_are_valid_and_deduplicated() {
        let d = Draft {
            to: r#""Doe, Jane" <jane@example.com>"#.into(),
            cc: "JANE@example.com, Copy <copy@example.com>".into(),
            bcc: "hidden@example.com, COPY@example.com".into(),
            ..Default::default()
        };
        let sets = recipient_sets(&d).unwrap();
        assert_eq!(sets.iter().map(Vec::len).collect::<Vec<_>>(), [1, 1, 1]);
        let store = store("imap");
        let d = Draft {
            account_id: "a".into(),
            bcc: "hidden@example.com".into(),
            body: "Only hidden recipient".into(),
            ..Default::default()
        };
        let raw = providers::build_mime(&store, &d, "bcc-only").unwrap();
        assert!(!String::from_utf8_lossy(&raw)
            .to_ascii_lowercase()
            .contains("bcc:"));
    }
    #[test]
    fn smtp_bcc_stays_in_envelope_and_provider_bcc_stays_in_private_api_mime() {
        for provider in ["imap", "google", "microsoft"] {
            let store = store(provider);
            let d = Draft {
                account_id: "a".into(),
                to: "visible@example.com".into(),
                cc: "copy@example.com".into(),
                bcc: "hidden@example.com".into(),
                body: "hello".into(),
                ..Default::default()
            };
            let raw = providers::build_mime(&store, &d, "job").unwrap();
            let text = String::from_utf8_lossy(&raw);
            assert_eq!(text.contains("Bcc:"), provider != "imap");
            assert_eq!(text.contains("hidden@example.com"), provider != "imap");
            assert!(text.contains("Cc: copy@example.com"));
            assert_eq!(recipient_sets(&d).unwrap().into_iter().flatten().count(), 3);
        }
    }
    #[test]
    fn queue_freezes_reply_context_and_rejects_cross_account_or_forged_context() {
        let store = store("google");
        let m = original(&store);
        let mut d = seed(&store, &m, "reply_all").unwrap().draft;
        d.provider_thread_id = Some("forged".into());
        d.reply_references = vec!["<forged@example.com>".into()];
        let job = crate::live::queue_send_inner(&store, d.clone()).unwrap();
        let raw = crate::live::queued_mime(&store, &job).unwrap();
        let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
        assert_eq!(
            parsed.in_reply_to().unwrap().iter().next(),
            Some("parent@example.com")
        );
        assert_eq!(
            parsed.references().unwrap().iter().collect::<Vec<_>>(),
            [
                "root@example.com",
                "earlier@example.com",
                "parent@example.com"
            ]
        );
        let snapshot: Draft = db(&store, |d| {
            let s: String = d
                .query_row("SELECT payload FROM jobs WHERE id=?1", [&job], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            serde_json::from_str(&s).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(
            snapshot.provider_thread_id.as_deref(),
            Some("native-thread")
        );
        db(&store, |d| engine::remove(d, &m.id)).unwrap();
        assert_eq!(crate::live::queued_mime(&store, &job).unwrap(), raw);
        assert_eq!(
            providers::gmail_send_payload(&snapshot, &raw)["threadId"],
            "native-thread"
        );
        db(&store, |d| engine::put(d, &m)).unwrap();
        d.account_id = "b".into();
        assert!(prepare_send(&store, d).is_err());
        let fresh = prepare_send(
            &store,
            Draft {
                account_id: "a".into(),
                to: "to@example.com".into(),
                provider_thread_id: Some("forged".into()),
                provider_reply_id: Some("forged".into()),
                reply_references: vec!["<forged@example.com>".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            fresh.provider_thread_id.is_none()
                && fresh.provider_reply_id.is_none()
                && fresh.reply_references.is_empty()
        );
    }
    #[test]
    fn changed_subject_does_not_force_gmail_thread_and_graph_uses_native_source() {
        for provider in ["google", "microsoft"] {
            let store = store(provider);
            let m = original(&store);
            let mut d = seed(&store, &m, "reply").unwrap().draft;
            d.subject = "New topic".into();
            let ready = prepare_send(&store, d).unwrap();
            assert!(ready.provider_thread_id.is_none());
            assert_eq!(ready.provider_reply_id.is_some(), provider == "microsoft");
        }
    }
    #[test]
    fn forward_excludes_hidden_recipients_and_never_inherits_reply_headers() {
        let store = store("google");
        let m = original(&store);
        let d = seed(&store, &m, "forward").unwrap().draft;
        assert!(d.to.is_empty() && d.cc.is_empty() && d.bcc.is_empty() && d.attachments.is_empty());
        assert!(d.body.contains("Forwarded message"));
        assert!(!d.body.contains("private-recipient"));
        let d = prepare_send(&store, d).unwrap();
        assert!(
            d.reply_message_id.is_none()
                && d.reply_references.is_empty()
                && d.provider_thread_id.is_none()
                && d.provider_reply_id.is_none()
        );
    }
    #[test]
    fn forwarding_attachment_selection_copies_owned_bytes_atomically() {
        let store = store("imap");
        let m = original(&store);
        db(&store,|d|{d.execute_batch("INSERT INTO files VALUES('incoming','source','report.txt','text/plain',X'68656c6c6f',0);").map_err(|e|e.to_string())}).unwrap();
        assert!(copy_attachments_inner(&store, &m.id, "b", &["incoming".into()], None).is_err());
        assert!(copy_attachments_inner(
            &store,
            &m.id,
            "a",
            &["incoming".into(), "missing".into()],
            None
        )
        .is_err());
        assert_eq!(
            db(&store, |d| d
                .query_row("SELECT count(*) FROM outgoing_owners", [], |r| r
                    .get::<_, i64>(0))
                .map_err(|e| e.to_string()))
            .unwrap(),
            0
        );
        let files = copy_attachments_inner(&store, &m.id, "a", &["incoming".into()], None).unwrap();
        assert_eq!(files[0].name, "report.txt");
        assert_eq!(files[0].size, 5);
        db(&store, |d| engine::remove(d, "source")).unwrap();
        let draft = Draft {
            account_id: "a".into(),
            to: "to@example.com".into(),
            attachments: vec![files[0].id.clone()],
            ..Default::default()
        };
        assert!(providers::build_mime(&store, &draft, "forward-job").is_ok());
    }
    #[test]
    fn malformed_recipients_headers_and_oversized_fields_are_rejected() {
        for value in [
            "bad address",
            "a@example.com\r\nBcc: victim@example.com",
            "\"broken <a@example.com>",
        ] {
            assert!(parse_recipients(value).is_err(), "{value}")
        }
        assert!(message_id("x@a\r\nBcc: x").is_none());
        assert!(parse_recipients(&"a".repeat(8193)).is_err());
        let store = store("imap");
        assert!(save_aliases_inner(&store, "missing", "x@example.com").is_err());
    }
    #[test]
    fn incomplete_recipient_drafts_can_be_saved_but_not_submitted() {
        let draft = Draft {
            to: "unfinished@".into(),
            cc: "\"Doe, Jane".into(),
            bcc: "private@".into(),
            ..Default::default()
        };
        assert!(validate_draft_storage(&draft).is_ok());
        assert!(validate_draft(&draft).is_err());
        let injected = Draft {
            bcc: "x@example.com\r\nTo: other@example.com".into(),
            ..draft
        };
        assert!(validate_draft_storage(&injected).is_err());
    }
    #[test]
    fn reference_chain_keeps_root_and_recent_messages_with_bounded_size() {
        let ids = (0..100)
            .map(|i| format!("<{i}@example.com>"))
            .collect::<Vec<_>>();
        let out = reference_ids(ids.iter().map(String::as_str));
        assert_eq!(out.len(), 20);
        assert_eq!(out[0], "<0@example.com>");
        assert_eq!(out[19], "<99@example.com>");
    }
    #[test]
    fn recipient_history_uses_only_successful_sends_for_the_requested_account() {
        let store = store("google");
        let db = store.0.lock().unwrap();
        for (id, account, status, to) in [
            (
                "older",
                "a",
                "done",
                "older@example.com, RECENT@example.com",
            ),
            ("other", "b", "done", "other@example.com"),
            ("pending", "a", "pending", "pending@example.com"),
            (
                "newer",
                "a",
                "done",
                "\"Recent, Name\" <recent@example.com>",
            ),
        ] {
            let payload = serde_json::json!({"to":to}).to_string();
            db.execute(
                "INSERT INTO jobs VALUES(?1,?2,'send',?3,?4,'',0,0)",
                params![id, account, payload, status],
            )
            .unwrap();
        }
        let suggestions = recent_for_account(&db, "a").unwrap();
        assert_eq!(suggestions.len(), 2);
        assert_eq!(
            suggestions[0].formatted,
            "\"Recent, Name\" <recent@example.com>"
        );
        assert_eq!(suggestions[1].address, "older@example.com");
    }

    #[test]
    fn queued_mime_contains_the_reviewed_signature_once() {
        let store = store("google");
        let draft = Draft {
            account_id: "a".into(),
            to: "someone@example.com".into(),
            body: "Hello".into(),
            signature: "Example signature".into(),
            ..Default::default()
        };
        let id = crate::live::queue_send_inner(&store, draft).unwrap();
        let db = store.0.lock().unwrap();
        let wire: Vec<u8> = db
            .query_row("SELECT raw FROM outbox_mime WHERE job_id=?1", [id], |row| {
                row.get(0)
            })
            .unwrap();
        let message = mail_parser::MessageParser::default().parse(&wire).unwrap();
        let body = message.body_text(0).unwrap();
        assert!(body
            .replace("\r\n", "\n")
            .contains("Hello\n\n-- \nExample signature"));
        assert_eq!(body.matches("Example signature").count(), 1);
    }
}
