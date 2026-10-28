//! `settings.json` in the app config folder: app-wide preferences (not a
//! project's), next to `recent_projects.json`. A missing field takes its
//! default, an unreadable file gives the defaults, and fields this version
//! doesn't know (from a newer one) are kept when it is written. The defaults
//! are only here: the Settings view restores one by sending `null`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Runtime, State};

use crate::write_atomic;

const SETTINGS: &str = "settings.json";

/// Appearance → Theme: the system's, or always light or dark.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// General: reopen the last project on start.
    pub reopen_last_project: bool,
    /// Appearance: every window's theme (the web views' `prefers-color-scheme`).
    pub theme: Theme,
    /// Editor (author and science views): the text size, CSS px.
    pub editor_text_size: f64,
    /// Editor: the line height, a multiple of the text size.
    pub editor_line_spacing: f64,
    /// Editor: a blank line's height (the paragraph gap), em.
    pub editor_paragraph_gap: f64,
    /// Editor, also View → Check spelling.
    pub spell_check: bool,
    /// Updates, also Help → Check for updates on start.
    pub check_updates_on_start: bool,
    /// Keyboard shortcuts: only the commands changed from their defaults
    /// (command id → keys as "Ctrl+Shift+S"; an empty list = no shortcut).
    /// Ids this version doesn't know are kept and ignored.
    pub shortcuts: BTreeMap<String, Vec<String>>,
    /// Fields this version doesn't know, written back unchanged.
    #[serde(flatten)]
    other: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            reopen_last_project: true,
            theme: Theme::System,
            editor_text_size: 19.5,
            editor_line_spacing: 1.65,
            editor_paragraph_gap: 0.8,
            spell_check: true,
            check_updates_on_start: true,
            shortcuts: BTreeMap::new(),
            other: Map::new(),
        }
    }
}

impl Settings {
    /// The settings in `dir`; none yet or unreadable gives the defaults.
    fn read(dir: &Path) -> Self {
        let Ok(json) = fs::read(dir.join(SETTINGS)) else {
            return Self::default();
        };
        serde_json::from_slice(&json).unwrap_or_else(|e| {
            eprintln!("Ignoring {}: {e}", dir.join(SETTINGS).display());
            Self::default()
        })
    }

    fn write(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        write_atomic(&dir.join(SETTINGS), &serde_json::to_vec_pretty(self)?)
    }

    /// With `changes` (field name → value) applied; `null` removes a field
    /// (its default applies), a value of the wrong type is an error.
    fn changed(&self, changes: Map<String, Value>) -> serde_json::Result<Self> {
        let Value::Object(mut fields) = serde_json::to_value(self)? else {
            unreachable!("settings are an object");
        };
        for (name, value) in changes {
            if value.is_null() {
                fields.remove(&name);
            } else {
                fields.insert(name, value);
            }
        }
        serde_json::from_value(Value::Object(fields))
    }
}

/// Gives every window the theme (`prefers-color-scheme` in the web views
/// follows it on Windows and macOS); at start before any window shows.
pub fn apply_theme<R: Runtime>(app: &AppHandle<R>, theme: Theme) {
    app.set_theme(match theme {
        Theme::System => None,
        Theme::Light => Some(tauri::Theme::Light),
        Theme::Dark => Some(tauri::Theme::Dark),
    });
}

/// The settings, read once at start; `None` without a config folder (then
/// changes last until the app quits).
pub struct SettingsState {
    dir: Option<PathBuf>,
    settings: Mutex<Settings>,
}

