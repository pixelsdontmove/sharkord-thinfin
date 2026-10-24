// hides the chromium screen share pill (the "X is sharing your screen" popup)
// that webview2 shows when the page starts a getDisplayMedia capture. the pill
// is a top level window owned by the msedgewebview2 browser process, so we find
// it by class and title and hide it at the win32 level.

use std::ffi::c_void;
use std::os::raw::c_int;

type Hwnd = *mut c_void;

type EnumWindowsProc = unsafe extern "system" fn(Hwnd, isize) -> i32;

extern "system" {
    fn EnumWindows(proc: EnumWindowsProc, lparam: isize) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, buf: *mut u16, max: c_int) -> c_int;
    fn GetClassNameW(hwnd: Hwnd, buf: *mut u16, max: c_int) -> c_int;
    fn IsWindowVisible(hwnd: Hwnd) -> i32;
    fn ShowWindow(hwnd: Hwnd, cmd: c_int) -> i32;
}

const SW_HIDE: c_int = 0;

// the pill window class and the substring that is always in its title.
const PILL_CLASS: &str = "Chrome_WidgetWin_1";
const PILL_TITLE_MARKER: &str = "is sharing";

fn is_pill(hwnd: Hwnd) -> bool {
    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            return false;
        }
        let mut class_buf = [0u16; 256];
        let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256);
        if class_len <= 0 {
            return false;
        }
        let class = String::from_utf16_lossy(&class_buf[..class_len as usize]);
        if class != PILL_CLASS {
            return false;
        }
        let mut title_buf = [0u16; 512];
        let title_len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 512);
        if title_len <= 0 {
            return false;
        }
        let title = String::from_utf16_lossy(&title_buf[..title_len as usize]);
        title.to_lowercase().contains(PILL_TITLE_MARKER)
    }
}

// hides every window that is the screen share pill. returns how many were hidden.
fn hide_pill_once() -> usize {
    let mut hidden = 0usize;
    unsafe extern "system" fn callback(hwnd: Hwnd, lparam: isize) -> i32 {
        if is_pill(hwnd) {
            ShowWindow(hwnd, SW_HIDE);
            *(lparam as *mut usize) += 1;
        }
        1
    }
    unsafe {
        EnumWindows(callback, &mut hidden as *mut usize as isize);
    }
    hidden
}

// the pill renders a moment after getDisplayMedia resolves, so hide it now and
// a few times over the next couple of seconds to catch it as it appears. this is
// a one shot burst at share start, not a continuous poller.
#[tauri::command]
pub fn hide_share_pill() {
    let delays = [0u64, 250, 500, 1000, 2000];
    std::thread::spawn(move || {
        for &ms in &delays {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            let n = hide_pill_once();
            if n > 0 {
                println!("[pill] hid {} screen share pill window(s)", n);
            }
        }
    });
}
