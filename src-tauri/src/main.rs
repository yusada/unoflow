// UnoFlow — minimal voice-to-text dictation for Windows.
//
// Flow: hold the global hotkey -> the webview records mic audio (getUserMedia +
// AudioWorklet) -> on release the PCM is sent here -> we transcribe it -> we
// paste the text into whatever window has focus.
//
// Design mirrors the split that makes desktop dictation reliable:
//   * webview  = capture + UI (has mic access, AudioWorklet)
//   * rust core = hotkey, transcription HTTP, clipboard + input injection
//   * UIA      = read the focused control so we can verify the paste landed

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appinfo;
mod cleanup;
mod local_server;
mod paste;
mod store;
mod transcribe;
mod translate;

use parking_lot::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WindowEvent,
};
use tauri_plugin_global_shortcut::{
    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutEvent, ShortcutState,
};

#[derive(Default)]
struct AppState {
    cfg: Config,
    status: String,
    store: store::Store,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    /// OpenAI API key. Empty until the user sets one.
    api_key: String,
    /// Transcription model.
    model: String,
    /// ISO-639-1 hint, e.g. "en". Empty = auto-detect.
    language: String,
    /// API base URL: OpenAI by default, or any OpenAI-compatible server
    /// (e.g. a local whisper.cpp server at http://127.0.0.1:8080/v1).
    base_url: String,
    /// When true, the focused control's text is sent as a transcription prompt
    /// so names and jargon come out right.
    context_aware: bool,
    /// Global hotkey that starts/stops dictation, e.g. "Ctrl+Shift+Space".
    shortcut: String,
    /// Show a small floating status pill while dictating.
    show_pill: bool,
    /// Cleanup strength: 0 off, 1 fillers+tidy, 2 also collapse repetitions.
    cleanup_level: u8,
    /// When true, dictation is translated before it is pasted.
    translate_enabled: bool,
    /// Target language for translation, e.g. "Indonesian".
    translate_to: String,
    /// Chat model used for translation (OpenAI-compatible endpoint).
    translate_model: String,
    /// True when audio is captured at 16 kHz mono PCM16.
    sample_rate: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: std::env::var("UNOFLOW_OPENAI_KEY").unwrap_or_default(),
            // Local by default: the bundled Whisper server with the multilingual
            // `tiny` model. Change the base URL in Settings to use a cloud API.
            model: "tiny".into(),
            language: String::new(),
            base_url: "http://127.0.0.1:8000/v1".into(),
            context_aware: false,
            shortcut: "Ctrl+Shift+Space".into(),
            show_pill: true,
            cleanup_level: 1,
            translate_enabled: false,
            translate_to: "English".into(),
            translate_model: "gpt-4o-mini".into(),
            sample_rate: 16_000,
        }
    }
}

/// The app's data directory: `%APPDATA%\UnoFlow`.
///
/// Renamed from `FlowLite`; on first run we adopt the old folder if the new one
/// doesn't exist yet, so an upgrade keeps its history and settings.
pub fn data_dir() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    let new = std::path::Path::new(&base).join("UnoFlow");
    if !new.exists() {
        let old = std::path::Path::new(&base).join("FlowLite");
        if old.exists() {
            let _ = std::fs::rename(&old, &new);
        }
    }
    new
}

fn config_path() -> std::path::PathBuf {
    data_dir().join("config.json")
}

fn load_config() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &Config) -> std::io::Result<()> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(cfg).unwrap())
}

/// Everything the frontend needs to know about the current configuration.
#[derive(Serialize)]
struct ConfigView {
    model: String,
    language: String,
    base_url: String,
    context_aware: bool,
    shortcut: String,
    show_pill: bool,
    cleanup_level: u8,
    translate_enabled: bool,
    translate_to: String,
    translate_model: String,
    api_key_set: bool,
}

