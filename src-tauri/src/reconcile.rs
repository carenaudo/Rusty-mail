use crate::engine::{self, Mail};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct ProviderState {
    unread: bool,
    starred: bool,
    folder: String,
    labels: Vec<String>,
}

pub fn field(action: &str) -> &str {
    match action {
        "read" | "unread" => "unread",
        "star" | "unstar" => "starred",
        _ => "folder",
    }
}

pub fn apply(mail: &mut Mail, action: &str) {
    match action {
        "read" => mail.unread = false,
        "unread" => mail.unread = true,
        "star" => mail.starred = true,
        "unstar" => mail.starred = false,
        folder => {
            mail.folder = folder.into();
            if mail.provider == "google" {
                mail.labels
                    .retain(|label| !matches!(label.as_str(), "INBOX" | "TRASH" | "SPAM"));
                match folder {
                    "inbox" => mail.labels.push("INBOX".into()),
                    "trash" => mail.labels.push("TRASH".into()),
                    "spam" => mail.labels.push("SPAM".into()),
                    _ => {}
                }
            } else {
                mail.labels.clear();
                if mail.provider == "microsoft" {
                    mail.labels
                        .push(folder.strip_prefix("mailbox:").unwrap_or(folder).into());
                }
            }
        }
    }
}

fn state(mail: &Mail) -> Result<String, String> {
    serde_json::to_string(&ProviderState {
        unread: mail.unread,
        starred: mail.starred,
        folder: mail.folder.clone(),
        labels: mail.labels.clone(),
    })
    .map_err(|e| e.to_string())
}

