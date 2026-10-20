use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
static TRAY_PRESENT: AtomicBool = AtomicBool::new(false);
static START_IN_TRAY: AtomicBool = AtomicBool::new(false);
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static UI_CONTEXT: std::sync::OnceLock<eframe::egui::Context> = std::sync::OnceLock::new();
static EXIT_REQUESTED: AtomicBool = AtomicBool::new(false);

pub const fn tray_enabled() -> bool {
    !cfg!(feature = "portable")
}

#[cfg(test)]
pub fn tray_present() -> bool {
    TRAY_PRESENT.load(Ordering::Relaxed)
}

/// Wake the event loop explicitly: hidden windows may never receive WM_PAINT.
/// The event loop performs a normal eframe shutdown, including on_exit/save.
pub fn request_exit() {
    EXIT_REQUESTED.store(true, Ordering::Release);
    if let Some(ctx) = UI_CONTEXT.get() {
        ctx.request_repaint();
    }
}

pub fn take_exit_request() -> bool {
    EXIT_REQUESTED.swap(false, Ordering::AcqRel)
}

pub fn remove_tray_icon() {
    #[cfg(windows)]
    unsafe {
        let hwnd = WINDOW.load(Ordering::Relaxed) as windows_sys::Win32::Foundation::HWND;
        if !hwnd.is_null() && TRAY_PRESENT.load(Ordering::Relaxed) {
            tray(hwnd, false);
        }
    }
    TRAY_PRESENT.store(false, Ordering::Relaxed);
}

pub fn close_button(close_to_tray: bool) -> bool {
    if tray_enabled() && close_to_tray {
        hide_to_tray()
    } else {
        request_exit();
        true
    }
}

/// Native visibility is authoritative: SW_HIDE does not set egui's minimized flag.
pub fn is_backgrounded() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
        let hwnd = WINDOW.load(Ordering::Relaxed) as windows_sys::Win32::Foundation::HWND;
        return !hwnd.is_null() && (IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0);
    }
    #[cfg(not(windows))]
    false
}

/// Progress can be coalesced while hidden; completion events still wake immediately.
pub fn progress_repaint(ctx: &eframe::egui::Context) {
    if is_backgrounded() {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    } else {
        ctx.request_repaint();
    }
}
/// Start hidden only after a usable tray icon has been created.
/// eframe shows its first rendered frame automatically; the native hook suppresses
/// that show until the user restores the launcher.
pub fn start_in_tray() -> bool {
    if hide_to_tray() {
        START_IN_TRAY.store(true, Ordering::Relaxed);
        true
    } else {
        false
    }
}

