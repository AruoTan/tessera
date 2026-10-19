use super::settings;
use crate::{monitor, state::AppState, widget};
use std::{
    cell::Cell,
    mem::size_of,
    path::Path,
    ptr::{null, null_mut},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use windows_sys::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
        UI::{Controls::*, HiDpi::*, WindowsAndMessaging::*},
    },
    core::w,
};

const OPEN_SETTINGS: u32 = WM_APP + 20;
const OPEN_MENU: u32 = WM_APP + 21;
const HOST_CLASS: windows_sys::core::PCWSTR = w!("TesseraRustApplication");
const PAUSE: u32 = 2;
const QUIT: u32 = 3;

/// Only the window address crosses threads; all UI work stays on the main loop.
#[derive(Clone, Copy)]
pub struct Handle(usize);

impl Handle {
    pub fn open_settings(self) {
        // SAFETY: posting does not dereference the address, and fails if it is gone.
        unsafe {
            PostMessageW(self.0 as HWND, OPEN_SETTINGS, 0, 0);
        }
    }

    pub fn open_menu(self, x: i32, y: i32) {
        // SAFETY: only screen coordinates cross threads; the host owns the menu.
        unsafe {
            PostMessageW(self.0 as HWND, OPEN_MENU, x as WPARAM, y as LPARAM);
        }
    }
}

pub(super) struct Context {
    pub state: AppState,
    pub settings: settings::Window,
    host: Cell<HWND>,
}

struct Instance(HANDLE);
impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: this guard owns the named mutex handle, not mutex ownership.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn single_instance() -> Result<Option<Instance>, String> {
    // SAFETY: static NUL-terminated name; ownership of the handle is retained.
    unsafe {
        let mutex = CreateMutexW(null(), 0, w!("Local\\io.tessera.application"));
        if mutex.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let existing = GetLastError() == ERROR_ALREADY_EXISTS;
        let instance = Instance(mutex);
        if !existing {
            return Ok(Some(instance));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let host = FindWindowW(HOST_CLASS, null());
            if !host.is_null() {
                let mut process = 0;
                GetWindowThreadProcessId(host, &mut process);
                AllowSetForegroundWindow(process);
                if PostMessageW(host, OPEN_SETTINGS, 0, 0) != 0 {
                    return Ok(None);
                }
            }
            if Instant::now() >= deadline {
                return Err(
                    "Another instance is starting or not responding. Try again shortly.".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Context {
    pub(super) fn new(state: AppState) -> Self {
        Self {
            state,
            settings: settings::Window::default(),
            host: Cell::new(null_mut()),
        }
    }

    fn pause(&self) {
        if let Ok(mut pause) = self.state.paused.lock() {
            let next = !pause.paused;
            pause.set(next);
        }
        self.settings.refresh(self);
    }

    fn menu(&self, x: i32, y: i32) {
        // SAFETY: popup menu is created, tracked, and destroyed on this thread.
        unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            let paused = self
                .state
                .paused
                .lock()
                .map(|value| value.paused)
                .unwrap_or(false);
            AppendMenuW(
                menu,
                MF_STRING,
                PAUSE as usize,
                if paused {
                    w!("继续监控")
                } else {
                    w!("暂停监控")
                },
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, QUIT as usize, w!("退出"));
            SetForegroundWindow(self.host.get());
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                x,
                y,
                0,
                self.host.get(),
                null(),
            );
            DestroyMenu(menu);
            // Complete popup dismissal even if the user cancels.
            PostMessageW(self.host.get(), WM_NULL, 0, 0);
            match command as u32 {
                PAUSE => self.pause(),
                QUIT => PostQuitMessage(0),
                _ => (),
            }
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        self.state.shutdown.store(true, Ordering::Relaxed);
        self.settings.close();
        // SAFETY: the hidden application window belongs to this thread.
        unsafe {
            if !self.host.get().is_null() {
                DestroyWindow(self.host.get());
            }
        }
    }
}

pub fn run() -> Result<(), String> {
    let Some(_instance) = single_instance()? else {
        return Ok(());
    };
    let roaming = std::env::var_os("APPDATA")
        .ok_or("The Windows roaming profile directory is unavailable.")?;
    let path = super::settings_path(Path::new(&roaming));
    let first_launch = !path.exists();
    let state = AppState::new(tessera_core::config::load(&path)?, path);
    // SAFETY: the main thread owns the context and every window below. Context
    // outlives its windows, including all synchronous destruction callbacks.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let controls = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        if InitCommonControlsEx(&controls) == 0 {
            return Err("Cannot initialize native controls.".into());
        }
        let module = GetModuleHandleW(null());
        // LoadIcon returns a shared resource handle; it must not be destroyed.
        let icon = LoadIconW(module, 1usize as windows_sys::core::PCWSTR);
        if icon.is_null() {
            return Err("Application icon resource is missing.".into());
        }
        let class = WNDCLASSW {
            lpfnWndProc: Some(host_proc),
            hInstance: module,
            lpszClassName: HOST_CLASS,
            hIcon: icon,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if let Err(error) = settings::register(module, icon) {
            UnregisterClassW(HOST_CLASS, module);
            return Err(error);
        }
        let context = Box::new(Context::new(state));
        let result = (|| {
            // A hidden top-level receiver handles widget actions and instance activation.
            let host = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                HOST_CLASS,
                w!("Tessera"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                module,
                (&*context as *const Context).cast(),
            );
            if host.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            monitor::start(context.state.clone()).map_err(|e| e.to_string())?;
            widget::start(Handle(host as usize), context.state.clone())
                .map_err(|e| e.to_string())?;
            if first_launch {
                context.settings.open(&context);
            }
            let mut message = MSG::default();
            loop {
                let result = GetMessageW(&mut message, null_mut(), 0, 0);
                if result == -1 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                if result == 0 {
                    break;
                }
                if !context.settings.translate(&message) {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            Ok(())
        })();
        drop(context);
        settings::unregister(module);
        UnregisterClassW(HOST_CLASS, module);
        result
    }
}

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: GWLP_USERDATA points to the stable boxed context until DestroyWindow.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            (*(create.lpCreateParams as *const Context)).host.set(hwnd);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Context;
        if let Some(context) = pointer.as_ref() {
            match message {
                OPEN_SETTINGS => {
                    context.settings.open(context);
                    return 0;
                }
                OPEN_MENU => {
                    context.menu(wparam as i32, lparam as i32);
                    return 0;
                }
                WM_CLOSE => {
                    PostQuitMessage(0);
                    return 0;
                }
                WM_QUERYENDSESSION => return 1,
                WM_ENDSESSION if wparam != 0 => {
                    PostQuitMessage(0);
                    return 0;
                }
                WM_NCDESTROY => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                _ => (),
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}