#[tauri::command]
fn get_config(state: tauri::State<'_, Mutex<AppState>>) -> ConfigView {
    let s = state.lock();
    ConfigView {
        model: s.cfg.model.clone(),
        language: s.cfg.language.clone(),
        base_url: s.cfg.base_url.clone(),
        context_aware: s.cfg.context_aware,
        shortcut: s.cfg.shortcut.clone(),
        show_pill: s.cfg.show_pill,
        cleanup_level: s.cfg.cleanup_level,
        translate_enabled: s.cfg.translate_enabled,
        translate_to: s.cfg.translate_to.clone(),
        translate_model: s.cfg.translate_model.clone(),
        api_key_set: !s.cfg.api_key.is_empty(),
    }
}

#[tauri::command]
fn set_config(
    state: tauri::State<'_, Mutex<AppState>>,
    api_key: Option<String>,
    model: Option<String>,
    language: Option<String>,
    base_url: Option<String>,
    context_aware: Option<bool>,
    show_pill: Option<bool>,
    cleanup_level: Option<u8>,
    translate_enabled: Option<bool>,
    translate_to: Option<String>,
    translate_model: Option<String>,
) -> Result<(), String> {
    let mut s = state.lock();
    if let Some(k) = api_key {
        s.cfg.api_key = k;
    }
    if let Some(m) = model {
        s.cfg.model = m;
    }
    if let Some(l) = language {
        s.cfg.language = l;
    }
    if let Some(b) = base_url {
        if !b.trim().is_empty() {
            s.cfg.base_url = b.trim().to_string();
        }
    }
    if let Some(c) = context_aware {
        s.cfg.context_aware = c;
    }
    if let Some(p) = show_pill {
        s.cfg.show_pill = p;
    }
    if let Some(c) = cleanup_level {
        s.cfg.cleanup_level = c.min(2);
    }
    if let Some(t) = translate_enabled {
        s.cfg.translate_enabled = t;
    }
    if let Some(t) = translate_to {
        s.cfg.translate_to = t;
    }
    if let Some(m) = translate_model {
        if !m.trim().is_empty() {
            s.cfg.translate_model = m.trim().to_string();
        }
    }
    save_config(&s.cfg).map_err(|e| e.to_string())
}

/// Frontend pushes a short status string. We keep it for `get_status`, mirror it
/// to the floating pill, and show/hide the pill around a dictation.
#[tauri::command]
fn set_status(app: tauri::AppHandle, state: tauri::State<'_, Mutex<AppState>>, status: String) {
    let show_pill = {
        let mut s = state.lock();
        s.status = status.clone();
        s.cfg.show_pill
    };

    if !show_pill {
        return;
    }

    if let Some(pill) = app.get_webview_window("pill") {
        match status.as_str() {
            "recording" => {
                position_pill(&app, &pill);
                let _ = pill.show();
            }
            // Keep the pill up while transcribing so the wait is visible.
            "processing" | "done" => {
                let _ = pill.show();
            }
            _ => {
                let _ = pill.hide();
            }
        }
    }
    let _ = app.emit_to("pill", "pill:state", status);
}

/// Park the pill at the bottom-centre of the primary monitor, above the taskbar.
fn position_pill(app: &tauri::AppHandle, pill: &tauri::WebviewWindow) {
    let Ok(Some(monitor)) = app.primary_monitor() else {
        return;
    };
    let screen = monitor.size();
    let scale = monitor.scale_factor();
    let win = pill.outer_size().unwrap_or_default();
    let x = (screen.width as i32 - win.width as i32) / 2;
    // ~96 logical px up from the bottom edge.
    let y = screen.height as i32 - win.height as i32 - (96.0 * scale) as i32;
    let _ = pill.set_position(tauri::PhysicalPosition::new(x.max(0), y.max(0)));
}

#[tauri::command]
fn get_status(state: tauri::State<'_, Mutex<AppState>>) -> String {
    state.lock().status.clone()
}