impl SettingsState {
    pub fn load(dir: Option<&Path>) -> Self {
        Self {
            dir: dir.map(Path::to_path_buf),
            settings: Mutex::new(dir.map(Settings::read).unwrap_or_default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Settings> {
        self.settings.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self) -> Settings {
        self.lock().clone()
    }
}

#[tauri::command]
pub async fn settings_get(state: State<'_, SettingsState>) -> Result<Settings, String> {
    Ok(state.get())
}

/// Changes some settings (`{"check_updates_on_start": false}`; `null`
/// restores the default) and writes them; returns all of them.
#[tauri::command]
pub async fn settings_set(
    changes: Map<String, Value>,
    app: AppHandle,
    state: State<'_, SettingsState>,
) -> Result<Settings, String> {
    let mut settings = state.lock();
    let new = settings
        .changed(changes)
        .map_err(|e| format!("Invalid setting: {e}"))?;
    if let Some(dir) = &state.dir {
        new.write(dir)
            .map_err(|e| format!("Could not save the settings: {e}"))?;
    }
    if new.theme != settings.theme {
        apply_theme(&app, new.theme);
    }
    *settings = new.clone();
    Ok(new)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lethora-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = temp_dir("missing");
        let settings = Settings::read(&dir);
        assert_eq!(settings, Settings::default());
        assert!(settings.check_updates_on_start);
    }

    #[test]
    fn a_broken_file_gives_the_defaults() {
        let dir = temp_dir("broken");
        fs::create_dir_all(&dir).unwrap();
        for broken in ["{not json", "[]", r#"{"check_updates_on_start": "no"}"#] {
            fs::write(dir.join(SETTINGS), broken).unwrap();
            assert_eq!(Settings::read(&dir), Settings::default(), "{broken}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_fields_default_and_unknown_ones_are_kept() {
        let dir = temp_dir("unknown");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(SETTINGS), r#"{"theme": "dark", "future": [1, 2]}"#).unwrap();
        let settings = Settings::read(&dir);
        assert!(settings.check_updates_on_start);
        assert_eq!(settings.theme, Theme::Dark);
        let settings = settings
            .changed(Map::from_iter([(
                "check_updates_on_start".into(),
                json!(false),
            )]))
            .unwrap();
        settings.write(&dir).unwrap();
        let written: Value =
            serde_json::from_slice(&fs::read(dir.join(SETTINGS)).unwrap()).unwrap();
        assert_eq!(written["check_updates_on_start"], json!(false));
        assert_eq!(written["theme"], json!("dark"));
        assert_eq!(written["future"], json!([1, 2]));
        assert_eq!(Settings::read(&dir), settings);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn null_restores_the_default_and_keeps_unknown_fields() {
        let settings: Settings = serde_json::from_value(json!({
            "theme": "light",
            "editor_text_size": 24.0,
            "spell_check": false,
            "future": "kept",
        }))
        .unwrap();
        let settings = settings
            .changed(Map::from_iter([
                ("theme".into(), Value::Null),
                ("editor_text_size".into(), Value::Null),
            ]))
            .unwrap();
        assert_eq!(settings.theme, Theme::System);
        assert_eq!(settings.editor_text_size, 19.5);
        assert!(!settings.spell_check);
        assert_eq!(settings.other.get("future"), Some(&json!("kept")));
    }

    #[test]
    fn shortcuts_round_trip_and_null_restores_the_defaults() {
        let dir = temp_dir("shortcuts");
        let settings = Settings::default()
            .changed(Map::from_iter([(
                "shortcuts".into(),
                json!({"bold": ["Ctrl+Shift+B", "F9"], "find": [], "future-command": ["Ctrl+J"]}),
            )]))
            .unwrap();
        assert_eq!(settings.shortcuts["bold"], ["Ctrl+Shift+B", "F9"]);
        assert!(settings.shortcuts["find"].is_empty());
        settings.write(&dir).unwrap();
        let read = Settings::read(&dir);
        assert_eq!(read, settings);
        assert_eq!(read.shortcuts["future-command"], ["Ctrl+J"]);
        let reset = read
            .changed(Map::from_iter([("shortcuts".into(), Value::Null)]))
            .unwrap();
        assert!(reset.shortcuts.is_empty());
        assert!(
            settings
                .changed(Map::from_iter([(
                    "shortcuts".into(),
                    json!({"bold": "Ctrl+B"})
                )]))
                .is_err()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_change_of_the_wrong_type_is_refused() {
        let settings = Settings::default();
        assert!(
            settings
                .changed(Map::from_iter([(
                    "check_updates_on_start".into(),
                    json!("no")
                )]))
                .is_err()
        );
        assert!(
            settings
                .changed(Map::from_iter([("theme".into(), json!("blue"))]))
                .is_err()
        );
    }
}
