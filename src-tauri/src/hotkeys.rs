// global input hook for the mute and deafen keybinds. listens to the whole
// system (keyboard and mouse) so the bindings work even when the app window is
// not focused, and supports mouse side buttons which registerhotkey cannot.
// a binding is a string like "ctrl+shift+m" or "mouse4". the hook emits
// chrome:hotkey, chrome:captured and chrome:capture_cancelled events that the
// page init script turns into clicks on the client's own voice buttons.
use rdev::{Button, Event, EventType, Key};
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

use crate::chrome::{load_settings, Settings};

const MOD_KEYS: [Key; 7] = [
    Key::ControlLeft,
    Key::ControlRight,
    Key::ShiftLeft,
    Key::ShiftRight,
    Key::Alt,
    Key::MetaLeft,
    Key::MetaRight,
];

fn is_modifier(key: Key) -> bool {
    MOD_KEYS.contains(&key)
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Toggle,
    Hold,
}

fn parse_mode(s: &str) -> Mode {
    if s == "hold" {
        Mode::Hold
    } else {
        Mode::Toggle
    }
}

#[derive(Clone, Copy)]
struct Mods {
    ctrl: bool,
    shift: bool,
    alt: bool,
    meta: bool,
}

fn current_mods(held: &HashSet<Key>) -> Mods {
    Mods {
        ctrl: held.contains(&Key::ControlLeft) || held.contains(&Key::ControlRight),
        shift: held.contains(&Key::ShiftLeft) || held.contains(&Key::ShiftRight),
        alt: held.contains(&Key::Alt),
        meta: held.contains(&Key::MetaLeft) || held.contains(&Key::MetaRight),
    }
}

// one keybind: a keyboard combo (modifiers + key) or a mouse button.
#[derive(Clone, Copy)]
struct Binding {
    key: Option<Key>,
    button: Option<Button>,
    ctrl: bool,
    shift: bool,
    alt: bool,
    meta: bool,
    mode: Mode,
}

impl Binding {
    fn empty(mode: Mode) -> Self {
        Self {
            key: None,
            button: None,
            ctrl: false,
            shift: false,
            alt: false,
            meta: false,
            mode,
        }
    }

    fn matches_key(&self, key: Key, mods: &Mods) -> bool {
        self.button.is_none()
            && self.key == Some(key)
            && self.ctrl == mods.ctrl
            && self.shift == mods.shift
            && self.alt == mods.alt
            && self.meta == mods.meta
    }

    fn matches_button(&self, button: Button) -> bool {
        self.button == Some(button)
    }
}

struct State {
    // which field is being captured, "mute" or "deafen".
    capture: Option<&'static str>,
    // modifier keys currently held, for building and matching combos.
    held_mods: HashSet<Key>,
    // non-modifier keys and mouse buttons currently down, for repeat detection.
    pressed: HashSet<Key>,
    pressed_buttons: HashSet<Button>,
    mute: Binding,
    deafen: Binding,
    // the key or button that activated the current hold, so the matching
    // release turns it off even if the modifiers changed in between.
    mute_hold_key: Option<Key>,
    deafen_hold_key: Option<Key>,
    mute_hold_button: Option<Button>,
    deafen_hold_button: Option<Button>,
}

impl State {
    fn new() -> Self {
        Self {
            capture: None,
            held_mods: HashSet::new(),
            pressed: HashSet::new(),
            pressed_buttons: HashSet::new(),
            mute: Binding::empty(Mode::Toggle),
            deafen: Binding::empty(Mode::Toggle),
            mute_hold_key: None,
            deafen_hold_key: None,
            mute_hold_button: None,
            deafen_hold_button: None,
        }
    }
}

static STATE: OnceLock<Arc<Mutex<State>>> = OnceLock::new();