pub fn remember(db: &Connection, mail: &Mail) -> Result<(), String> {
    db.execute(
        "INSERT OR IGNORE INTO mutation_base(id,state) VALUES(?1,?2)",
        params![mail.id, state(mail)?],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn tracked(db: &Connection, id: &str) -> Result<Option<(Option<String>, bool)>, String> {
    db.query_row(
        "SELECT state,missing FROM mutation_base WHERE id=?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn restore(mail: &mut Mail, state: &str) -> Result<(), String> {
    let state: ProviderState = serde_json::from_str(state).map_err(|e| e.to_string())?;
    mail.unread = state.unread;
    mail.starred = state.starred;
    mail.folder = state.folder;
    mail.labels = state.labels;
    Ok(())
}

fn overlay(db: &Connection, mail: &mut Mail) -> Result<(), String> {
    let mut query = db
        .prepare(
            "SELECT json_extract(payload,'$.action') FROM jobs
         WHERE kind='mutation' AND account_id=?1 AND json_extract(payload,'$.id')=?2
           AND status IN ('pending','running') ORDER BY rowid",
        )
        .map_err(|e| e.to_string())?;
    let actions = query
        .query_map(params![mail.account_id, mail.id], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| e.to_string())?;
    for action in actions {
        apply(mail, &action.map_err(|e| e.to_string())?);
    }
    Ok(())
}

pub fn put_remote(db: &Connection, mail: &Mail) -> Result<(), String> {
    if tracked(db, &mail.id)?.is_none() {
        return engine::put(db, mail);
    }
    db.execute(
        "UPDATE mutation_base SET state=?2,missing=0 WHERE id=?1",
        params![mail.id, state(mail)?],
    )
    .map_err(|e| e.to_string())?;
    let mut projected = mail.clone();
    overlay(db, &mut projected)?;
    engine::put(db, &projected)
}

fn protected(db: &Connection, id: &str) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE kind='mutation' AND json_extract(payload,'$.id')=?1
           AND status IN ('pending','running','uncertain'))",
        [id],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

fn remove_missing(db: &Connection, id: &str) -> Result<(), String> {
    if protected(db, id)? {
        db.execute(
            "INSERT INTO mutation_base(id,missing) VALUES(?1,1)
             ON CONFLICT(id) DO UPDATE SET missing=1",
            [id],
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }
    engine::remove(db, id)
}

pub fn remove_remote(db: &Connection, id: &str, folder: Option<&str>) -> Result<(), String> {
    if let Some(folder) = folder {
        let matches: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM mail m LEFT JOIN mutation_base b ON b.id=m.id
             WHERE m.id=?1 AND ((b.id IS NULL AND m.folder=?2) OR json_extract(b.state,'$.folder')=?2))",
                params![id, folder],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !matches {
            return Ok(());
        }
    }
    remove_missing(db, id)
}

pub fn reproject(db: &Connection, id: &str) -> Result<(), String> {
    let Some((base, missing)) = tracked(db, id)? else {
        return Ok(());
    };
    if missing && !protected(db, id)? {
        return engine::remove(db, id);
    }
    if let Some(base) = base {
        let mut mail = engine::get(db, id)?;
        let before = state(&mail)?;
        restore(&mut mail, &base)?;
        overlay(db, &mut mail)?;
        if state(&mail)? != before {
            engine::put(db, &mail)?;
        }
    }
    db.execute(
        "DELETE FROM mutation_base WHERE id=?1 AND NOT EXISTS (
           SELECT 1 FROM jobs WHERE kind='mutation' AND json_extract(payload,'$.id')=?1
           AND status IN ('failed','uncertain','pending','running'))",
        [id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn confirm_action(
    db: &Connection,
    id: &str,
    action: &str,
    gmail_labels: Option<Vec<String>>,
) -> Result<(), String> {
    let Some((base, _)) = tracked(db, id)? else {
        return Ok(());
    };
    let mut mail = engine::get(db, id)?;
    if let Some(labels) = gmail_labels {
        mail.folder = crate::providers::gmail_folder(&labels).into();
        mail.unread = labels.iter().any(|label| label == "UNREAD");
        mail.starred = labels.iter().any(|label| label == "STARRED");
        mail.labels = labels;
    } else if let Some(base) = base {
        restore(&mut mail, &base)?;
        apply(&mut mail, action);
    } else {
        return Ok(());
    }
    db.execute(
        "UPDATE mutation_base SET state=?2 WHERE id=?1",
        params![id, state(&mail)?],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn can_retry(db: &Connection, id: &str) -> Result<bool, String> {
    Ok(matches!(tracked(db, id)?, Some((Some(_), false))))
}

pub fn mutation_target(db: &Connection, id: &str) -> Result<Mail, String> {
    if matches!(tracked(db, id)?, Some((_, true))) {
        return Err("The message is no longer in its confirmed mailbox".into());
    }
    review_target(db, id)
}

pub fn review_target(db: &Connection, id: &str) -> Result<Mail, String> {
    let mut mail = engine::get(db, id)?;
    if let Some((Some(base), _)) = tracked(db, id)? {
        restore(&mut mail, &base)?;
    }
    Ok(mail)
}

pub fn recover_running(db: &Connection) -> Result<(), String> {
    let mut query = db
        .prepare(
            "UPDATE jobs SET status='uncertain',
         detail='App stopped during a mailbox action. Synchronize and review its state.'
         WHERE kind='mutation' AND status='running' RETURNING json_extract(payload,'$.id')",
        )
        .map_err(|e| e.to_string())?;
    let ids = query
        .query_map([], |row| row.get::<_, Option<String>>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(query);
    for id in ids.into_iter().flatten() {
        reproject(db, &id)?;
    }
    Ok(())
}

pub fn rebuilding(db: &Connection, account: &str, scope: &str) -> Result<bool, String> {
    Ok(engine::state(db, account, &format!("rebuild:{scope}"))?.is_some())
}

pub fn reset_rebuild(db: &Connection, account: &str, scope: &str) -> Result<(), String> {
    db.execute(
        "DELETE FROM rebuild_seen WHERE account_id=?1 AND scope=?2",
        params![account, scope],
    )
    .map_err(|e| e.to_string())?;
    engine::checkpoint(db, account, &format!("rebuild:{scope}"), "1")
}

pub fn observe(db: &Connection, account: &str, scope: &str, id: &str) -> Result<(), String> {
    db.execute(
        "INSERT OR IGNORE INTO rebuild_seen VALUES(?1,?2,?3)",
        params![account, scope, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn finish_rebuild(
    db: &Transaction<'_>,
    account: &str,
    scope: &str,
    folder: Option<&str>,
) -> Result<(), String> {
    let mut query = db.prepare(
        "SELECT m.id FROM mail m LEFT JOIN mutation_base b ON b.id=m.id
         WHERE m.account_id=?1 AND (?3 IS NULL OR (b.id IS NULL AND m.folder=?3)
           OR json_extract(b.state,'$.folder')=?3)
           AND NOT EXISTS (SELECT 1 FROM rebuild_seen s WHERE s.account_id=?1 AND s.scope=?2 AND s.id=m.id)",
    ).map_err(|e| e.to_string())?;
    let ids = query
        .query_map(params![account, scope, folder], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(query);
    for id in ids {
        remove_missing(db, &id)?;
    }
    db.execute(
        "DELETE FROM rebuild_seen WHERE account_id=?1 AND scope=?2",
        params![account, scope],
    )
    .map_err(|e| e.to_string())?;
    db.execute(
        "DELETE FROM sync_state WHERE account_id=?1 AND key=?2",
        params![account, format!("rebuild:{scope}")],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{live, Store};
    use std::sync::{Arc, Mutex};

    fn mail(id: &str, account: &str, folder: &str) -> Mail {
        Mail {
            id: id.into(),
            remote_id: id.into(),
            account_id: account.into(),
            folder: folder.into(),
            provider: "microsoft".into(),
            ..Default::default()
        }
    }

    #[test]
    fn interrupted_or_restarted_rebuild_keeps_cache_and_prunes_only_its_scope() {
        let db = Connection::open_in_memory().unwrap();
        engine::migrate(&db).unwrap();
        let store = Store(Arc::new(Mutex::new(db)));
        {
            let db = store.0.lock().unwrap();
            for (id, account, folder) in [
                ("kept", "account", "inbox"),
                ("stale", "account", "inbox"),
                ("protected", "account", "inbox"),
                ("moved", "account", "inbox"),
                ("sent", "account", "sent"),
                ("unrelated", "other", "inbox"),
            ] {
                engine::put(&db, &mail(id, account, folder)).unwrap();
            }
        }
        let pending = live::queue_mutation_inner(&store, "protected", "archive").unwrap();
        {
            let db = store.0.lock().unwrap();
            let (account, scope) = ("account", "delta:inbox");
            let page = db.unchecked_transaction().unwrap();
            reset_rebuild(&page, account, scope).unwrap();
            observe(&page, account, scope, "stale").unwrap();
            engine::checkpoint(&page, account, scope, "old-next").unwrap();
            page.commit().unwrap();

            let page = db.unchecked_transaction().unwrap();
            reset_rebuild(&page, account, scope).unwrap();
            observe(&page, account, scope, "kept").unwrap();
            engine::checkpoint(&page, account, scope, "new-next").unwrap();
            page.commit().unwrap();
            engine::migrate(&db).unwrap();
            assert!(rebuilding(&db, account, scope).unwrap());
            assert_eq!(
                engine::state(&db, account, scope).unwrap().as_deref(),
                Some("new-next")
            );
            assert!(engine::get(&db, "stale").is_ok());

            put_remote(&db, &mail("moved", account, "sent")).unwrap();
            let page = db.unchecked_transaction().unwrap();
            finish_rebuild(&page, account, scope, Some("inbox")).unwrap();
            page.rollback().unwrap();
            assert!(engine::get(&db, "stale").is_ok());
            assert!(rebuilding(&db, account, scope).unwrap());

            let page = db.unchecked_transaction().unwrap();
            finish_rebuild(&page, account, scope, Some("inbox")).unwrap();
            page.commit().unwrap();
            assert!(engine::get(&db, "stale").is_err());
            assert!(!rebuilding(&db, account, scope).unwrap());
            for id in ["kept", "protected", "moved", "sent", "unrelated"] {
                assert!(engine::get(&db, id).is_ok(), "{id}");
            }
        }
        live::cancel_job_inner(&store, &pending).unwrap();
        assert!(engine::get(&store.0.lock().unwrap(), "protected").is_err());
    }

    #[test]
    fn legacy_failed_projection_is_preserved_until_a_provider_observation() {
        let db = Connection::open_in_memory().unwrap();
        engine::migrate(&db).unwrap();
        let cached = mail("legacy", "account", "archive");
        engine::put(&db, &cached).unwrap();
        db.execute_batch(
            r#"DROP TRIGGER mutation_base_remove;
             DROP TABLE mutation_base;
             DROP TABLE rebuild_seen;
             INSERT INTO jobs VALUES('old','account','mutation','{"id":"legacy","action":"archive"}','failed','',0,0);
             PRAGMA user_version=13;"#,
        ).unwrap();
        engine::migrate(&db).unwrap();
        assert_eq!(engine::get(&db, "legacy").unwrap().folder, "archive");
        assert!(!can_retry(&db, "legacy").unwrap());
        put_remote(&db, &mail("legacy", "account", "inbox")).unwrap();
        assert_eq!(engine::get(&db, "legacy").unwrap().folder, "inbox");
        assert!(can_retry(&db, "legacy").unwrap());
    }
}
