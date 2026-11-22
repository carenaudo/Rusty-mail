use crate::{engine, providers, Store};
use rusqlite::{params, Connection};

pub fn validate_draft_id(id: &str) -> Result<(), String> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid draft ID".into());
    }
    Ok(())
}

pub fn store_file(
    db: &Connection,
    account: &str,
    draft: Option<&str>,
    name: String,
    mime: String,
    data: Vec<u8>,
) -> Result<engine::Attachment, String> {
    let id = hex::encode(rand::random::<[u8; 16]>());
    db.execute(
        "INSERT INTO files VALUES(?1,'outgoing',?2,?3,?4,?5)",
        params![id, name, mime, data, engine::now()],
    )
    .map_err(|e| e.to_string())?;
    db.execute(
        "INSERT INTO outgoing_owners VALUES(?1,?2)",
        params![id, account],
    )
    .map_err(|e| e.to_string())?;
    if let Some(draft) = draft {
        db.execute(
            "INSERT INTO outgoing_staged VALUES(?1,?2)",
            params![id, draft],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(engine::Attachment {
        id,
        name,
        mime,
        size: data.len(),
    })
}

// References are evaluated once for the complete batch. Pending, running,
// and uncertain sends retain their copies even if they lack a saved draft.
fn reclaim(db: &Connection, candidates: Option<&[String]>) -> Result<(), String> {
    let ids = candidates
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| e.to_string())?;
    db.execute(
        "WITH referenced(id) AS MATERIALIZED (
            SELECT f.value FROM saved_drafts d, json_each(CASE WHEN json_valid(d.payload) THEN d.payload ELSE '{}' END,'$.attachments') f
            UNION
            SELECT f.value FROM jobs j, json_each(CASE WHEN json_valid(j.payload) THEN j.payload ELSE '{}' END,'$.attachments') f
            WHERE j.kind='send' AND j.status IN ('pending','running','uncertain')
            UNION SELECT file_id FROM outgoing_staged
         ), candidates(id) AS (
            SELECT value FROM json_each(?1)
            UNION ALL SELECT id FROM files WHERE ?1 IS NULL AND message_id='outgoing'
         )
         DELETE FROM files WHERE message_id='outgoing'
           AND id IN (SELECT id FROM candidates EXCEPT SELECT id FROM referenced)", [ids],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn reclaim_files(db: &Connection, ids: &[String]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    reclaim(db, Some(ids))
}

pub fn startup_cleanup(db: &Connection) -> Result<(), String> {
    let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM outgoing_staged", [])
        .map_err(|e| e.to_string())?;
    reclaim(&tx, None)?;
    tx.execute(
        "DELETE FROM outgoing_owners WHERE NOT EXISTS(SELECT 1 FROM files WHERE id=file_id)",
        [],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub fn staged_files(db: &Connection, draft: &str) -> Result<Vec<String>, String> {
    let mut query = db
        .prepare("SELECT file_id FROM outgoing_staged WHERE draft_id=?1")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map([draft], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

pub fn unstage_files(
    db: &Connection,
    draft: &str,
    files: &[String],
) -> Result<Vec<String>, String> {
    let mut released = Vec::new();
    if files.is_empty() {
        return Ok(released);
    }
    let mut delete = db
        .prepare_cached("DELETE FROM outgoing_staged WHERE draft_id=?1 AND file_id=?2")
        .map_err(|e| e.to_string())?;
    for file in files {
        if delete
            .execute(params![draft, file])
            .map_err(|e| e.to_string())?
            != 0
        {
            released.push(file.clone());
        }
    }
    Ok(released)
}

pub fn release_stage(db: &Connection, draft: &str, files: &[String]) -> Result<(), String> {
    reclaim_files(db, &unstage_files(db, draft, files)?)
}

#[tauri::command]
pub fn release_outgoing_attachments(
    store: tauri::State<Store>,
    draft_id: String,
    ids: Vec<String>,
) -> Result<(), String> {
    validate_draft_id(&draft_id)?;
    providers::db(&store, |db| {
        let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
        release_stage(&tx, &draft_id, &ids)?;
        tx.commit().map_err(|e| e.to_string())
    })
}

pub fn payload_files(payload: &str) -> Vec<String> {
    serde_json::from_str::<engine::Draft>(payload)
        .map(|draft| draft.attachments)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_retains_editing_and_active_send_references_and_ignores_incoming_files() {
        let db = Connection::open_in_memory().unwrap();
        engine::migrate(&db).unwrap();
        let open = store_file(
            &db,
            "a",
            Some("open"),
            "local".into(),
            "text/plain".into(),
            vec![1],
        )
        .unwrap();
        let orphan = store_file(
            &db,
            "a",
            None,
            "orphan".into(),
            "text/plain".into(),
            vec![2],
        )
        .unwrap();
        let send = store_file(&db, "a", None, "send".into(), "text/plain".into(), vec![3]).unwrap();
        let payload = serde_json::json!({"attachments": [&send.id]}).to_string();
        db.execute(
            "INSERT INTO jobs VALUES('job','a','send',?1,'uncertain','',0,0)",
            [&payload],
        )
        .unwrap();
        db.execute(
            "INSERT INTO files VALUES('incoming','message','source','text/plain',X'01',0)",
            [],
        )
        .unwrap();
        let exists = |id: &str| -> bool {
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE id=?1)",
                [id],
                |row| row.get(0),
            )
            .unwrap()
        };
        release_stage(&db, "some-other-draft", std::slice::from_ref(&open.id)).unwrap();
        assert!(exists(&open.id));
        reclaim_files(&db, &[open.id.clone(), send.id.clone(), "incoming".into()]).unwrap();
        assert!(exists(&open.id) && exists(&send.id) && exists("incoming"));
        startup_cleanup(&db).unwrap();
        assert!(!exists(&open.id) && !exists(&orphan.id));
        assert!(exists(&send.id) && exists("incoming"));

        let tx = db.unchecked_transaction().unwrap();
        tx.execute("UPDATE jobs SET status='reviewed' WHERE id='job'", [])
            .unwrap();
        reclaim_files(&tx, std::slice::from_ref(&send.id)).unwrap();
        tx.rollback().unwrap();
        assert!(exists(&send.id));
        db.execute("UPDATE jobs SET status='reviewed' WHERE id='job'", [])
            .unwrap();
        reclaim_files(&db, std::slice::from_ref(&send.id)).unwrap();
        assert!(!exists(&send.id));
    }
}