// start the hook. called once from the tauri setup, before the app runs.
pub fn init(app: AppHandle) {
    let state = Arc::new(Mutex::new(State::new()));
    if let Ok(settings) = load_settings(&app) {
        let mut st = state.lock().unwrap();
        st.mute = parse_binding(&settings.mute_key, &settings.mute_mode);
        st.deafen = parse_binding(&settings.deafen_key, &settings.deafen_mode);
    }
    if STATE.set(state.clone()).is_err() {
        return;
    }
    let state_cb = state.clone();
    let app_cb = app.clone();
    std::thread::spawn(move || {
        let result = rdev::listen(move |event: Event| {
            handle_event(&app_cb, &state_cb, event);
        });
        if let Err(e) = result {
            eprintln!("[hotkeys] listen error: {:?}", e);
        }
    });
}

// re-read the bindings after the settings are saved.
pub fn update_bindings(settings: &Settings) {
    if let Some(state) = STATE.get() {
        let mut st = state.lock().unwrap();
        st.mute = parse_binding(&settings.mute_key, &settings.mute_mode);
        st.deafen = parse_binding(&settings.deafen_key, &settings.deafen_mode);
        st.mute_hold_key = None;
        st.deafen_hold_key = None;
        st.mute_hold_button = None;
        st.deafen_hold_button = None;
    }
}

pub fn start_capture(field: &str) {
    if let Some(state) = STATE.get() {
        let which = if field == "mute" { "mute" } else { "deafen" };
        let mut st = state.lock().unwrap();
        st.capture = Some(which);
    }
}

pub fn cancel_capture() {
    if let Some(state) = STATE.get() {
        let mut st = state.lock().unwrap();
        st.capture = None;
    }
}

