// system audio capture for screen share. webview2's own getDisplayMedia
// loopback fails ("Could not start audio source"), so when the page requests
// system audio we capture the default render device here (plain wasapi
// loopback, this process is unsandboxed) and stream the pcm to the page as
// base64 chunks. the page turns it into a mediastream track and mixes it
// into the display stream.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use tauri::{AppHandle, Emitter, Manager, State};
use wasapi::{AudioClient, Direction, SampleType, StreamMode, WaveFormat, initialize_mta};

// 50ms of 48khz stereo f32 per emitted chunk.
const CHUNK_FRAMES: usize = 2400;

pub struct SystemAudio {
    stop: Mutex<Option<Arc<AtomicBool>>>,
    enabled: AtomicBool,
}

impl Default for SystemAudio {
    fn default() -> Self {
        Self {
            stop: Mutex::new(None),
            enabled: AtomicBool::new(false),
        }
    }
}

// current value of the "share system audio" toggle.
#[tauri::command]
pub fn system_audio_get_enabled(state: State<SystemAudio>) -> bool {
    state.enabled.load(Ordering::Relaxed)
}

// toggles "share system audio" and tells both webviews about it.
#[tauri::command]
pub fn system_audio_set_enabled(
    state: State<SystemAudio>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    state.enabled.store(enabled, Ordering::Relaxed);
    let _ = app.emit("chrome:sys_audio", serde_json::json!({ "enabled": enabled }));
    println!("[system-audio] share toggle: {}", if enabled { "on" } else { "off" });
    Ok(())
}

// finds the pid of the msedgewebview2.exe browser process we launched. the
// call audio is rendered by the webview's audio-service process, a child of
// that browser process, so excluding the browser process's tree is what drops
// the call audio. prefers the webview whose parent is us, falls back to the
// first webview found. logs every webview pid + parent so the tree is visible.
fn webview_browser_pid() -> Option<u32> {
    use std::os::raw::c_void;

    const TH32CS_SNAPPROCESS: u32 = 2;

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: *mut c_void,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_priority_class: i32,
        dw_flags: u32,
        sz_exe: [u16; 260],
    }

    extern "system" {
        fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> isize;
        fn Process32FirstW(h_snapshot: isize, lppre: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(h_snapshot: isize, lppre: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }

    let our_pid = std::process::id();
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == -1_isize {
        return None;
    }
    let mut entry: ProcessEntry32W = unsafe { std::mem::zeroed() };
    entry.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
    let mut first: Option<u32> = None;
    let mut own: Option<u32> = None;
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while ok {
        let name: Vec<u16> = entry
            .sz_exe
            .iter()
            .take_while(|&&c| c != 0)
            .copied()
            .collect();
        if String::from_utf16_lossy(&name).eq_ignore_ascii_case("msedgewebview2.exe") {
            println!(
                "[system-audio] webview: pid {} parent {}",
                entry.th32_process_id, entry.th32_parent_process_id
            );
            if first.is_none() {
                first = Some(entry.th32_process_id);
            }
            if entry.th32_parent_process_id == our_pid && own.is_none() {
                own = Some(entry.th32_process_id);
            }
        }
        ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    own.or(first)
}

fn capture_loop(app: AppHandle, stop: Arc<AtomicBool>) {
    let result = || -> Result<(), String> {
        let hr = initialize_mta();
        if !hr.is_ok() {
            return Err(format!("com init failed: {hr:?}"));
        }
        // process loopback: capture the default render device but EXCLUDE the
        // webview's process tree. the voice call renders in the webview's
        // audio-service process (a child of the msedgewebview2 browser process),
        // so excluding that tree drops the call audio -- including the viewer's
        // own voice -- while passing through everything else (games, music,
        // other apps). falls back to our own pid if no webview is found.
        let our_pid = std::process::id();
        let target_pid = webview_browser_pid().unwrap_or(our_pid);
        let mut client = AudioClient::new_application_loopback_client(target_pid, false)
            .map_err(|e| format!("process loopback: {e:?}"))?;
        println!(
            "[system-audio] process loopback: excluding tree of pid {target_pid} (host {our_pid})"
        );

        // 48khz stereo float, autoconverted. fixed buffer period: get_device_period
        // is not implemented in process loopback mode.
        let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
        let blockalign = format.get_blockalign() as usize;
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 200_000, // 20ms
        };
        client
            .initialize_client(&format, &Direction::Capture, &mode)
            .map_err(|e| format!("initialize: {e:?}"))?;
        let event = client
            .set_get_eventhandle()
            .map_err(|e| format!("event handle: {e:?}"))?;
        let capture = client
            .get_audiocaptureclient()
            .map_err(|e| format!("capture client: {e:?}"))?;

        let mut queue: VecDeque<u8> = VecDeque::new();
        client.start_stream().map_err(|e| format!("start stream: {e:?}"))?;

        let chunk_bytes = CHUNK_FRAMES * blockalign;
        while !stop.load(Ordering::Relaxed) {
            if queue.len() >= chunk_bytes {
                let mut chunk = vec![0u8; chunk_bytes];
                for slot in chunk.iter_mut() {
                    *slot = queue.pop_front().unwrap();
                }
                let _ = app.emit("system_audio:pcm", STANDARD.encode(&chunk));
            }
            if let Err(e) = capture.read_from_device_to_deque(&mut queue) {
                return Err(format!("read: {e:?}"));
            }
            // short wait so the stop flag is checked even when silent.
            let _ = event.wait_for_event(200);
        }
        let _ = client.stop_stream();
        Ok(())
    }();
    if let Err(error) = result {
        println!("[system-audio] capture stopped: {error}");
    }
    // clear the running state so a later share can start again.
    if let Some(managed) = app.try_state::<SystemAudio>() {
        if let Ok(mut guard) = managed.stop.lock() {
            if guard.as_ref().is_some_and(|s| Arc::ptr_eq(s, &stop)) {
                *guard = None;
            }
        }
    }
}

// starts the loopback capture if it is not running yet.
#[tauri::command]
pub fn system_audio_start(state: State<SystemAudio>, app: AppHandle) -> Result<(), String> {
    let mut guard = state.stop.lock().map_err(|e| e.to_string())?;
    if guard.is_some() {
        return Ok(());
    }
    let stop = Arc::new(AtomicBool::new(false));
    *guard = Some(stop.clone());
    drop(guard);
    thread::Builder::new()
        .name("system-audio".into())
        .spawn(move || capture_loop(app, stop))
        .map_err(|e| format!("spawn: {e}"))?;
    println!("[system-audio] capture started");
    Ok(())
}

// stops the capture and clears the running state.
#[tauri::command]
pub fn system_audio_stop(state: State<SystemAudio>) -> Result<(), String> {
    let mut guard = state.stop.lock().map_err(|e| e.to_string())?;
    if let Some(stop) = guard.take() {
        stop.store(true, Ordering::Relaxed);
        println!("[system-audio] capture stopping");
    }
    Ok(())
}