/// Transcribe raw PCM16 mono audio. Sends the request to the configured API
/// (OpenAI by default, or any OpenAI-compatible server) and returns the text.
/// When context-awareness is on, the focused control's text is passed as a
/// prompt so names and jargon come out right.
#[tauri::command]
async fn transcribe(
    state: tauri::State<'_, Mutex<AppState>>,
    pcm: Vec<u8>,
    sample_rate: Option<u32>,
    context: Option<String>,
    duration_ms: Option<u64>,
) -> Result<String, String> {
    let (cfg, rate) = {
        let s = state.lock();
        // Dictionary words always bias the model; focused-app context does too
        // when the user has enabled it.
        let mut prompt = String::new();
        if !s.store.dictionary.is_empty() {
            prompt.push_str("Vocabulary: ");
            prompt.push_str(&s.store.dictionary.join(", "));
            prompt.push('.');
        }
        if s.cfg.context_aware {
            if let Some(c) = context.as_deref() {
                if !c.is_empty() {
                    if !prompt.is_empty() {
                        prompt.push(' ');
                    }
                    prompt.push_str(c);
                }
            }
        }
        let cfg = transcribe::TranscribeConfig {
            api_key: s.cfg.api_key.clone(),
            model: s.cfg.model.clone(),
            language: s.cfg.language.clone(),
            prompt,
            base_url: s.cfg.base_url.clone(),
        };
        (cfg, s.cfg.sample_rate)
    };
    // A local server may still be starting up; wait briefly rather than fail
    // the first dictation after launch.
    if local_server::is_local_url(&cfg.base_url) {
        let url = cfg.base_url.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            local_server::wait_until_ready(&url, std::time::Duration::from_secs(20))
        })
        .await;
    }

    let raw = transcribe::transcribe_pcm(&pcm, sample_rate.unwrap_or(rate), &cfg)
        .await
        .map_err(|e| e.to_string())?;

    // Post-processing pipeline: cleanup -> snippet expansion -> translation.
    // The history keeps the raw transcript; the caller receives the final text.
    let (mut text, snips, cleanup_level, translate_cfg, target) = {
        let s = state.lock();
        (
            raw.clone(),
            s.store.snippets.clone(),
            s.cfg.cleanup_level,
            translate::TranslateConfig {
                api_key: s.cfg.api_key.clone(),
                model: s.cfg.translate_model.clone(),
                base_url: s.cfg.base_url.clone(),
                target: s.cfg.translate_to.clone(),
            },
            s.cfg.translate_enabled,
        )
    };

    text = cleanup::clean(&text, cleanup_level);
    let pairs: Vec<(String, String)> = snips
        .into_iter()
        .map(|s| (s.trigger, s.expansion))
        .collect();
    text = cleanup::expand_snippets(&text, &pairs);

    if target && !text.is_empty() {
        match translate::translate(&text, &translate_cfg).await {
            Ok(t) => text = t,
            // A translation failure must not lose the dictation: keep the
            // untranslated text and let it paste.
            Err(e) => eprintln!("[translate] {e}"),
        }
    }

    if !raw.is_empty() {
        // Attribute this dictation to whatever app has focus right now.
        let app = appinfo::foreground_app().unwrap_or_default();
        state
            .lock()
            .store
            .add(raw, duration_ms.unwrap_or(0), app)
            .ok();
    }
    Ok(text)
}

/// Paste text into the currently focused control: save clipboard -> set
/// clipboard -> simulate Ctrl+V -> restore clipboard. Returns whether the
/// focused control's text changed (best-effort verification).
#[tauri::command]
fn insert_text(text: String) -> Result<bool, String> {
    paste::paste_text(&text).map_err(|e| e.to_string())
}

/// Read the focused control's current text (UI Automation). Used for context
/// and for paste verification. Returns None when unavailable.
#[tauri::command]
fn read_focused_text() -> Option<String> {
    paste::focused_text()
}

#[tauri::command]
fn list_history(state: tauri::State<'_, Mutex<AppState>>) -> Vec<store::Entry> {
    state.lock().store.history.clone()
}

