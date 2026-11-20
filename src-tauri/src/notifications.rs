use crate::{engine, providers, Store};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Preferences {
    enabled: bool,
    previews: bool,
}

fn preferences(db: &Connection) -> Result<Preferences, String> {
    engine::state(db, "", "mail-notifications")?
        .map(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))
        .transpose()
        .map(Option::unwrap_or_default)
}

pub fn initialize(db: &Connection) -> Result<(), String> {
    let enabled = preferences(db)?.enabled;
    db.execute_batch(
        "CREATE TEMP TABLE notification_session(enabled INTEGER NOT NULL, since INTEGER NOT NULL);
         CREATE TEMP TABLE notification_candidates(
            id TEXT PRIMARY KEY, account_id TEXT NOT NULL, handled INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX notification_candidates_account ON notification_candidates(account_id,handled);
         CREATE TEMP TRIGGER notification_candidate AFTER INSERT ON main.mail_groups
         WHEN new.folder='inbox' AND new.unread=1
          AND EXISTS(SELECT 1 FROM notification_session WHERE enabled=1 AND new.received>=since)
         BEGIN
          INSERT OR IGNORE INTO notification_candidates(id,account_id) VALUES(new.id,new.account_id);
         END;",
    ).map_err(|error| error.to_string())?;
    db.execute(
        "INSERT INTO notification_session VALUES(?1,?2)",
        params![enabled, engine::now()],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn notification_preferences(store: tauri::State<Store>) -> Result<Preferences, String> {
    providers::db(&store, preferences)
}

#[tauri::command]
pub fn set_notification_preferences(
    store: tauri::State<Store>,
    value: Preferences,
) -> Result<Preferences, String> {
    providers::db(&store, |db| {
        let tx = db
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        if preferences(&tx)?.enabled != value.enabled {
            tx.execute(
                "UPDATE notification_session SET enabled=?1,since=?2",
                params![value.enabled, engine::now()],
            )
            .map_err(|error| error.to_string())?;
            tx.execute("DELETE FROM notification_candidates", [])
                .map_err(|error| error.to_string())?;
        }
        engine::checkpoint(
            &tx,
            "",
            "mail-notifications",
            &serde_json::to_string(&value).map_err(|error| error.to_string())?,
        )?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(value)
    })
}

#[tauri::command]
pub fn unread_counts(store: tauri::State<Store>) -> Result<HashMap<String, i64>, String> {
    providers::db(&store, counts)
}

fn counts(db: &Connection) -> Result<HashMap<String, i64>, String> {
    let mut query = db
        .prepare(
            "SELECT a.id,
            coalesce((SELECT sum(unread) FROM inbox_summary WHERE account_id=a.id),0)
            - (SELECT count(*) FROM snoozes s JOIN mail_groups m ON m.id=s.id
               WHERE s.wake>?1 AND m.account_id=a.id AND m.folder='inbox' AND m.unread=1)
         FROM accounts a",
        )
        .map_err(|error| error.to_string())?;
    let rows = query
        .query_map([engine::now()], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<_, _>>()
        .map_err(|error| error.to_string())
}

#[derive(Clone, Serialize)]
struct Arrival {
    id: String,
    account: String,
}

fn take_arrival(db: &Connection, account: &str) -> Result<Option<Arrival>, String> {
    if !preferences(db)?.enabled {
        return Ok(None);
    }
    let tx = db
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let id = tx.query_row(
        "SELECT m.id FROM notification_candidates n JOIN mail_groups m ON m.id=n.id
         WHERE n.account_id=?1 AND n.handled=0 AND m.folder='inbox' AND m.unread=1
         AND NOT EXISTS(SELECT 1 FROM snoozes WHERE id=m.id AND wake>?2)
         ORDER BY m.received DESC,m.id DESC LIMIT 1",
        params![account, engine::now()],
        |row| row.get(0),
    );
    use rusqlite::OptionalExtension;
    let id = id.optional().map_err(|error| error.to_string())?;
    // One banner per account per sync, even if several messages arrived.
    tx.execute(
        "UPDATE notification_candidates SET handled=1 WHERE account_id=?1 AND handled=0",
        [account],
    )
    .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(id.map(|id| Arrival {
        id,
        account: account.into(),
    }))
}

pub fn after_sync(app: &tauri::AppHandle, store: &Store, account: &str) {
    #[cfg(windows)]
    if let Ok(Some(arrival)) = providers::db(store, |db| take_arrival(db, account)) {
        let handle = app.clone();
        let store = store.clone();
        let _ = app.run_on_main_thread(move || {
            let Ok(Some((title, body))) = providers::db(&store, |db| {
                let value = preferences(db)?;
                if !value.enabled {
                    return Ok(None);
                }
                let mail = engine::get(db, &arrival.id)?;
                let eligible: bool = db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM notification_candidates WHERE id=?1)
                     AND NOT EXISTS(SELECT 1 FROM snoozes WHERE id=?1 AND wake>?2)",
                        params![arrival.id, engine::now()],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
                if mail.folder != "inbox" || !mail.unread || !eligible {
                    return Ok(None);
                }
                let title = if value.previews && !mail.sender.trim().is_empty() {
                    mail.sender
                } else {
                    "New mail".into()
                };
                let body = if value.previews && !mail.subject.trim().is_empty() {
                    mail.subject
                } else {
                    "Open Rusty Mail to read it.".into()
                };
                Ok(Some((title, body)))
            }) else {
                return;
            };
            if show(&handle, arrival, &title, &body).is_err() {
                use tauri::Emitter;
                let _ = handle.emit("mail-notification-unavailable", ());
            }
        });
    }
}

#[cfg(windows)]
#[derive(Default)]
pub struct WindowsToast(
    std::sync::Mutex<std::collections::VecDeque<windows::UI::Notifications::ToastNotification>>,
);

#[cfg(windows)]
fn show(
    app: &tauri::AppHandle,
    arrival: Arrival,
    title: &str,
    body: &str,
) -> windows::core::Result<()> {
    use tauri::{Emitter, Manager};
    use windows::{
        core::{IInspectable, HSTRING},
        Data::Xml::Dom::XmlDocument,
        Foundation::TypedEventHandler,
        UI::Notifications::{ToastNotification, ToastNotificationManager},
    };

    let xml = XmlDocument::new()?;
    xml.LoadXml(&HSTRING::from("<toast><visual><binding template=\"ToastGeneric\"><text/><text/></binding></visual></toast>"))?;
    let fields = xml.GetElementsByTagName(&HSTRING::from("text"))?;
    for (index, value) in [title, body].into_iter().enumerate() {
        let text: String = value
            .chars()
            .filter(|character| {
                !character.is_control() && !matches!(character, '\u{fffe}' | '\u{ffff}')
            })
            .take(180)
            .collect();
        let node = xml.CreateTextNode(&HSTRING::from(text))?;
        fields.Item(index as u32)?.AppendChild(&node)?;
    }
    let toast = ToastNotification::CreateToastNotification(&xml)?;
    let app_handle = app.clone();
    toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
        move |_, _| {
            let handle = app_handle.clone();
            let id = arrival.id.clone();
            let account = arrival.account.clone();
            let _ = app_handle.run_on_main_thread(move || {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                    let _ = window.emit("open-mail-notification", Arrival { id, account });
                }
            });
            Ok(())
        },
    ))?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
        app.config().identifier.as_str(),
    ))?;
    notifier.Show(&toast)?;
    let state = app.state::<WindowsToast>();
    if let Ok(mut active) = state.0.lock() {
        active.push_back(toast);
        if active.len() > 32 {
            active.pop_front();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str, account: &str, received: i64) -> engine::Mail {
        engine::Mail {
            id: id.into(),
            remote_id: id.into(),
            account_id: account.into(),
            folder: "inbox".into(),
            unread: true,
            received,
            ..Default::default()
        }
    }

    #[test]
    fn arrivals_exclude_backfill_and_counts_follow_local_changes() {
        let db = Connection::open_in_memory().unwrap();
        engine::migrate(&db).unwrap();
        db.execute_batch(
            "CREATE TABLE accounts(id TEXT PRIMARY KEY);
            INSERT INTO accounts VALUES('first'),('second');",
        )
        .unwrap();
        assert!(!preferences(&db).unwrap().enabled);
        engine::checkpoint(&db, "", "mail-notifications", r#"{"enabled":true}"#).unwrap();
        initialize(&db).unwrap();
        db.execute("UPDATE notification_session SET since=100", [])
            .unwrap();
        let mut new = message("new", "first", 103);
        let old = message("old", "first", 99);
        let other = message("other", "second", 102);
        for mail in [&old, &new, &other] {
            engine::put(&db, mail).unwrap();
        }
        assert_eq!(take_arrival(&db, "first").unwrap().unwrap().id, "new");
        assert!(take_arrival(&db, "first").unwrap().is_none());
        assert_eq!(take_arrival(&db, "second").unwrap().unwrap().id, "other");
        assert_eq!(counts(&db).unwrap()["first"], 2);
        new.unread = false;
        engine::put(&db, &new).unwrap();
        assert_eq!(counts(&db).unwrap()["first"], 1);
        db.execute("INSERT INTO snoozes VALUES('old',?1)", [engine::now() + 60])
            .unwrap();
        assert_eq!(counts(&db).unwrap()["first"], 0);
        assert_eq!(counts(&db).unwrap()["second"], 1);
        assert!(take_arrival(&db, "first").unwrap().is_none());
    }
}
