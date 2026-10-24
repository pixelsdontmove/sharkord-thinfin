use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

pub const SLOTS: usize = 3;

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct ServerSlot {
    pub name: String,
    pub url: String,
}

fn default_mode() -> String {
    "toggle".to_string()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    #[serde(default)]
    pub mute_key: String,
    #[serde(default = "default_mode")]
    pub mute_mode: String,
    #[serde(default)]
    pub deafen_key: String,
    #[serde(default = "default_mode")]
    pub deafen_mode: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mute_key: String::new(),
            mute_mode: default_mode(),
            deafen_key: String::new(),
            deafen_mode: default_mode(),
        }
    }
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn favorites_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("favorites.json"))
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("settings.json"))
}

fn load_favorites(app: &AppHandle) -> Result<Vec<Option<ServerSlot>>, String> {
    let path = favorites_path(app)?;
    if !path.exists() {
        return Ok(vec![None; SLOTS]);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut slots: Vec<Option<ServerSlot>> =
        serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    while slots.len() < SLOTS {
        slots.push(None);
    }
    slots.truncate(SLOTS);
    Ok(slots)
}

fn save_favorites(app: &AppHandle, slots: &[Option<ServerSlot>]) -> Result<(), String> {
    let path = favorites_path(app)?;
    let raw = serde_json::to_string_pretty(slots).map_err(|e| e.to_string())?;
    std::fs::write(&path, raw).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn close_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
pub fn get_servers(app: AppHandle) -> Result<Vec<Option<ServerSlot>>, String> {
    load_favorites(&app)
}

#[tauri::command]
pub fn add_server(app: AppHandle, name: String, url: String) -> Result<usize, String> {
    let name = name.trim().to_string();
    let url = url.trim().to_string();
    if name.is_empty() {
        return Err("server name must not be empty".to_string());
    }
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("server url must start with http(s)://".to_string());
    }
    let mut slots = load_favorites(&app)?;
    // fill the first empty slot, or replace the last one when full.
    let slot = slots.iter().position(|s| s.is_none()).unwrap_or(SLOTS - 1);
    slots[slot] = Some(ServerSlot { name, url });
    save_favorites(&app, &slots)?;
    let _ = app.emit("chrome:servers_changed", ());
    Ok(slot)
}

#[tauri::command]
pub fn clear_server(app: AppHandle, slot: usize) -> Result<(), String> {
    if slot >= SLOTS {
        return Err("unknown slot".to_string());
    }
    let mut slots = load_favorites(&app)?;
    slots[slot] = None;
    save_favorites(&app, &slots)?;
    let _ = app.emit("chrome:servers_changed", ());
    Ok(())
}

#[tauri::command]
pub fn open_server(app: AppHandle, slot: usize) -> Result<(), String> {
    if slot >= SLOTS {
        return Err("unknown slot".to_string());
    }
    let slots = load_favorites(&app)?;
    let Some(server) = slots[slot].clone() else {
        return Err(format!("slot {} is empty", slot + 1));
    };
    let webview = app
        .get_webview("main")
        .ok_or_else(|| "main webview is missing".to_string())?;
    let url = server.url.parse::<tauri::Url>().map_err(|e| e.to_string())?;
    webview.navigate(url).map_err(|e| e.to_string())
}

// read the saved settings, or defaults when nothing is saved yet.
pub fn load_settings(app: &AppHandle) -> Result<Settings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<Settings, String> {
    load_settings(&app)
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let path = settings_path(&app)?;
    let raw = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, raw).map_err(|e| e.to_string())?;
    // keep the global hook in sync with the saved bindings.
    crate::hotkeys::update_bindings(&settings);
    let _ = app.emit("chrome:settings_changed", ());
    Ok(())
}

