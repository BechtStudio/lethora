//! Updates (Help → Check for updates…, and quietly a few seconds after
//! start): the latest GitHub release's `latest.json` (the endpoint and the
//! public key are in tauri.conf.json), fetched and downloaded here, so the
//! web view's CSP stays closed. The download must carry a signature by the
//! release key. Installing saves the open project as closing the window
//! does, then the app restarts (on Windows the installer runs passive,
//! exits the app and starts it again); the start reopens the project.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::settings::SettingsState;
use crate::{AppState, close_project, message};

#[derive(Default)]
pub struct Updates {
    /// What the last check found, for `update_install`.
    found: Mutex<Option<Update>>,
    /// The quiet check after start ran (once per run, also over web view
    /// reloads).
    started: AtomicBool,
}

impl Updates {
    fn found(&self) -> MutexGuard<'_, Option<Update>> {
        self.found.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A newer version, for the update dialog.
#[derive(Serialize)]
pub struct UpdateInfo {
    version: String,
    /// The release notes, plain text.
    notes: String,
    /// The release date, `YYYY-MM-DD`.
    date: Option<String>,
}

/// `update-progress`: bytes downloaded so far, of `total` if known.
#[derive(Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

/// Looks for a newer version. `manual: false` is the quiet check after
/// start: once per run, only with Check for updates on start, and never in a
/// debug build (whose version the release doesn't match).
#[tauri::command]
pub async fn update_check(
    manual: bool,
    app: AppHandle,
    updates: State<'_, Updates>,
    settings: State<'_, SettingsState>,
) -> Result<Option<UpdateInfo>, String> {
    if !manual
        && (updates.started.swap(true, Ordering::SeqCst)
            || cfg!(debug_assertions)
            || !settings.get().check_updates_on_start)
    {
        return Ok(None);
    }
    let handle = app.clone();
    let updater = app
        .updater_builder()
        // Windows: the installer starts and the app exits at once, without
        // the exit event that saves the project otherwise.
        .on_before_exit(move || close_project(&handle))
        .build()
        .map_err(|e| format!("Could not check for updates: {e}"))?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("Could not check for updates: {e}"))?;
    let info = update.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        notes: u.body.clone().unwrap_or_default(),
        date: u.date.map(|d| d.date().to_string()),
    });
    *updates.found() = update;
    Ok(info)
}

/// Downloads the update the last check found (`update-progress` events),
/// saves the open project, installs and restarts. Returns only on errors.
#[tauri::command]
pub async fn update_install(app: AppHandle, updates: State<'_, Updates>) -> Result<(), String> {
    let update = updates
        .found()
        .clone()
        .ok_or("No update to install. Check for updates again.")?;
    let mut downloaded = 0u64;
    let mut shown = None;
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                // Each percent, or each MiB without a size.
                let step = total.map_or(downloaded >> 20, |t| downloaded * 100 / t.max(1));
                if shown != Some(step) {
                    shown = Some(step);
                    let _ = app.emit("update-progress", Progress { downloaded, total });
                }
            },
            || {},
        )
        .await
        .map_err(|e| format!("Could not download the update: {e}"))?;
    save_project(&app)?;
    update
        .install(bytes)
        .map_err(|e| format!("Could not install the update: {e}"))?;
    app.restart()
}

/// Saves the open project before installing; the exit saves it again.
fn save_project(app: &AppHandle) -> Result<(), String> {
    if let Some(project) = app.state::<AppState>().lock().as_mut() {
        project.save().map_err(|e| {
            format!(
                "Saving failed, so the update was not installed: {}",
                message(e)
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    /// A broken updater section would stop the app at start.
    #[test]
    fn the_updater_config_parses() {
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let config: tauri_plugin_updater::Config =
            serde_json::from_value(conf["plugins"]["updater"].clone()).unwrap();
        assert_eq!(
            config.endpoints[0].as_str(),
            "https://github.com/BechtStudio/lethora/releases/latest/download/latest.json"
        );
        assert!(!config.pubkey.is_empty());
        // No signed updater files in a local build (see release.yml).
        assert!(conf["bundle"].get("createUpdaterArtifacts").is_none());
    }
}