// what the page should do in response to an event.
enum Action {
    None,
    Hotkey {
        action: &'static str,
        cmd: &'static str,
    },
    Captured { field: &'static str, value: String },
    CaptureCancelled { field: &'static str },
}

fn handle_event(app: &AppHandle, state: &Arc<Mutex<State>>, event: Event) {
    let action = {
        let mut st = state.lock().unwrap();
        process(&mut st, event)
    };
    match action {
        Action::None => {}
        Action::Hotkey { action, cmd } => {
            let _ = app.emit("chrome:hotkey", json!({ "action": action, "cmd": cmd }));
        }
        Action::Captured { field, value } => {
            let _ = app.emit("chrome:captured", json!({ "field": field, "value": value }));
        }
        Action::CaptureCancelled { field } => {
            let _ = app.emit("chrome:capture_cancelled", json!({ "field": field }));
        }
    }
}

fn hotkey_action(binding: &Binding, action: &'static str) -> Action {
    match binding.mode {
        Mode::Toggle => Action::Hotkey { action, cmd: "toggle" },
        Mode::Hold => Action::Hotkey { action, cmd: "hold_down" },
    }
}

fn process(st: &mut State, event: Event) -> Action {
    match event.event_type {
        EventType::KeyPress(key) => {
            if is_modifier(key) {
                st.held_mods.insert(key);
                return Action::None;
            }
            if let Some(field) = st.capture.take() {
                if key == Key::Escape {
                    return Action::CaptureCancelled { field };
                }
                let value = format_combo(&st.held_mods, key);
                return Action::Captured { field, value };
            }
            // ignore the os key repeat while the key is held.
            if st.pressed.contains(&key) {
                return Action::None;
            }
            st.pressed.insert(key);
            let mods = current_mods(&st.held_mods);
            if st.mute.matches_key(key, &mods) {
                st.mute_hold_key = (st.mute.mode == Mode::Hold).then_some(key);
                return hotkey_action(&st.mute, "mute");
            }
            if st.deafen.matches_key(key, &mods) {
                st.deafen_hold_key = (st.deafen.mode == Mode::Hold).then_some(key);
                return hotkey_action(&st.deafen, "deafen");
            }
            Action::None
        }
        EventType::KeyRelease(key) => {
            if is_modifier(key) {
                st.held_mods.remove(&key);
                return Action::None;
            }
            st.pressed.remove(&key);
            if st.mute_hold_key == Some(key) {
                st.mute_hold_key = None;
                return Action::Hotkey { action: "mute", cmd: "hold_up" };
            }
            if st.deafen_hold_key == Some(key) {
                st.deafen_hold_key = None;
                return Action::Hotkey { action: "deafen", cmd: "hold_up" };
            }
            Action::None
        }
        EventType::ButtonPress(button) => {
            if let Some(field) = st.capture.take() {
                let value = format_button(button);
                return Action::Captured { field, value };
            }
            if st.pressed_buttons.contains(&button) {
                return Action::None;
            }
            st.pressed_buttons.insert(button);
            if st.mute.matches_button(button) {
                st.mute_hold_button = (st.mute.mode == Mode::Hold).then_some(button);
                return hotkey_action(&st.mute, "mute");
            }
            if st.deafen.matches_button(button) {
                st.deafen_hold_button = (st.deafen.mode == Mode::Hold).then_some(button);
                return hotkey_action(&st.deafen, "deafen");
            }
            Action::None
        }
        EventType::ButtonRelease(button) => {
            st.pressed_buttons.remove(&button);
            if st.mute_hold_button == Some(button) {
                st.mute_hold_button = None;
                return Action::Hotkey { action: "mute", cmd: "hold_up" };
            }
            if st.deafen_hold_button == Some(button) {
                st.deafen_hold_button = None;
                return Action::Hotkey { action: "deafen", cmd: "hold_up" };
            }
            Action::None
        }
        _ => Action::None,
    }
}

// --- binding parsing and formatting --------------------------------------

fn parse_binding(combo: &str, mode: &str) -> Binding {
    let mode = parse_mode(mode);
    let combo = combo.trim();
    if combo.is_empty() {
        return Binding::empty(mode);
    }
    // a single token is either a mouse button or a bare key.
    if !combo.contains('+') {
        if let Some(button) = parse_button(combo) {
            let mut b = Binding::empty(mode);
            b.button = Some(button);
            return b;
        }
        if let Some(key) = parse_key(combo) {
            let mut b = Binding::empty(mode);
            b.key = Some(key);
            return b;
        }
        return Binding::empty(mode);
    }
    // otherwise it is a keyboard combo: modifiers plus a final key.
    let parts: Vec<&str> = combo.split('+').collect();
    let key_name = parts.last().unwrap().trim();
    let mut b = Binding::empty(mode);
    for part in &parts[..parts.len() - 1] {
        match part.trim().to_lowercase().as_str() {
            "ctrl" | "control" => b.ctrl = true,
            "shift" => b.shift = true,
            "alt" => b.alt = true,
            "meta" | "cmd" | "win" | "command" => b.meta = true,
            _ => {}
        }
    }
    if let Some(key) = parse_key(key_name) {
        b.key = Some(key);
    }
    b
}

fn format_combo(held: &HashSet<Key>, key: Key) -> String {
    let mods = current_mods(held);
    let mut parts: Vec<String> = Vec::new();
    if mods.ctrl {
        parts.push("Ctrl".to_string());
    }
    if mods.shift {
        parts.push("Shift".to_string());
    }
    if mods.alt {
        parts.push("Alt".to_string());
    }
    if mods.meta {
        parts.push("Meta".to_string());
    }
    parts.push(key_name(key));
    parts.join("+")
}

fn format_button(button: Button) -> String {
    match button {
        Button::Left => "Left".to_string(),
        Button::Right => "Right".to_string(),
        Button::Middle => "Middle".to_string(),
        // side buttons arrive as unknown with the xbutton code (1, 2, ...).
        // the mouse button number is the code plus three.
        Button::Unknown(code) => format!("Mouse{}", code as u32 + 3),
    }
}

fn parse_button(name: &str) -> Option<Button> {
    if let Some(rest) = name.strip_prefix("Mouse") {
        return rest.parse::<u32>().ok().and_then(|n| {
            if n >= 4 {
                Some(Button::Unknown((n - 3) as u8))
            } else {
                None
            }
        });
    }
    match name {
        "Left" => Some(Button::Left),
        "Right" => Some(Button::Right),
        "Middle" => Some(Button::Middle),
        _ => None,
    }
}

// human friendly name for a key, used for display and as the saved string.
fn key_name(key: Key) -> String {
    let letter = match key {
        Key::KeyA => Some('A'),
        Key::KeyB => Some('B'),
        Key::KeyC => Some('C'),
        Key::KeyD => Some('D'),
        Key::KeyE => Some('E'),
        Key::KeyF => Some('F'),
        Key::KeyG => Some('G'),
        Key::KeyH => Some('H'),
        Key::KeyI => Some('I'),
        Key::KeyJ => Some('J'),
        Key::KeyK => Some('K'),
        Key::KeyL => Some('L'),
        Key::KeyM => Some('M'),
        Key::KeyN => Some('N'),
        Key::KeyO => Some('O'),
        Key::KeyP => Some('P'),
        Key::KeyQ => Some('Q'),
        Key::KeyR => Some('R'),
        Key::KeyS => Some('S'),
        Key::KeyT => Some('T'),
        Key::KeyU => Some('U'),
        Key::KeyV => Some('V'),
        Key::KeyW => Some('W'),
        Key::KeyX => Some('X'),
        Key::KeyY => Some('Y'),
        Key::KeyZ => Some('Z'),
        _ => None,
    };
    if let Some(c) = letter {
        return c.to_string();
    }
    let digit = match key {
        Key::Num0 => Some('0'),
        Key::Num1 => Some('1'),
        Key::Num2 => Some('2'),
        Key::Num3 => Some('3'),
        Key::Num4 => Some('4'),
        Key::Num5 => Some('5'),
        Key::Num6 => Some('6'),
        Key::Num7 => Some('7'),
        Key::Num8 => Some('8'),
        Key::Num9 => Some('9'),
        _ => None,
    };
    if let Some(c) = digit {
        return c.to_string();
    }
    let fkey = match key {
        Key::F1 => Some(1),
        Key::F2 => Some(2),
        Key::F3 => Some(3),
        Key::F4 => Some(4),
        Key::F5 => Some(5),
        Key::F6 => Some(6),
        Key::F7 => Some(7),
        Key::F8 => Some(8),
        Key::F9 => Some(9),
        Key::F10 => Some(10),
        Key::F11 => Some(11),
        Key::F12 => Some(12),
        _ => None,
    };
    if let Some(n) = fkey {
        return format!("F{}", n);
    }
    match key {
        Key::Space => "Space".to_string(),
        Key::Return => "Enter".to_string(),
        Key::Tab => "Tab".to_string(),
        Key::Escape => "Escape".to_string(),
        Key::Backspace => "Backspace".to_string(),
        Key::Delete => "Delete".to_string(),
        Key::Insert => "Insert".to_string(),
        Key::Home => "Home".to_string(),
        Key::End => "End".to_string(),
        Key::PageUp => "PageUp".to_string(),
        Key::PageDown => "PageDown".to_string(),
        Key::LeftArrow => "LeftArrow".to_string(),
        Key::RightArrow => "RightArrow".to_string(),
        Key::UpArrow => "UpArrow".to_string(),
        Key::DownArrow => "DownArrow".to_string(),
        Key::Minus => "Minus".to_string(),
        Key::Equal => "Equal".to_string(),
        Key::Comma => "Comma".to_string(),
        Key::Dot => "Dot".to_string(),
        Key::Slash => "Slash".to_string(),
        Key::SemiColon => "SemiColon".to_string(),
        Key::Quote => "Quote".to_string(),
        Key::BackQuote => "BackQuote".to_string(),
        Key::LeftBracket => "LeftBracket".to_string(),
        Key::RightBracket => "RightBracket".to_string(),
        Key::BackSlash => "BackSlash".to_string(),
        _ => format!("{:?}", key),
    }
}

fn parse_key(name: &str) -> Option<Key> {
    match name {
        "A" => Some(Key::KeyA),
        "B" => Some(Key::KeyB),
        "C" => Some(Key::KeyC),
        "D" => Some(Key::KeyD),
        "E" => Some(Key::KeyE),
        "F" => Some(Key::KeyF),
        "G" => Some(Key::KeyG),
        "H" => Some(Key::KeyH),
        "I" => Some(Key::KeyI),
        "J" => Some(Key::KeyJ),
        "K" => Some(Key::KeyK),
        "L" => Some(Key::KeyL),
        "M" => Some(Key::KeyM),
        "N" => Some(Key::KeyN),
        "O" => Some(Key::KeyO),
        "P" => Some(Key::KeyP),
        "Q" => Some(Key::KeyQ),
        "R" => Some(Key::KeyR),
        "S" => Some(Key::KeyS),
        "T" => Some(Key::KeyT),
        "U" => Some(Key::KeyU),
        "V" => Some(Key::KeyV),
        "W" => Some(Key::KeyW),
        "X" => Some(Key::KeyX),
        "Y" => Some(Key::KeyY),
        "Z" => Some(Key::KeyZ),
        "0" => Some(Key::Num0),
        "1" => Some(Key::Num1),
        "2" => Some(Key::Num2),
        "3" => Some(Key::Num3),
        "4" => Some(Key::Num4),
        "5" => Some(Key::Num5),
        "6" => Some(Key::Num6),
        "7" => Some(Key::Num7),
        "8" => Some(Key::Num8),
        "9" => Some(Key::Num9),
        "F1" => Some(Key::F1),
        "F2" => Some(Key::F2),
        "F3" => Some(Key::F3),
        "F4" => Some(Key::F4),
        "F5" => Some(Key::F5),
        "F6" => Some(Key::F6),
        "F7" => Some(Key::F7),
        "F8" => Some(Key::F8),
        "F9" => Some(Key::F9),
        "F10" => Some(Key::F10),
        "F11" => Some(Key::F11),
        "F12" => Some(Key::F12),
        "Space" => Some(Key::Space),
        "Enter" => Some(Key::Return),
        "Tab" => Some(Key::Tab),
        "Escape" => Some(Key::Escape),
        "Backspace" => Some(Key::Backspace),
        "Delete" => Some(Key::Delete),
        "Insert" => Some(Key::Insert),
        "Home" => Some(Key::Home),
        "End" => Some(Key::End),
        "PageUp" => Some(Key::PageUp),
        "PageDown" => Some(Key::PageDown),
        "LeftArrow" => Some(Key::LeftArrow),
        "RightArrow" => Some(Key::RightArrow),
        "UpArrow" => Some(Key::UpArrow),
        "DownArrow" => Some(Key::DownArrow),
        "Minus" => Some(Key::Minus),
        "Equal" => Some(Key::Equal),
        "Comma" => Some(Key::Comma),
        "Dot" => Some(Key::Dot),
        "Slash" => Some(Key::Slash),
        "SemiColon" => Some(Key::SemiColon),
        "Quote" => Some(Key::Quote),
        "BackQuote" => Some(Key::BackQuote),
        "LeftBracket" => Some(Key::LeftBracket),
        "RightBracket" => Some(Key::RightBracket),
        "BackSlash" => Some(Key::BackSlash),
        _ => None,
    }
}

// --- tauri commands -------------------------------------------------------

#[tauri::command]
pub fn hk_start_capture(which: String) {
    start_capture(&which);
}

#[tauri::command]
pub fn hk_cancel_capture() {
    cancel_capture();
}
