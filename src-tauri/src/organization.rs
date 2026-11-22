use crate::{engine, live::Runtime, providers, reconcile, Store};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::{sync::Arc, time::Duration};
use tauri::Emitter;

#[derive(Serialize)]
pub struct Mailbox {
    id: String,
    account_id: String,
    name: String,
    label: bool,
}

#[tauri::command]
pub fn list_mailboxes(store: tauri::State<Store>) -> Result<Vec<Mailbox>, String> {
    providers::db(&store, |db| {
        let mut query = db
            .prepare(
                "SELECT m.id,m.account_id,m.name,a.provider
             FROM mailboxes m JOIN accounts a ON a.id=m.account_id
             WHERE (a.provider='google' AND m.special='label')
                OR (a.provider='microsoft' AND m.special='custom')
                OR (a.provider='imap' AND m.special NOT IN ('inbox','drafts'))
             ORDER BY m.name COLLATE NOCASE,m.id",
            )
            .map_err(|error| error.to_string())?;
        let rows = query
            .query_map([], |row| {
                Ok(Mailbox {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    name: row.get(2)?,
                    label: row.get::<_, String>(3)? == "google",
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    })
}

#[tauri::command]
pub async fn organize_mail(
    app: tauri::AppHandle,
    store: tauri::State<'_, Store>,
    runtime: tauri::State<'_, Arc<Runtime>>,
    id: String,
    target: String,
    enabled: Option<bool>,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let (message, _sync, _organize) = loop {
        let sync = runtime.gate.lock().await;
        let organize = runtime.organize_gate.lock().await;
        let (message, conflict) = providers::db(&store, |db| {
            let message = engine::get(db, &id)?;
            let special: String = db
                .query_row(
                    "SELECT special FROM mailboxes WHERE account_id=?1 AND id=?2",
                    params![message.account_id, target],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?
                .ok_or("This folder or label is no longer available")?;
            let supported = match message.provider.as_str() {
                "google" => enabled.is_some() && special == "label",
                "microsoft" => enabled.is_none() && special == "custom",
                "imap" => enabled.is_none() && !matches!(special.as_str(), "inbox" | "drafts"),
                _ => false,
            };
            if !supported {
                return Err("This action is unavailable for the selected folder or label".into());
            }
            let conflict: Option<String> = db.query_row(
                "SELECT status FROM jobs WHERE kind='mutation'
                 AND json_extract(payload,'$.id')=?1 AND status IN ('pending','running','failed','uncertain')
                 AND (?2 OR json_extract(payload,'$.action') NOT IN ('read','unread','star','unstar'))
                 ORDER BY (status='failed') DESC LIMIT 1",
                params![id, enabled.is_none()],
                |row| row.get(0),
            ).optional().map_err(|error| error.to_string())?;
            Ok((message, conflict))
        })?;
        match conflict.as_deref() {
            None => break (message, sync, organize),
            Some("pending" | "running") if tokio::time::Instant::now() < deadline => {
                // Opening a message can queue a read flag just before a move.
                drop(organize);
                drop(sync);
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            _ => return Err(
                "Resolve the pending actions for this message in Mail operations, then try again."
                    .into(),
            ),
        }
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        if let Some(enabled) = enabled {
            providers::set_gmail_label(&store, &message, &target, enabled).await?;
            providers::db(&store, |db| {
                let tx = db
                    .unchecked_transaction()
                    .map_err(|error| error.to_string())?;
                let mut current = reconcile::mutation_target(&tx, &id)?;
                current.labels.retain(|label| label != &target);
                if enabled {
                    current.labels.push(target.clone());
                }
                reconcile::put_remote(&tx, &current)?;
                tx.commit().map_err(|error| error.to_string())
            })
        } else {
            if message.folder == format!("mailbox:{target}") {
                return Err("This message is already in that folder".into());
            }
            let moved = providers::move_to_mailbox(&store, &message, &target).await?;
            providers::db(&store, |db| {
                let tx = db
                    .unchecked_transaction()
                    .map_err(|error| error.to_string())?;
                match moved {
                    Some(moved) if moved.id == id => reconcile::put_remote(&tx, &moved)?,
                    Some(moved) => {
                        engine::put(&tx, &moved)?;
                        engine::remove(&tx, &id)?;
                    }
                    None => engine::remove(&tx, &id)?,
                }
                tx.commit().map_err(|error| error.to_string())
            })
        }
    })
    .await
    .map_err(|_| "The server did not confirm the change. Synchronize before retrying.")??;
    let _ = app.emit("mail-changed", ());
    if enabled.is_none() && message.provider == "imap" {
        let store = store.inner().clone();
        let runtime = runtime.inner().clone();
        tauri::async_runtime::spawn(async move {
            let _sync = runtime.gate.lock().await;
            let refresh = providers::sync_imap_mailbox(&store, &message.account_id, &target);
            if matches!(
                tokio::time::timeout(Duration::from_secs(30), refresh).await,
                Ok(Ok(_))
            ) {
                let _ = app.emit("mail-changed", ());
            }
        });
    }
    Ok(())
}
