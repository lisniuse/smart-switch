use std::cell::RefCell;
use std::io;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::w;

mod startup;
mod text_input;
use text_input::{Apartment, EditFocus, EditId, FocusProbe, FocusState, Ime};

const TIMER_ID: usize = 1;
const POLL_MS: u32 = 200;
const RETRY_AFTER: Duration = Duration::from_millis(600);
const MAX_REQUESTS: u8 = 3;
const TRAY_MESSAGE: u32 = WM_APP + 1;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const TOGGLE: usize = 1;
const LANGUAGE_SETTINGS: usize = 2;
const EXIT: usize = 3;
const STARTUP: usize = 4;
const APP_ICON: usize = 101;
// The default US layout has both language and device words set to 0409.
const US_KEYBOARD: u32 = 0x0409_0409;

thread_local! {
    static APP: RefCell<App> = const { RefCell::new(App::new()) };
}

pub fn run() -> io::Result<()> {
    let mutex = unsafe { CreateMutexW(null(), 0, w!("Local\\SmartSwitch.Tray")) };
    if mutex.is_null() {
        return Err(io::Error::last_os_error());
    }
    let duplicate = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let _instance = Instance(mutex);
    if duplicate {
        return Ok(());
    }

    let com = Apartment::enter(::windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    APP.with_borrow_mut(|app| {
        app.ime = com.as_ref().and_then(|_| Ime::new());
        app.focus_probe = FocusProbe::start();
    });
    let result = run_tray();
    APP.with_borrow_mut(|app| {
        app.ime = None;
        app.focus_probe = None;
    });
    result
}

fn run_tray() -> io::Result<()> {
    // Match the physical coordinates returned by DWM on mixed-DPI displays.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let module = unsafe { GetModuleHandleW(null()) };
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: module,
        lpszClassName: w!("SmartSwitch.TrayWindow"),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(io::Error::last_os_error());
    }
    APP.with_borrow_mut(|app| app.taskbar_created = taskbar_created);

    // A hidden top-level window receives Explorer's TaskbarCreated broadcast.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.lpszClassName,
            w!("Smart Switch"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            module,
            null(),
        )
    };
    if hwnd.is_null() {
        return Err(io::Error::last_os_error());
    }
    if unsafe { SetTimer(hwnd, TIMER_ID, POLL_MS, None) } == 0 {
        let error = io::Error::last_os_error();
        unsafe { DestroyWindow(hwnd) };
        return Err(error);
    }
    APP.with_borrow_mut(|app| app.add_tray(hwnd));

    let mut message = MSG::default();
    loop {
        match unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } {
            -1 => {
                let error = io::Error::last_os_error();
                unsafe { DestroyWindow(hwnd) };
                return Err(error);
            }
            0 => return Ok(()),
            _ => unsafe { DispatchMessageW(&message) },
        };
    }
}

struct Instance(HANDLE);

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TIMER if wparam == TIMER_ID => APP.with(|state| {
            if let Ok(mut app) = state.try_borrow_mut() {
                app.tick(hwnd);
            }
        }),
        TRAY_MESSAGE => {
            let event = lparam as u16 as u32;
            let Some(modern) = APP.with(|state| state.try_borrow().ok().map(|app| app.modern_tray))
            else {
                return 0;
            };
            if (modern && matches!(event, WM_CONTEXTMENU | NIN_KEYSELECT))
                || (!modern && event == WM_RBUTTONUP)
            {
                let anchor = (event == NIN_KEYSELECT).then_some(POINT {
                    x: wparam as i16 as i32,
                    y: (wparam >> 16) as i16 as i32,
                });
                show_menu(hwnd, anchor);
            }
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(hwnd) };
        }
        WM_DESTROY => unsafe {
            KillTimer(hwnd, TIMER_ID);
            Shell_NotifyIconW(NIM_DELETE, &tray_data(hwnd));
            PostQuitMessage(0);
        },
        _ => {
            let handled = APP.with(|state| {
                let Ok(mut app) = state.try_borrow_mut() else {
                    return false;
                };
                if app.taskbar_created != message {
                    return false;
                }
                app.add_tray(hwnd);
                true
            });
            if !handled {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
        }
    }
    0
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