// Close and tray startup hide the window. Ordinary minimization stays on the taskbar.
pub fn hide_to_tray() -> bool {
    if !tray_enabled() {
        return false;
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
        let hwnd = WINDOW.load(Ordering::Relaxed) as windows_sys::Win32::Foundation::HWND;
        if !hwnd.is_null() && (TRAY_PRESENT.load(Ordering::Relaxed) || tray(hwnd, true)) {
            ShowWindow(hwnd, SW_HIDE);
            return true;
        }
    }
    false
}
#[cfg(windows)]
unsafe fn tray(hwnd: windows_sys::Win32::Foundation::HWND, add: bool) -> bool {
    if add && !tray_enabled() {
        return false;
    }
    use windows_sys::Win32::UI::{Shell::*, WindowsAndMessaging::*};
    unsafe {
        let mut n: NOTIFYICONDATAW = std::mem::zeroed();
        n.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        n.hWnd = hwnd;
        n.uID = 1;
        n.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        n.uCallbackMessage = WM_APP + 77;
        n.hIcon = LoadIconW(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as _, 1usize as _);
        if n.hIcon.is_null() {
            n.hIcon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
        }
        for (dest, src) in n
            .szTip
            .iter_mut()
            .zip("DoNothing&Win Launcher - click to restore".encode_utf16())
        {
            *dest = src;
        }
        let ok = Shell_NotifyIconW(if add { NIM_ADD } else { NIM_DELETE }, &n) != 0;
        if ok {
            TRAY_PRESENT.store(add, Ordering::Relaxed);
        }
        ok
    }
}
pub fn completion_notice(message: &str) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::{Shell::*, WindowsAndMessaging::*};
        let hwnd = WINDOW.load(Ordering::Relaxed) as windows_sys::Win32::Foundation::HWND;
        if hwnd.is_null() {
            return;
        }
        if !tray_enabled() {
            // Portable builds use the taskbar and in-app activity history only.
            FlashWindow(hwnd, 1);
            return;
        }
        if !TRAY_PRESENT.load(Ordering::Relaxed) && !tray(hwnd, true) {
            return;
        }
        let mut n: NOTIFYICONDATAW = std::mem::zeroed();
        n.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        n.hWnd = hwnd;
        n.uID = 1;
        n.uFlags = NIF_INFO;
        n.dwInfoFlags = NIIF_INFO;
        for (dest, src) in n
            .szInfoTitle
            .iter_mut()
            .zip("DoNothing&Win Launcher".encode_utf16())
        {
            *dest = src;
        }
        for (dest, src) in n.szInfo.iter_mut().take(255).zip(message.encode_utf16()) {
            *dest = src;
        }
        Shell_NotifyIconW(NIM_MODIFY, &n);
        FlashWindow(hwnd, 1);
    }
}
// Give the borderless window native non-client resize edges. Windows performs
// hit-testing synchronously, so fast drags work without waiting for an egui frame.
#[cfg(windows)]
pub fn install(cc: &eframe::CreationContext<'_>) {
    let _ = UI_CONTEXT.set(cc.egui_ctx.clone());
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(handle) = cc.window_handle() {
        if let RawWindowHandle::Win32(window) = handle.as_raw() {
            unsafe {
                WINDOW.store(window.hwnd.get(), Ordering::Relaxed);
                windows_sys::Win32::UI::Shell::SetWindowSubclass(
                    window.hwnd.get() as _,
                    Some(frame_proc),
                    0x45575253,
                    0,
                );
            }
        }
    }
}
#[cfg(not(windows))]
pub fn install(_: &eframe::CreationContext<'_>) {}

#[cfg(windows)]
unsafe extern "system" fn frame_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    subclass: usize,
    _: usize,
) -> isize {
    use windows_sys::Win32::{
        Foundation::RECT,
        UI::{
            HiDpi::GetDpiForWindow,
            Shell::{DefSubclassProc, RemoveWindowSubclass},
            WindowsAndMessaging::*,
        },
    };
    unsafe {
        if message == WM_WINDOWPOSCHANGED && START_IN_TRAY.load(Ordering::Relaxed) {
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            if IsWindowVisible(hwnd) != 0 {
                ShowWindow(hwnd, SW_HIDE);
            }
            return result;
        }
        if message == WM_SHOWWINDOW || message == WM_SIZE {
            if let Some(ctx) = UI_CONTEXT.get() {
                ctx.request_repaint();
            }
        }
        if message == WM_APP + 78 {
            if wparam == 1 {
                REPLACE_REQUESTED.store(true, Ordering::SeqCst);
            }
            START_IN_TRAY.store(false, Ordering::Relaxed);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
            tray(hwnd, false);
            return 0;
        }
        if message == WM_NCDESTROY {
            remove_tray_icon();
            WINDOW.store(0, Ordering::Relaxed);
            RemoveWindowSubclass(hwnd, Some(frame_proc), subclass);
        }
        if message == WM_APP + 77 {
            let event = lparam as u32;
            if event == WM_LBUTTONUP
                || event == WM_LBUTTONDBLCLK
                || event == windows_sys::Win32::UI::Shell::NIN_BALLOONUSERCLICK
            {
                START_IN_TRAY.store(false, Ordering::Relaxed);
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                tray(hwnd, false);
                return 0;
            }
            if event == WM_RBUTTONUP {
                let menu = CreatePopupMenu();
                let show: Vec<u16> = "Show DoNothing&Win Launcher\0".encode_utf16().collect();
                let quit: Vec<u16> = "Exit\0".encode_utf16().collect();
                AppendMenuW(menu, MF_STRING, 1, show.as_ptr());
                AppendMenuW(menu, MF_STRING, 2, quit.as_ptr());
                let mut point = windows_sys::Win32::Foundation::POINT::default();
                GetCursorPos(&mut point);
                SetForegroundWindow(hwnd);
                let choice = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    point.x,
                    point.y,
                    0,
                    hwnd,
                    std::ptr::null(),
                );
                DestroyMenu(menu);
                // Finish the native popup's message sequence before returning.
                PostMessageW(hwnd, WM_NULL, 0, 0);
                if choice == 1 {
                    START_IN_TRAY.store(false, Ordering::Relaxed);
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                    tray(hwnd, false);
                }
                if choice == 2 {
                    request_exit();
                }
                return 0;
            }
        }
        if message == WM_NCHITTEST && IsZoomed(hwnd) == 0 {
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r) != 0 {
                let x = (lparam as u32 & 0xffff) as i16 as i32;
                let y = ((lparam as u32 >> 16) & 0xffff) as i16 as i32;
                let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.;
                let rect = eframe::egui::Rect::from_min_max(
                    eframe::egui::pos2(r.left as f32 / scale, r.top as f32 / scale),
                    eframe::egui::pos2(r.right as f32 / scale, r.bottom as f32 / scale),
                );
                if let Some((edge, _)) = crate::ui::resize_edge(
                    rect,
                    eframe::egui::pos2(x as f32 / scale, y as f32 / scale),
                ) {
                    use eframe::egui::viewport::ResizeDirection::*;
                    return match edge {
                        NorthWest => HTTOPLEFT,
                        NorthEast => HTTOPRIGHT,
                        SouthWest => HTBOTTOMLEFT,
                        SouthEast => HTBOTTOMRIGHT,
                        North => HTTOP,
                        South => HTBOTTOM,
                        West => HTLEFT,
                        East => HTRIGHT,
                    } as isize;
                }
            }
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

static REPLACE_REQUESTED: AtomicBool = AtomicBool::new(false);
pub fn take_replace_request() -> bool {
    REPLACE_REQUESTED.swap(false, Ordering::SeqCst)
}
pub fn activate_existing(replace: bool) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let title: Vec<u16> = "DoNothing&Win Launcher\0".encode_utf16().collect();
        for _ in 0..30 {
            let mut hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if hwnd.is_null() {
                let legacy: Vec<u16> = "Alpine Launcher\0".encode_utf16().collect();
                hwnd = FindWindowW(std::ptr::null(), legacy.as_ptr());
            }
            if !hwnd.is_null() {
                PostMessageW(hwnd, WM_APP + 78, usize::from(replace), 0);
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// Read physical screen coordinates, so mixed-DPI monitors don't shift the saved origin.
/// Hidden, minimized and maximized windows must not replace the normal restore position.
pub fn normal_position() -> Option<[i32; 2]> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{Foundation::RECT, UI::WindowsAndMessaging::*};
        let hwnd = WINDOW.load(Ordering::Relaxed) as windows_sys::Win32::Foundation::HWND;
        if hwnd.is_null()
            || IsWindowVisible(hwnd) == 0
            || IsIconic(hwnd) != 0
            || IsZoomed(hwnd) != 0
        {
            return None;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect) != 0 {
            return Some([rect.left, rect.top]);
        }
    }
    None
}

fn position_in_work_area(position: [i32; 2], size: [i32; 2], work: [i32; 4]) -> [i32; 2] {
    [
        position[0].clamp(work[0], work[2].saturating_sub(size[0]).max(work[0])),
        position[1].clamp(work[1], work[3].saturating_sub(size[1]).max(work[1])),
    ]
}

/// Called while eframe's new window is still hidden, before maximizing it.
pub fn restore_position(position: [i32; 2]) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::{POINT, RECT},
            Graphics::Gdi::{
                GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
            },
            UI::WindowsAndMessaging::*,
        };
        let hwnd = WINDOW.load(Ordering::Relaxed) as windows_sys::Win32::Foundation::HWND;
        if hwnd.is_null() {
            return;
        }
        let monitor = MonitorFromPoint(
            POINT {
                x: position[0],
                y: position[1],
            },
            MONITOR_DEFAULTTONEAREST,
        );
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return;
        }
        let work = info.rcWork;
        // Moving across monitors can change DPI and resize the window. Clamp again
        // using the resulting physical size, keeping the title bar reachable.
        for _ in 0..2 {
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect) == 0 {
                return;
            }
            let target = position_in_work_area(
                position,
                [rect.right - rect.left, rect.bottom - rect.top],
                [work.left, work.top, work.right, work.bottom],
            );
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                target[0],
                target[1],
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    #[cfg(not(windows))]
    let _ = position;
}

#[cfg(test)]
mod position_tests {
    use super::*;
    #[test]
    fn saved_position_survives_restart_and_handles_monitor_changes() {
        use engine_workspace::storage::Preferences;
        let mut prefs: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs.window_position, None);
        prefs.window_position = Some([-1800, 120]);
        let restored: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert_eq!(restored.window_position, Some([-1800, 120]));
        assert_eq!(
            position_in_work_area([-1800, 120], [1000, 680], [-1920, 0, 0, 1080]),
            [-1800, 120]
        );
        assert_eq!(
            position_in_work_area([-1800, 120], [1000, 680], [0, 0, 1920, 1040]),
            [0, 120]
        );
        assert_eq!(
            position_in_work_area([4000, 2000], [1000, 680], [0, 40, 1920, 1080]),
            [920, 400]
        );
        assert_eq!(
            position_in_work_area([4000, 2000], [2500, 1500], [0, 40, 1920, 1080]),
            [0, 40]
        );
    }
}