// served by the sharkordbar custom protocol. the bar is a 36px strip, so the
// add-server and settings panels live in the page webview instead: the bar
// only emits events that ask the page to render them.
pub const BAR_HTML: &str = r##"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<style>
  html, body { margin: 0; height: 100%; overflow: hidden; background: #0a0a0a; }
  body { display: flex; font-family: 'Segoe UI', 'Inter', sans-serif; user-select: none; cursor: default; }
  #bar { display: flex; align-items: center; gap: 4px; width: 100%; height: 100%; padding: 0 8px; box-sizing: border-box; }
  #brand { color: #f08c2e; font-weight: 700; font-size: 13px; margin-right: 6px; letter-spacing: .3px; display: inline-flex; align-items: center; gap: 5px; }
  button { background: none; border: none; color: #b5bac1; font-size: 12px; font-family: inherit; padding: 5px 9px; border-radius: 4px; cursor: pointer; max-width: 140px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  button:hover { background: #2b2d31; color: #ffffff; }
  .empty { color: #5c5e66; }
  .empty:hover { background: none; color: #87898c; }
  #spacer { flex: 1; }
  .icon { font-size: 14px; padding: 4px 8px; display: inline-flex; align-items: center; justify-content: center; }
  #sysaudio { color: #f23f43; }
  #sysaudio.on { color: #57f287; }
</style>
</head>
<body>
  <div id="bar">
    <span id="brand"><svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true"><defs><linearGradient id="fingrad" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#f6ab3e"/><stop offset="1" stop-color="#e05f1b"/></linearGradient></defs><path d="M4.5 13C4.5 8 6.5 4.5 10.5 2.5C10.5 6.5 11 10 12.5 13Z" fill="url(#fingrad)"/></svg>Thinfin</span>
    <button id="sysaudio" class="icon" title="share system audio with screen"><svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M4 6h2.5L11 3v10L6.5 10H4z"/><path d="M12.6 5.6a3.5 3.5 0 0 1 0 4.8" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg></button>
    <button id="settings" class="icon" title="hotkeys"><svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="1.5" y="4" width="13" height="8" rx="1.5"/><circle cx="4.5" cy="7" r="0.7" fill="currentColor" stroke="none"/><circle cx="7" cy="7" r="0.7" fill="currentColor" stroke="none"/><circle cx="9.5" cy="7" r="0.7" fill="currentColor" stroke="none"/><circle cx="12" cy="7" r="0.7" fill="currentColor" stroke="none"/><path d="M4.5 9.5h7"/></svg></button>
    <button class="slot" data-slot="0">S1</button>
    <button class="slot" data-slot="1">S2</button>
    <button class="slot" data-slot="2">S3</button>
    <button id="add" title="add favorite server">+</button>
    <div id="spacer"></div>
    <button id="minimize" class="icon" title="minimize to tray">&#8722;</button>
    <button id="maximize" class="icon" title="maximize">&#9633;</button>
    <button id="close" class="icon" title="close app">&#10005;</button>
  </div>
  <script>
  (function () {
    'use strict';
    var internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function') return;
    function invoke(cmd, args) { return internals.invoke(cmd, args || {}); }
    function barLog(message) {
      invoke('capture_log', { level: 'warn', message: '[sharkord-bar] ' + message });
    }

    // drag the frameless window from anywhere on the bar that is not a button.
    document.getElementById('bar').addEventListener('mousedown', function (e) {
      if (e.button !== 0) return;
      if (e.target.closest('button')) return;
      invoke('plugin:window|start_dragging', { label: 'main' });
    });

    var slots = Array.prototype.slice.call(document.querySelectorAll('.slot'));
    function render(servers) {
      for (var i = 0; i < slots.length; i++) {
        var el = slots[i];
        var server = servers && servers[i];
        if (server && server.name) {
          el.textContent = server.name;
          el.classList.remove('empty');
          el.title = server.url;
        } else {
          el.textContent = 'S' + (i + 1);
          el.classList.add('empty');
          el.title = 'empty slot, use + to add a server';
        }
      }
    }
    function refresh() {
      invoke('get_servers').then(render).catch(function () {});
    }

    slots.forEach(function (el) {
      var slot = Number(el.getAttribute('data-slot'));
      el.addEventListener('click', function () {
        invoke('open_server', { slot: slot }).catch(function () {});
      });
      el.addEventListener('contextmenu', function (e) {
        e.preventDefault();
        invoke('clear_server', { slot: slot }).then(refresh).catch(function () {});
      });
    });

    // the add and settings panels need text input and more room than the 36px
    // strip, so they are rendered inside the page webview via events.
    document.getElementById('add').addEventListener('click', function () {
      invoke('plugin:event|emit', { event: 'chrome:show_add_server', payload: null })
        .catch(function (error) { barLog('emit show_add_server failed ' + error); });
    });
    document.getElementById('settings').addEventListener('click', function () {
      invoke('plugin:event|emit', { event: 'chrome:show_settings', payload: null })
        .catch(function (error) { barLog('emit show_settings failed ' + error); });
    });

    // the speaker toggles "share system audio" for the next screen share.
    // rust owns the state and echoes it back so the page stays in sync.
    var sysAudioButton = document.getElementById('sysaudio');
    function renderSysAudio(enabled) {
      sysAudioButton.classList.toggle('on', !!enabled);
      sysAudioButton.title = enabled
        ? 'share system audio with screen (on)'
        : 'share system audio with screen (off)';
    }
    sysAudioButton.addEventListener('click', function () {
      var next = !sysAudioButton.classList.contains('on');
      renderSysAudio(next);
      invoke('system_audio_set_enabled', { enabled: next })
        .catch(function (error) { barLog('set_enabled failed ' + error); });
    });
    invoke('system_audio_get_enabled').then(renderSysAudio).catch(function () {});
    var sysAudioHandlerId = internals.transformCallback(function (eventData) {
      var payload = eventData && eventData.payload;
      if (payload) renderSysAudio(payload.enabled);
    });
    invoke('plugin:event|listen', {
      event: 'chrome:sys_audio',
      target: { kind: 'Any' },
      handler: sysAudioHandlerId
    }).catch(function (error) {
      barLog('sys_audio listen failed ' + error);
    });
    document.getElementById('minimize').addEventListener('click', function () {
      invoke('minimize_to_tray');
    });

    // maximize/restore toggle; the icon tracks the real window state, which
    // can also change outside the button (win+up, dragging to the screen
    // edge) via the chrome:window_maximized event emitted by rust.
    var maximizeButton = document.getElementById('maximize');
    // the glyph shows the target size: windowed shows the large square (click
    // to grow), maximized shows the small square (click to shrink).
    function renderMaximized(maximized) {
      maximizeButton.innerHTML = maximized ? '&#9643;' : '&#9633;';
      maximizeButton.title = maximized ? 'restore down' : 'maximize';
    }
    maximizeButton.addEventListener('click', function () {
      invoke('plugin:window|toggle_maximize', { label: 'main' })
        .catch(function (error) { barLog('toggle_maximize failed ' + error); });
    });
    invoke('plugin:window|is_maximized', { label: 'main' }).then(renderMaximized).catch(function () {});
    var maxHandlerId = internals.transformCallback(function (eventData) {
      var payload = eventData && eventData.payload;
      if (payload) renderMaximized(payload.maximized);
    });
    invoke('plugin:event|listen', {
      event: 'chrome:window_maximized',
      target: { kind: 'Any' },
      handler: maxHandlerId
    }).catch(function (error) {
      barLog('window_maximized listen failed ' + error);
    });
    document.getElementById('close').addEventListener('click', function () {
      invoke('close_app');
    });

    refresh();
    var handlerId = internals.transformCallback(function (eventData) {
      if (eventData && eventData.event === 'chrome:servers_changed') refresh();
    });
    invoke('plugin:event|listen', {
      event: 'chrome:servers_changed',
      target: { kind: 'Any' },
      handler: handlerId
    }).catch(function (error) {
      barLog('servers_changed listen failed ' + error);
    });
  })();
  </script>
</body>
</html>"##;