fn show_menu(hwnd: HWND, anchor: Option<POINT>) {
    let enabled = APP.with_borrow(|app| app.enabled);
    let startup = startup::enabled();
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return;
    }

    let point = anchor.unwrap_or_else(|| {
        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point) };
        point
    });
    unsafe {
        AppendMenuW(
            menu,
            MF_STRING,
            TOGGLE,
            if enabled {
                w!("暂停自动切换")
            } else {
                w!("恢复自动切换")
            },
        );
        let startup_flags = match &startup {
            Ok(true) => MF_CHECKED,
            Ok(false) => MF_UNCHECKED,
            Err(_) => MF_GRAYED,
        };
        AppendMenuW(menu, MF_STRING | startup_flags, STARTUP, w!("开机启动"));
        AppendMenuW(menu, MF_STRING, LANGUAGE_SETTINGS, w!("Windows 语言设置"));
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, EXIT, w!("退出"));
        SetForegroundWindow(hwnd);
    }
    // The menu runs its own message loop. No App borrow may outlive this call.
    let command = unsafe {
        let command = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
            point.x,
            point.y,
            0,
            hwnd,
            null(),
        );
        DestroyMenu(menu);
        PostMessageW(hwnd, WM_NULL, 0, 0);
        command as usize
    };
    match command {
        TOGGLE => APP.with_borrow_mut(|app| {
            app.enabled = !app.enabled;
            app.entry = None;
            app.tick(hwnd);
        }),
        STARTUP => {
            if let Ok(enabled) = startup
                && let Err(error) = startup::set_enabled(!enabled)
            {
                let detail = if error.kind() == io::ErrorKind::InvalidInput {
                    "程序路径过长，请移到层级较浅的文件夹。".to_owned()
                } else if let Some(code) = error.raw_os_error() {
                    format!("Windows 错误 {code}。")
                } else {
                    "无法读取当前程序路径。".to_owned()
                };
                let mut data = tray_data(hwnd);
                data.uFlags = NIF_INFO;
                data.dwInfoFlags = NIIF_ERROR;
                write_wide(&mut data.szInfoTitle, "无法更改开机启动");
                write_wide(&mut data.szInfo, &detail);
                unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
            }
        }
        LANGUAGE_SETTINGS => unsafe {
            ShellExecuteW(
                hwnd,
                w!("open"),
                w!("ms-settings:regionlanguage"),
                null(),
                null(),
                SW_SHOWNORMAL,
            );
        },
        EXIT => unsafe {
            DestroyWindow(hwnd);
        },
        _ => {}
    }
}

fn write_wide(buffer: &mut [u16], message: &str) {
    for (slot, value) in buffer.iter_mut().zip(message.encode_utf16()) {
        *slot = value;
    }
}

fn tray_icon() -> HICON {
    unsafe {
        let icon = LoadImageW(
            GetModuleHandleW(null()),
            APP_ICON as *const u16,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_SHARED,
        );
        if icon.is_null() {
            LoadIconW(null_mut(), IDI_APPLICATION)
        } else {
            icon
        }
    }
}

struct App {
    enabled: bool,
    taskbar_created: u32,
    tray_added: bool,
    modern_tray: bool,
    next_tray_attempt: Option<Instant>,
    status: Status,
    entry: Option<Entry>,
    focus_probe: Option<FocusProbe>,
    ime: Option<Ime>,
}

impl App {
    const fn new() -> Self {
        Self {
            enabled: true,
            taskbar_created: 0,
            tray_added: false,
            modern_tray: false,
            next_tray_attempt: None,
            status: Status::Watching,
            entry: None,
            focus_probe: None,
            ime: None,
        }
    }

