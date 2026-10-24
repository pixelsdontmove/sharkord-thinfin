// release builds are windows gui apps (no console window on double click);
// debug builds keep the console for the forwarded page log lines.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chrome;
mod hotkeys;
mod pill;
mod system_audio;
mod tray;

use tauri::{Emitter, Manager};

// the window loads the hosted sharkord client directly.
// a dark page the main webview loads before the hosted client. on first run it
// shows a connect-to-server intro; otherwise it hands off to the first saved
// server. the window never opens on white.
const SHELL_HTML: &str = r##"<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <style>
      html, body { margin: 0; height: 100%; background: #0a0a0a; }
      body { font-family: 'Segoe UI', 'Inter', system-ui, sans-serif; color: #dbdee1; }
      #intro { position: fixed; inset: 0; display: none; align-items: center; justify-content: center; }
      #intro.show { display: flex; }
      .card { width: 380px; background: #1c1d20; border: 1px solid #2b2d31; border-radius: 10px; padding: 28px; box-sizing: border-box; box-shadow: 0 8px 24px rgba(0,0,0,.5); }
      .brand { display: flex; align-items: center; gap: 8px; margin-bottom: 20px; }
      .brand .name { color: #f08c2e; font-weight: 700; font-size: 20px; letter-spacing: .3px; }
      h1 { font-size: 15px; font-weight: 600; margin: 0 0 6px; }
      p.sub { font-size: 12px; color: #949ba4; margin: 0 0 18px; line-height: 1.5; }
      input { width: 100%; box-sizing: border-box; background: #0f0f10; border: 1px solid #2b2d31; border-radius: 6px; color: #dbdee1; font-size: 13px; padding: 10px 12px; outline: none; }
      input:focus { border-color: #f08c2e; }
      button { width: 100%; margin-top: 12px; background: #f08c2e; color: #0a0a0a; font-weight: 700; font-size: 13px; border: none; border-radius: 6px; padding: 11px; cursor: pointer; }
      button:hover { background: #f79a3c; }
      button:disabled { opacity: .6; cursor: default; }
      .err { color: #f23f43; font-size: 12px; margin-top: 10px; min-height: 14px; }
      .note { color: #5c5e66; font-size: 11px; margin-top: 14px; text-align: center; }
    </style>
  </head>
  <body>
    <div id="intro">
      <div class="card">
        <div class="brand">
          <svg width="28" height="28" viewBox="0 0 16 16" aria-hidden="true"><defs><linearGradient id="fingrad" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#f6ab3e"/><stop offset="1" stop-color="#e05f1b"/></linearGradient></defs><path d="M4.5 13C4.5 8 6.5 4.5 10.5 2.5C10.5 6.5 11 10 12.5 13Z" fill="url(#fingrad)"/></svg>
          <span class="name">Thinfin</span>
        </div>
        <h1>Connect to your server</h1>
        <p class="sub">Enter the URL of your Sharkord server. It is saved to S1 and opened.</p>
        <input id="url" type="text" placeholder="https://your-server.example" spellcheck="false" autocomplete="off" />
        <button id="go">Connect</button>
        <div class="err" id="err"></div>
        <div class="note">Add more servers later with the + button.</div>
      </div>
    </div>
    <script>
      (function () {
        var internals = window.__TAURI_INTERNALS__;
        if (!internals || typeof internals.invoke !== 'function') {
          document.body.style.background = 'purple';
        }
        function invoke(cmd, args) { return internals.invoke(cmd, args || {}); }
        var intro = document.getElementById('intro');
        var urlInput = document.getElementById('url');
        var goButton = document.getElementById('go');
        var errEl = document.getElementById('err');

        function showIntro() {
          intro.classList.add('show');
          urlInput.focus();
        }

        function connect() {
          var url = urlInput.value.trim();
          if (!url) { errEl.textContent = 'Enter a server URL.'; return; }
          if (!/^https?:\/\//i.test(url)) url = 'https://' + url;
          var name;
          try { name = new URL(url).hostname; } catch (e) { name = url; }
          errEl.textContent = '';
          goButton.disabled = true;
          invoke('add_server', { name: name, url: url }).then(function () {
            window.location.replace(url);
          }).catch(function (error) {
            goButton.disabled = false;
            errEl.textContent = String(error);
          });
        }

        goButton.addEventListener('click', connect);
        urlInput.addEventListener('keydown', function (e) {
          if (e.key === 'Enter') connect();
        });

        // after one painted dark frame: hand off to the first saved server, or
        // show the first-run intro if none are saved.
        window.addEventListener('load', function () {
          requestAnimationFrame(function () {
            invoke('get_servers').then(function (servers) {
              var first = servers && servers.find(function (s) { return s && s.url; });
              if (first) {
                window.location.replace(first.url);
              } else {
                showIntro();
              }
            }).catch(function (e) {
              errEl.textContent = 'diag: ' + e;
              showIntro();
            });
          });
        });
      })();
    </script>
  </body>
</html>"##;

// runs before any page script, so it can patch the page's media apis
// regardless of the site's csp. also renders the chrome panels.
const PAGE_INIT_SCRIPT: &str = include_str!("capture_stub.js");

// height of the custom title bar strip, in logical pixels.
const BAR_HEIGHT: f64 = 36.0;

// receives console.warn/error forwarded by the page init script.
#[tauri::command]
fn capture_log(level: String, message: String) {
    println!("[page][{}] {}", level, message);
}

// lays out the bar and page webviews inside the window. the bar occupies the
// top strip, the page fills the rest.
fn layout_webviews(window: &tauri::Window) {
    let scale = match window.scale_factor() {
        Ok(scale) => scale,
        Err(_) => return,
    };
    let size = match window.inner_size() {
        Ok(size) => size,
        Err(_) => return,
    };
    let bar_h = ((BAR_HEIGHT * scale) as u32).max(1);
    let page_h = size.height.saturating_sub(bar_h);

    for webview in window.webviews() {
        let bounds = if webview.label() == "bar" {
            tauri::Rect {
                position: tauri::PhysicalPosition::new(0, 0).into(),
                size: tauri::PhysicalSize::new(size.width, bar_h).into(),
            }
        } else {
            tauri::Rect {
                position: tauri::PhysicalPosition::new(0, bar_h as i32).into(),
                size: tauri::PhysicalSize::new(size.width, page_h).into(),
            }
        };
        let _ = webview.set_bounds(bounds);
    }
}

fn main() {
    // keep the renderer at full speed while the window is in the background or
    // covered by a game, so the stream stays smooth. webview2 reads this before
    // it spawns the browser process, so it must be set before the builder runs.
    unsafe {
        std::env::set_var(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "--disable-background-timer-throttling \
             --disable-backgrounding-occluded-windows \
             --disable-renderer-backgrounding \
             --disable-features=CalculateNativeWinOcclusion",
        );
    }

    let shell_html = SHELL_HTML.to_string();

    tauri::Builder::default()
        .manage(system_audio::SystemAudio::default())
        .register_uri_scheme_protocol("sharkordbar", |_ctx, _request| {
            tauri::http::Response::builder()
                .header(
                    tauri::http::header::CONTENT_TYPE,
                    "text/html; charset=utf-8",
                )
                .body(chrome::BAR_HTML.as_bytes().to_vec())
                .expect("bar response builds")
        })
        .register_uri_scheme_protocol("sharkordshell", move |_ctx, _request| {
            tauri::http::Response::builder()
                .header(
                    tauri::http::header::CONTENT_TYPE,
                    "text/html; charset=utf-8",
                )
                .body(shell_html.as_bytes().to_vec())
                .expect("shell response builds")
        })
        .setup(|app| {
            // the hosted client page is a remote origin to tauri's acl. grant
            // it ipc for every saved server before the shell page can hand
            // off to one, or the first navigation would land on a page whose
            // ipc is rejected.
            if let Ok(slots) = chrome::load_favorites(app.handle()) {
                for slot in slots.iter().flatten() {
                    chrome::grant_remote_access(app.handle(), &slot.url);
                }
            }

            let shell_url: tauri::Url = "sharkordshell://localhost/index.html"
                .parse()
                .expect("shell url is valid");
            let builder = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::CustomProtocol(shell_url),
            )
            .title("Sharkord Client")
            .decorations(false)
            .inner_size(1280.0, 720.0)
            // dark while the window exists but the page has not painted yet.
            .background_color(tauri::window::Color(0x0a, 0x0a, 0x0a, 0xff))
            .initialization_script(PAGE_INIT_SCRIPT)
            // keep the window hidden until the local dark shell page has loaded,
            // so the first visible frame is dark, never the unpainted white surface.
            .visible(false)
            .on_page_load(|window, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            });

            builder.center().build()?;

            // custom title bar: a 36px webview strip above the page.
            let window = app
                .get_window("main")
                .expect("main window was just built");
            let bar_url: tauri::Url = "sharkordbar://localhost/index.html"
                .parse()
                .expect("bar url is valid");
            window.add_child(
                tauri::WebviewBuilder::new("bar", tauri::WebviewUrl::CustomProtocol(bar_url))
                    .focused(false),
                tauri::LogicalPosition::new(0.0, 0.0),
                tauri::LogicalSize::new(1280.0, BAR_HEIGHT),
            )?;

            layout_webviews(&window);

            // global mute and deafen keybinds, active even when unfocused.
            hotkeys::init(app.handle().clone());

            // tray icon: the window minimizes here instead of the taskbar.
            tray::init(app.handle())?;

            // keep both webviews laid out while the window is resized, and
            // tell the bar when the maximize state changes so its button icon
            // stays in sync (win+up, dragging to the screen edge). there is no
            // dedicated maximize window event, so the state is re-checked on
            // every resize and only emitted when it actually changes.
            let layout_window = window.clone();
            let app_handle = app.handle().clone();
            let last_maximized = std::cell::Cell::new(false);
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::Resized(_) = event {
                    layout_webviews(&layout_window);
                    let maximized = layout_window.is_maximized().unwrap_or(false);
                    if last_maximized.get() != maximized {
                        last_maximized.set(maximized);
                        let _ = app_handle.emit(
                            "chrome:window_maximized",
                            serde_json::json!({ "maximized": maximized }),
                        );
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            capture_log,
            chrome::close_app,
            chrome::get_servers,
            chrome::add_server,
            chrome::clear_server,
            chrome::open_server,
            chrome::get_settings,
            chrome::save_settings,
            hotkeys::hk_start_capture,
            hotkeys::hk_cancel_capture,
            tray::minimize_to_tray,
            pill::hide_share_pill,
            system_audio::system_audio_start,
            system_audio::system_audio_stop,
            system_audio::system_audio_get_enabled,
            system_audio::system_audio_set_enabled
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
