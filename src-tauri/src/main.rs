#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#[cfg(test)]
mod acceptance;
mod ai;
mod auth;
mod compose;
mod drafts;
mod engine;
#[cfg(test)]
mod hardening_tests;
mod live;
mod mail_html;
#[cfg(test)]
mod mail_performance;
mod notifications;
mod organization;
mod outgoing;
mod providers;
mod reconcile;
#[cfg(test)]
mod threading_tests;
mod updates;
use rand::RngCore;
use rusqlite::Connection;

use std::sync::{Arc, Mutex};
use tauri::Manager;

#[derive(Clone)]
struct Store(Arc<Mutex<Connection>>);
fn open_store(path: &std::path::Path, key: &str) -> Result<Connection, String> {
    let db = Connection::open(path).map_err(|e| e.to_string())?;
    if key.len() != 64 || !key.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid storage key".into());
    }
    db.pragma_update(None, "key", format!("x'{key}'"))
        .map_err(|e| e.to_string())?;
    let cipher: String = db
        .query_row("PRAGMA cipher_version", [], |r| r.get(0))
        .map_err(|_| "SQLCipher is required; refusing plaintext storage")?;
    if cipher.is_empty() {
        return Err("SQLCipher unavailable".into());
    }
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA temp_store=MEMORY; PRAGMA cache_size=-32768; CREATE TABLE IF NOT EXISTS messages(id TEXT PRIMARY KEY, payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS drafts(id INTEGER PRIMARY KEY CHECK(id=1), recipient TEXT NOT NULL, subject TEXT NOT NULL, body TEXT NOT NULL);").map_err(|e|e.to_string())?;
    Ok(db)
}
mod performance_profile;
fn main() {
    let profile =
        performance_profile::Profile::from_args().expect("Invalid synthetic performance profile");
    let isolated = profile.is_some();
    let mut builder = tauri::Builder::default();
    if !isolated {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }));
    }
    builder.setup(move |app| {
            #[cfg(feature="signed-updates")]
            app.handle().plugin(tauri_plugin_updater::Builder::new().build())?;
            app.manage(updates::PendingUpdate::default());
            #[cfg(windows)]
            app.manage(notifications::WindowsToast::default());
            let db = if let Some(profile) = &profile {
                open_store(&profile.path, &"ba".repeat(32)).map_err(std::io::Error::other)?
            } else {
            let entry = keyring::Entry::new("dev.rustymail.desktop", "database-key")?;
            let key = match entry.get_password() {
                Ok(k) => k,
                Err(keyring::Error::NoEntry) => {
                    let mut bytes = [0u8; 32];
                    rand::thread_rng().fill_bytes(&mut bytes);
                    let k = hex::encode(bytes);
                    entry.set_password(&k)?;
                    k
                }
                Err(e) => return Err(e.into()),
            };
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            open_store(&dir.join("mail.db"), &key).map_err(std::io::Error::other)?
            };
            db.execute_batch("CREATE TABLE IF NOT EXISTS accounts(id TEXT PRIMARY KEY, provider TEXT NOT NULL, email TEXT NOT NULL)")?;
            engine::migrate(&db).map_err(std::io::Error::other)?;
            outgoing::startup_cleanup(&db).map_err(std::io::Error::other)?;
            notifications::initialize(&db).map_err(std::io::Error::other)?;
            if profile.is_some() {
                db.execute("DELETE FROM accounts", [])?;
                for n in 0..3 { db.execute("INSERT INTO accounts VALUES(?1,'synthetic',?2)", rusqlite::params![format!("account-{n}"),format!("synthetic{n}@example.invalid")])?; }
                if let Some(window)=app.get_webview_window("main") { window.set_title("Rusty Mail — synthetic performance profile")?; }
            }
            app.manage(performance_profile::ProfileState(std::sync::Mutex::new(profile)));
            let store=Store(Arc::new(Mutex::new(db)));
            let runtime=Arc::new(live::Runtime::default());
            app.manage(store.clone()); app.manage(runtime.clone()); app.manage(ai::AiRuntime::default());
            if !isolated { live::start_worker(app.handle().clone(),store,runtime); }
            app.manage(auth::AuthState::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            performance_profile::synthetic_profile_ready,auth::sign_in,
            auth::cancel_sign_in,
            auth::list_accounts, notifications::unread_counts, notifications::notification_preferences, notifications::set_notification_preferences,
            updates::check_updates,updates::install_update,
            mail_html::load_message_images,mail_html::open_message_link,compose::prepare_compose,compose::copy_forward_attachments,compose::get_account_aliases,compose::save_account_aliases,compose::get_signatures,compose::set_signature,compose::recent_recipients,organization::list_mailboxes,organization::organize_mail,live::get_conversation,live::query_mail,live::sync_account,live::get_mail,live::connect_imap,live::account_status,live::queue_mutation,live::queue_bulk_mutation,live::snooze_mail,live::next_snooze_wake,drafts::save_live_draft,drafts::load_live_draft,drafts::list_live_drafts,drafts::discard_live_draft,live::queue_send,live::list_jobs,live::list_active_jobs,live::cancel_job,
            live::outgoing_attachments,outgoing::release_outgoing_attachments,live::resolve_send,live::retry_mutation,live::review_mutation,live::pick_attachments,live::save_attachment,
            live::list_sync_statuses,
            live::message_snooze_wake,live::unsnooze_mail,
            ai::ai_info,ai::ai_consent,ai::cancel_ai,ai::run_ai
        ])
        .run(tauri::generate_context!())
        .expect("Rusty Mail could not start");
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_roundtrip() {
        let path =
            std::env::temp_dir().join(format!("rustymail-test-{}.db", rand::random::<u64>()));
        let key = "ab".repeat(32);
        {
            let db = open_store(&path, &key).unwrap();
            db.execute("INSERT INTO drafts VALUES(1,'private@example.com','secret subject','private body')",[]).unwrap();
        }
        assert!(!std::fs::read(&path)
            .unwrap()
            .windows(12)
            .any(|w| w == b"private body"));
        assert!(open_store(&path, &"cd".repeat(32)).is_err());
        let db = open_store(&path, &key).unwrap();
        let body: String = db
            .query_row("SELECT body FROM drafts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(body, "private body");
        drop(db);
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn invalid_key_rejected() {
        assert!(open_store(std::path::Path::new(":memory:"), "bad").is_err())
    }
}
