use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
static TRAY_PRESENT: AtomicBool = AtomicBool::new(false);
static WINDOW: AtomicIsize = AtomicIsize::new(0);
// Only the close button hides the window. Ordinary minimization stays on the taskbar.
pub fn hide_to_tray() -> bool {
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
            .zip("DoNothing&Win Launcher — click to restore".encode_utf16())
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
        if message == WM_APP + 78 {
            if wparam == 1 {
                REPLACE_REQUESTED.store(true, Ordering::SeqCst);
            }
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
            tray(hwnd, false);
            return 0;
        }
        if message == WM_NCDESTROY {
            if TRAY_PRESENT.load(Ordering::Relaxed) {
                tray(hwnd, false);
            }
            WINDOW.store(0, Ordering::Relaxed);
            RemoveWindowSubclass(hwnd, Some(frame_proc), subclass);
        }
        if message == WM_APP + 77 {
            let event = lparam as u32;
            if event == WM_LBUTTONUP
                || event == WM_LBUTTONDBLCLK
                || event == windows_sys::Win32::UI::Shell::NIN_BALLOONUSERCLICK
            {
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
                if choice == 1 {
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                    tray(hwnd, false);
                }
                if choice == 2 {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
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