    fn add_tray(&mut self, hwnd: HWND) {
        let mut data = tray_data(hwnd);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = TRAY_MESSAGE;
        data.hIcon = tray_icon();
        self.status.write_tooltip(&mut data.szTip);
        self.tray_added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0;
        self.next_tray_attempt = if self.tray_added {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            self.modern_tray = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) } != 0;
            None
        } else {
            Some(Instant::now() + Duration::from_secs(2))
        };
    }

    fn tick(&mut self, hwnd: HWND) {
        if self
            .next_tray_attempt
            .is_some_and(|due| Instant::now() >= due)
        {
            self.add_tray(hwnd);
        }
        let status = self.update();
        if self.status == status {
            return;
        }
        self.status = status;
        if self.tray_added {
            let mut data = tray_data(hwnd);
            data.uFlags = NIF_TIP | NIF_SHOWTIP;
            status.write_tooltip(&mut data.szTip);
            unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
        }
    }

    fn update(&mut self) -> Status {
        if !self.enabled {
            if let Some(probe) = &mut self.focus_probe {
                probe.clear();
            }
            return Status::Paused;
        }
        let foreground = unsafe { GetForegroundWindow() };
        // A transient null foreground during Alt+Tab or a mode switch is not an exit.
        if foreground.is_null() {
            return self.status;
        }
        let mode = if is_fullscreen(foreground) {
            if let Some(probe) = &mut self.focus_probe {
                probe.clear();
            }
            Some(Mode::Fullscreen)
        } else {
            let (target, _) = keyboard_target(foreground);
            let focus = EditFocus::new(foreground, target);
            let state = self
                .focus_probe
                .as_mut()
                .map_or(FocusState::Other, |probe| probe.inspect(focus));
            match state {
                FocusState::Edit(edit) => Some(Mode::Edit { focus, edit }),
                FocusState::Pending
                    if self.entry.as_ref().is_some_and(|entry| {
                        matches!(entry.mode, Mode::Edit { focus: previous, .. } if previous == focus)
                    }) =>
                {
                    return self.status;
                }
                FocusState::Pending | FocusState::Other => None,
            }
        };
        let Some(mode) = mode else {
            self.entry = None;
            return Status::Watching;
        };
        if self
            .entry
            .as_ref()
            .is_none_or(|entry| entry.foreground != foreground || entry.mode != mode)
        {
            self.entry = Some(Entry {
                foreground,
                mode,
                requests: 0,
                retry_at: Instant::now(),
                status: Status::Switching,
            });
        }
        self.entry.as_mut().unwrap().update(self.ime.as_mut())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Watching,
    Paused,
    Switching,
    UsActive,
    MissingUs,
    UsFailed,
    DoubaoActive,
    MissingDoubao,
    DoubaoFailed,
}