#[tauri::command]
fn delete_history(state: tauri::State<'_, Mutex<AppState>>, id: String) -> Result<(), String> {
    state.lock().store.delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn clear_history(state: tauri::State<'_, Mutex<AppState>>) -> Result<(), String> {
    state.lock().store.clear().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_stats(state: tauri::State<'_, Mutex<AppState>>) -> store::Stats {
    state.lock().store.stats()
}

#[tauri::command]
fn get_insights(state: tauri::State<'_, Mutex<AppState>>) -> store::Insights {
    state.lock().store.insights()
}

#[tauri::command]
fn list_snippets(state: tauri::State<'_, Mutex<AppState>>) -> Vec<store::Snippet> {
    state.lock().store.snippets.clone()
}

#[tauri::command]
fn set_snippets(
    state: tauri::State<'_, Mutex<AppState>>,
    snippets: Vec<store::Snippet>,
) -> Result<(), String> {
    state
        .lock()
        .store
        .set_snippets(snippets)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_dictionary(state: tauri::State<'_, Mutex<AppState>>) -> Vec<String> {
    state.lock().store.dictionary.clone()
}

#[tauri::command]
fn set_dictionary(
    state: tauri::State<'_, Mutex<AppState>>,
    words: Vec<String>,
) -> Result<(), String> {
    state
        .lock()
        .store
        .set_dictionary(words)
        .map_err(|e| e.to_string())
}

/// Parse a hotkey string into a `Shortcut`, falling back to the default when
/// the string is invalid (so a bad config can't brick startup).
fn shortcut_from(s: &str) -> Shortcut {
    s.parse::<Shortcut>()
        .unwrap_or_else(|_| Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space))
}

/// Shared handler for the global hotkey: press starts, release stops.
fn shortcut_handler(app: &tauri::AppHandle, _s: &Shortcut, event: ShortcutEvent) {
    match event.state() {
        ShortcutState::Pressed => {
            let _ = app.emit("dictation:start", ());
        }
        ShortcutState::Released => {
            let _ = app.emit("dictation:stop", ());
        }
    }
}

#[tauri::command]
fn get_shortcut(state: tauri::State<'_, Mutex<AppState>>) -> String {
    state.lock().cfg.shortcut.clone()
}

/// Change the global hotkey: unregister the old one, register the new one, and
/// persist. The old shortcut is restored if the new one can't be registered.
#[tauri::command]
fn set_shortcut(
    app: tauri::AppHandle,
    state: tauri::State<'_, Mutex<AppState>>,
    shortcut: String,
) -> Result<(), String> {
    let parsed: Shortcut = shortcut
        .trim()
        .parse()
        .map_err(|_| format!("\"{shortcut}\" is not a valid shortcut"))?;

    let old_parsed = {
        let s = state.lock();
        shortcut_from(&s.cfg.shortcut)
    };

    let gs = app.global_shortcut();
    let _ = gs.unregister(old_parsed);
    if let Err(e) = gs.on_shortcut(parsed, shortcut_handler) {
        // Put the previous binding back so the user isn't left with nothing.
        let _ = gs.on_shortcut(old_parsed, shortcut_handler);
        return Err(format!("could not register \"{shortcut}\": {e}"));
    }

    let mut s = state.lock();
    s.cfg.shortcut = shortcut.trim().to_string();
    save_config(&s.cfg).map_err(|e| e.to_string())
}

fn main() {
    run();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let cfg = load_config();

    // If the config points at a local API and the bundled server is present,
    // start it now and remember the handle so it dies with the app. The handle
    // is a plain local: it lives across the event loop below and is dropped on
    // exit, which kills the server.
    let base_url = cfg.base_url.clone();
    let mut local_server = None;
    if local_server::is_local_url(&base_url) {
        if let Some(dir) = local_server::server_dir() {
            let port = local_server::port_from_url(&base_url);
            match local_server::LocalServer::start_if_local(&base_url, port, &dir, &cfg.model) {
                Ok(handle) => local_server = handle,
                Err(e) => eprintln!("[local-stt] {e}"),
            }
        }
    }
    // Wait for the server in the background so the window still appears at
    // once. The transcribe command has its own wait as a safety net.
    if local_server.is_some() {
        let url = base_url.clone();
        std::thread::spawn(move || {
            if local_server::wait_until_ready(&url, std::time::Duration::from_secs(120)) {
                eprintln!("[local-stt] ready at {url}");
            } else {
                eprintln!("[local-stt] server at {url} did not become ready in time");
            }
        });
    }

    let state = AppState {
        cfg,
        status: "idle".into(),
        store: store::Store::load(data_dir().join("store.json")),
    };
    let state_cfg = state.cfg.clone();

    let app = tauri::Builder::default()
        // Only one instance may run. A second launch reveals the existing
        // window instead of starting a rival process (which would fight over
        // the local server's port and add a duplicate tray icon).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .manage(Mutex::new(state))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcut(shortcut_from(&state_cfg.shortcut))
                .expect("register shortcut")
                .with_handler(shortcut_handler)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_config,
            set_config,
            set_status,
            get_status,
            transcribe,
            insert_text,
            read_focused_text,
            list_history,
            delete_history,
            clear_history,
            get_stats,
            get_insights,
            list_snippets,
            set_snippets,
            list_dictionary,
            set_dictionary,
            get_shortcut,
            set_shortcut
        ])
        .setup(|app| {
            // Tray menu. "Hide" keeps UnoFlow running in the background (the
            // dictation hotkey stays live); "Quit" is the real exit.
            let show = MenuItem::with_id(app, "show", "Open UnoFlow", true, None::<&str>)?;
            let hide = MenuItem::with_id(app, "hide", "Hide to background", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit UnoFlow", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &hide, &quit])?;
            TrayIconBuilder::with_id("unoflow-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("UnoFlow — hold Ctrl+Shift+Space to dictate")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                    "hide" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                    // Exit the whole app: stops the local STT server too.
                    "quit" => app.exit(0),
                    _ => {}
                })
                // Left-clicking the tray icon toggles the window.
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building UnoFlow");

    app.run(move |app_handle, event| match event {
        // Closing the main window keeps UnoFlow alive in the tray so the
        // dictation hotkey keeps working. "Quit UnoFlow" in the tray menu (or
        // app.exit) is the real exit.
        tauri::RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } if label == "main" => {
            api.prevent_close();
            if let Some(w) = app_handle.get_webview_window("main") {
                let _ = w.hide();
            }
        }
        // Stop the local STT server when the app actually exits, however it
        // exits. Doing it here (rather than after `run` returns) is what makes
        // the cleanup happen: the exit path does not always return from `run`.
        tauri::RunEvent::Exit => {
            if let Some(mut srv) = local_server.take() {
                srv.stop();
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An older config.json has no `shortcut` key. It must still parse, keeping
    /// the user's base_url — this regressed once and silently dropped local mode.
    #[test]
    fn old_config_without_shortcut_still_parses() {
        let json = r#"{
            "api_key": "",
            "model": "base.en",
            "language": "",
            "base_url": "http://127.0.0.1:8000/v1",
            "context_aware": false,
            "sample_rate": 16000
        }"#;
        let cfg: Config = serde_json::from_str(json).expect("old config must parse");
        assert_eq!(cfg.base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(cfg.model, "base.en");
        // The missing field falls back to the default.
        assert_eq!(cfg.shortcut, "Ctrl+Shift+Space");
    }

    #[test]
    fn empty_config_uses_defaults() {
        let cfg: Config = serde_json::from_str("{}").expect("empty config must parse");
        assert_eq!(cfg.shortcut, "Ctrl+Shift+Space");
        assert_eq!(cfg.base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(cfg.model, "tiny");
    }

    #[test]
    fn shortcut_parses_and_bad_input_falls_back() {
        assert_eq!(shortcut_from("Ctrl+Shift+Space"), "Ctrl+Shift+Space".parse::<Shortcut>().unwrap());
        // Garbage must not panic; it falls back to the default binding.
        let fallback = shortcut_from("not-a-real-shortcut");
        assert_eq!(fallback, shortcut_from("Ctrl+Shift+Space"));
    }

    #[test]
    fn whisper_model_detection() {
        for m in ["tiny", "base.en", "small", "medium.en", "large-v3"] {
            assert!(local_server::is_whisper_model(m), "{m} should be local");
        }
        for m in ["gpt-4o-transcribe", "whisper-1", ""] {
            assert!(!local_server::is_whisper_model(m), "{m} should not be local");
        }
    }
}
