//! Optional signed release updates. No endpoint or key is accepted from the webview.
use serde::Serialize;
use tauri::{AppHandle, State};
#[cfg(feature = "signed-updates")]
use tauri_plugin_updater::{Update, UpdaterExt};
#[derive(Default)]
pub struct PendingUpdate {
    #[cfg(feature = "signed-updates")]
    update: std::sync::Mutex<Option<Update>>,
}
#[derive(Serialize)]
pub struct Offer {
    pub enabled: bool,
    pub version: Option<String>,
    pub notes: Option<String>,
}
#[tauri::command]
pub async fn check_updates(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<Offer, String> {
    #[cfg(feature = "signed-updates")]
    {
        let update = app
            .updater_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|_| "Invalid signed-update configuration")?
            .check()
            .await
            .map_err(|_| "Update check failed. Check connectivity and release metadata.")?;
        let offer = Offer {
            enabled: true,
            version: update.as_ref().map(|u| u.version.clone()),
            notes: update.as_ref().and_then(|u| u.body.clone()),
        };
        *pending
            .update
            .lock()
            .map_err(|_| "Update state unavailable")? = update;
        Ok(offer)
    }
    #[cfg(not(feature = "signed-updates"))]
    {
        let _ = (app, pending);
        Ok(Offer {
            enabled: false,
            version: None,
            notes: None,
        })
    }
}
#[tauri::command]
pub async fn install_update(
    pending: State<'_, PendingUpdate>,
    runtime: State<'_, std::sync::Arc<crate::live::Runtime>>,
    store: State<'_, crate::Store>,
    version: String,
) -> Result<(), String> {
    #[cfg(feature = "signed-updates")]
    {
        let _guard = runtime
            .gate
            .try_lock()
            .map_err(|_| "Finish the active mail operation before installing an update")?;
        let queued = crate::providers::db(&store, |d| {
            d.query_row(
                "SELECT count(*) FROM jobs WHERE status IN ('pending','running')",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())
        })?;
        if queued > 0 {
            return Err(
                "Finish or cancel pending mail operations before installing an update".into(),
            );
        }
        let update = pending
            .update
            .lock()
            .map_err(|_| "Update state unavailable")?
            .take()
            .ok_or("Check for updates first")?;
        if update.version != version {
            return Err("The update offer changed. Check for updates again.".into());
        }
        // The official plugin verifies the pinned minisign key and signed version before executing.
        update
            .download_and_install(|_, _| {}, || {})
            .await
            .map_err(|_| "Update failed signature verification, download, or installation")?;
        Ok(())
    }
    #[cfg(not(feature = "signed-updates"))]
    {
        let _ = (pending, runtime, store, version);
        Err("This development build has no signed update channel".into())
    }
}