impl Status {
    fn write_tooltip(self, tip: &mut [u16]) {
        let status = match self {
            Self::Watching => "等待全屏窗口或输入框",
            Self::Paused => "已暂停",
            Self::Switching => "正在切换键盘",
            Self::UsActive => "已为本次全屏启用美式键盘",
            Self::MissingUs => "请在 Windows 中添加英语（美国）的美式键盘",
            Self::UsFailed => "全屏窗口未接受键盘切换",
            Self::DoubaoActive => "已为当前输入框切到豆包输入法",
            Self::MissingDoubao => "请安装并启用豆包输入法",
            Self::DoubaoFailed => "当前输入框未接受豆包输入法切换",
        };
        for (slot, value) in tip.iter_mut().zip(
            "Smart Switch · "
                .encode_utf16()
                .chain(status.encode_utf16()),
        ) {
            *slot = value;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Fullscreen,
    Edit { focus: EditFocus, edit: EditId },
}

struct Entry {
    foreground: HWND,
    mode: Mode,
    requests: u8,
    retry_at: Instant,
    status: Status,
}

impl Entry {
    fn update(&mut self, ime: Option<&mut Ime>) -> Status {
        // Once confirmed, respect a manual language change until the next entry.
        if matches!(
            self.status,
            Status::UsActive | Status::UsFailed | Status::DoubaoActive | Status::DoubaoFailed
        ) || Instant::now() < self.retry_at
        {
            return self.status;
        }
        let (target, thread) = keyboard_target(self.foreground);
        if thread == 0 || unsafe { GetForegroundWindow() } != self.foreground {
            return self.status;
        }
        self.status = match self.mode {
            Mode::Fullscreen => self.switch_to_us(target, thread),
            Mode::Edit { focus, .. } if focus == EditFocus::new(self.foreground, target) => {
                self.switch_to_doubao(target, thread, ime)
            }
            Mode::Edit { .. } => self.status,
        };
        self.status
    }

    fn switch_to_us(&mut self, target: HWND, thread: u32) -> Status {
        if is_us_layout(unsafe { GetKeyboardLayout(thread) }) {
            Status::UsActive
        } else if self.requests >= MAX_REQUESTS {
            Status::UsFailed
        } else if let Some(layout) = us_keyboard() {
            if unsafe { PostMessageW(target, WM_INPUTLANGCHANGEREQUEST, 0, layout as LPARAM) } != 0
            {
                self.requests += 1;
                self.retry_at = Instant::now() + RETRY_AFTER;
                Status::Switching
            } else {
                Status::UsFailed
            }
        } else {
            self.retry_at = Instant::now() + Duration::from_secs(2);
            Status::MissingUs
        }
    }

    fn switch_to_doubao(&mut self, target: HWND, thread: u32, ime: Option<&mut Ime>) -> Status {
        let Some(ime) = ime else {
            return Status::DoubaoFailed;
        };
        let Some(profile) = ime.doubao_profile() else {
            self.retry_at = Instant::now() + Duration::from_secs(2);
            return Status::MissingDoubao;
        };
        let current = unsafe { GetKeyboardLayout(thread) };
        if current as usize as u16 == profile.langid && ime.is_active(&profile) {
            return Status::DoubaoActive;
        }
        if self.requests >= MAX_REQUESTS {
            return Status::DoubaoFailed;
        }
        if unsafe { GetForegroundWindow() } != self.foreground
            || keyboard_target(self.foreground).0 != target
            || is_fullscreen(self.foreground)
        {
            return self.status;
        }
        if !ime.activate(&profile) {
            return Status::DoubaoFailed;
        }
        self.requests += 1;
        self.retry_at = Instant::now() + RETRY_AFTER;
        Status::Switching
    }
}

fn keyboard_target(foreground: HWND) -> (HWND, u32) {
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(0, &mut info) } != 0
        && !info.hwndFocus.is_null()
        && unsafe { GetAncestor(info.hwndFocus, GA_ROOT) } == foreground
    {
        let thread = unsafe { GetWindowThreadProcessId(info.hwndFocus, null_mut()) };
        if thread != 0 {
            return (info.hwndFocus, thread);
        }
    }
    (foreground, unsafe {
        GetWindowThreadProcessId(foreground, null_mut())
    })
}

fn is_us_layout(layout: HKL) -> bool {
    layout as usize as u32 == US_KEYBOARD
}

fn us_keyboard() -> Option<HKL> {
    let count = unsafe { GetKeyboardLayoutList(0, null_mut()) };
    if count <= 0 {
        return None;
    }
    let mut layouts = vec![null_mut(); count as usize];
    let copied = unsafe { GetKeyboardLayoutList(count, layouts.as_mut_ptr()) };
    layouts.truncate(copied.max(0) as usize);
    layouts.into_iter().find(|&layout| is_us_layout(layout))
}

fn is_fullscreen(hwnd: HWND) -> bool {
    if unsafe { IsWindowVisible(hwnd) } == 0
        || unsafe { IsIconic(hwnd) } != 0
        || hwnd == unsafe { GetShellWindow() }
        || hwnd == unsafe { GetDesktopWindow() }
    {
        return false;
    }
    let mut class = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    if [
        "Progman",
        "WorkerW",
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
    ]
    .iter()
    .any(|name| {
        name.encode_utf16()
            .eq(class[..len as usize].iter().copied())
    }) {
        return false;
    }

    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    if style & WS_CAPTION == WS_CAPTION && unsafe { IsZoomed(hwnd) } != 0 {
        return false;
    }

    let display = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL) };
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if display.is_null() || unsafe { GetMonitorInfoW(display, &mut monitor) } == 0 {
        return false;
    }

    let mut bounds = RECT::default();
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&mut bounds as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
    };
    if result < 0 && unsafe { GetWindowRect(hwnd, &mut bounds) } == 0 {
        return false;
    }
    covers_monitor(bounds, monitor.rcMonitor)
}

fn covers_monitor(window: RECT, monitor: RECT) -> bool {
    const EDGE_MARGIN: i32 = 2;
    window.left <= monitor.left + EDGE_MARGIN
        && window.top <= monitor.top + EDGE_MARGIN
        && window.right >= monitor.right - EDGE_MARGIN
        && window.bottom >= monitor.bottom - EDGE_MARGIN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_fullscreen_on_a_secondary_monitor() {
        let monitor = RECT {
            left: -2560,
            top: -120,
            right: 0,
            bottom: 1320,
        };
        assert!(covers_monitor(monitor, monitor));
        assert!(covers_monitor(
            RECT {
                right: -2,
                ..monitor
            },
            monitor
        ));
        assert!(!covers_monitor(
            RECT {
                bottom: 1272,
                ..monitor
            },
            monitor
        ));
        assert!(!covers_monitor(
            RECT {
                left: -2400,
                ..monitor
            },
            monitor
        ));
    }

    #[test]
    fn distinguishes_us_from_other_english_layouts() {
        assert!(is_us_layout(0x0409_0409 as HKL));
        assert!(!is_us_layout(0xF002_0409 as HKL));
        assert!(!is_us_layout(0x0809_0809 as HKL));
    }
}
