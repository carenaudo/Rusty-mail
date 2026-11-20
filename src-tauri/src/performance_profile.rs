//! Explicit synthetic-only profiling; skips credential loading and background sync.
use std::{path::PathBuf, sync::Mutex, time::Instant};
pub struct Profile {
    pub path: PathBuf,
    started: Instant,
}
pub struct ProfileState(pub Mutex<Option<Profile>>);
impl Profile {
    pub fn from_args() -> Result<Option<Self>, String> {
        let args: Vec<_> = std::env::args_os().collect();
        let Some(index) = args
            .iter()
            .position(|a| a == "--synthetic-performance-profile")
        else {
            return Ok(None);
        };
        let started = Instant::now();
        let path = PathBuf::from(args.get(index + 1).ok_or("Missing fixture path")?)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        validate_fixture(&path)?;
        Ok(Some(Self { path, started }))
    }
}
fn validate_fixture(path: &std::path::Path) -> Result<(), String> {
    let db =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
    db.pragma_update(None, "key", format!("x'{}'", "ba".repeat(32)))
        .map_err(|e| e.to_string())?;
    let marker: String = db.query_row("SELECT value FROM sync_state WHERE account_id='synthetic-benchmark' AND key='conversation-fixture-v1'", [], |r| r.get(0)).map_err(|_| "Not a synthetic conversation fixture")?;
    let count: i64 = db
        .query_row("SELECT count(*) FROM mail", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let foreign: i64 = db.query_row("SELECT count(*) FROM mail WHERE account_id NOT IN ('account-0','account-1','account-2')",[],|r|r.get(0)).map_err(|e|e.to_string())?;
    if marker != "1" || ![100_000, 500_000].contains(&count) || foreign != 0 {
        return Err("Not an approved synthetic fixture".into());
    }
    Ok(())
}
#[tauri::command]
pub fn synthetic_profile_ready(
    state: tauri::State<ProfileState>,
    rows: usize,
) -> Result<(), String> {
    let mut profile = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(p) = profile.take() {
        let result = serde_json::json!({"startup_to_painted_list_ms":p.started.elapsed().as_secs_f64()*1000.0,"rows":rows,"background_sync":false,"synthetic":true});
        std::fs::write(
            p.path.with_extension("startup.json"),
            serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiling_rejects_other_stores_without_modifying_them() {
        let path =
            std::env::temp_dir().join(format!("rustymail-profile-{}.db", rand::random::<u64>()));
        assert!(validate_fixture(&path).is_err());
        assert!(!path.exists(), "Read-only validation must not create files");
        {
            let db = crate::open_store(&path, &"ba".repeat(32)).unwrap();
            crate::engine::migrate(&db).unwrap();
            assert!(validate_fixture(&path).is_err());
            crate::engine::checkpoint(&db, "synthetic-benchmark", "conversation-fixture-v1", "1")
                .unwrap();
            assert!(
                validate_fixture(&path).is_err(),
                "Marker alone is insufficient"
            );
        }
        let before = std::fs::read(&path).unwrap();
        assert!(validate_fixture(&path).is_err());
        assert_eq!(before, std::fs::read(&path).unwrap());
        std::fs::remove_file(path).unwrap();
    }
}
