//! Native, keyboard-accessible Win32 settings. No embedded browser or IPC bridge.
use super::windows::Context;
use std::{
    cell::{Cell, RefCell},
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

const CLASS: PCWSTR = w!("TesseraRustSettings");
const LICENSE_CLASS: PCWSTR = w!("TesseraRustLicense");
const ADAPTER: i32 = 100;
const RESET: i32 = 101;
const SAVE: i32 = 102;
const REVERT: i32 = 103;
const PAUSE: i32 = 104;
const LICENSE: i32 = 105;
const CONNECTION: i32 = 106;
const MESSAGE: i32 = 107;
const LICENSE_TEXT: i32 = 108;
const WIDTH: i32 = 640;
const HEIGHT: i32 = 556;

pub(super) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

#[derive(Default)]
pub(super) struct Window {
    hwnd: Cell<HWND>,
    license: Cell<HWND>,
    font: Cell<HFONT>,
    license_font: Cell<HFONT>,
    adapters: RefCell<Vec<String>>,
}

pub(super) fn register(instance: HINSTANCE, icon: HICON) -> Result<(), String> {
    // SAFETY: classes use static names and live procedures, with shared resources.
    unsafe {
        for (name, proc) in [
            (
                CLASS,
                window_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
            ),
            (LICENSE_CLASS, license_proc),
        ] {
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance,
                hIcon: icon,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: (COLOR_BTNFACE + 1) as HBRUSH,
                lpszClassName: name,
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                let error = std::io::Error::last_os_error().to_string();
                UnregisterClassW(CLASS, instance);
                return Err(error);
            }
        }
    }
    Ok(())
}

pub(super) fn unregister(instance: HINSTANCE) {
    // SAFETY: the owning context and all its windows have already been destroyed.
    unsafe {
        UnregisterClassW(LICENSE_CLASS, instance);
        UnregisterClassW(CLASS, instance);
    }
}

impl Window {
    pub fn open(&self, context: &Context) {
        // SAFETY: all settings window operations run on the application thread.
        unsafe {
            if !self.hwnd.get().is_null() {
                ShowWindow(self.hwnd.get(), SW_RESTORE);
                SetForegroundWindow(self.hwnd.get());
                return;
            }
            if let Err(error) = self.create(context) {
                self.close();
                self.error(&error);
                return;
            }
            let hwnd = self.hwnd.get();
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
            SetFocus(GetDlgItem(hwnd, ADAPTER));
        }
    }

    fn create(&self, context: &Context) -> Result<(), String> {
        // SAFETY: the application context outlives this window and its controls.
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_CONTROLPARENT,
                CLASS,
                w!("Tessera · 设置"),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                WIDTH,
                HEIGHT,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                (context as *const Context).cast(),
            );
            if hwnd.is_null() {
                return Err("无法创建设置窗口。".into());
            }
            self.build()?;
            self.resize(GetDpiForWindow(hwnd));
            center(hwnd);
            self.fill(context);
            self.refresh(context);
            if SetTimer(hwnd, 1, 1_000, None) == 0 {
                return Err("无法创建设置状态刷新计时器。".into());
            }
        }
        Ok(())
    }

    fn build(&self) -> Result<(), String> {
        // SAFETY: each native control is parented to the live settings window.
        unsafe {
            for control in controls() {
                let class = wide(control.class);
                let text = wide(control.text);
                let child = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    text.as_ptr(),
                    WS_CHILD | WS_VISIBLE | control.style,
                    0,
                    0,
                    0,
                    0,
                    self.hwnd.get(),
                    control.id as usize as HMENU,
                    GetModuleHandleW(null()),
                    null(),
                );
                if child.is_null() {
                    return Err("无法创建设置控件。".into());
                }
            }
            SendMessageW(GetDlgItem(self.hwnd.get(), ADAPTER), CB_SETMINVISIBLE, 8, 0);
        }
        Ok(())
    }

    fn resize(&self, dpi: u32) {
        // SAFETY: controls and the window belong to this thread; a new font is
        // assigned to every control before the previous one is released.
        unsafe {
            let dpi = dpi.max(96);
            let px = |value| scale(value, dpi);
            let font = message_font(dpi);
            if !font.is_null() {
                for control in controls() {
                    SendMessageW(
                        GetDlgItem(self.hwnd.get(), control.id),
                        WM_SETFONT,
                        font as usize,
                        1,
                    );
                }
                let old = self.font.replace(font);
                if !old.is_null() {
                    DeleteObject(old);
                }
            }
            for control in controls() {
                let [x, y, width, height] = control.bounds;
                MoveWindow(
                    GetDlgItem(self.hwnd.get(), control.id),
                    px(x),
                    px(y),
                    px(width),
                    px(height),
                    1,
                );
            }
            // Fixed logical client size keeps every option visible at each DPI.
            let mut bounds = RECT {
                left: 0,
                top: 0,
                right: px(WIDTH),
                bottom: px(HEIGHT),
            };
            AdjustWindowRectExForDpi(
                &mut bounds,
                GetWindowLongPtrW(self.hwnd.get(), GWL_STYLE) as u32,
                0,
                WS_EX_CONTROLPARENT,
                dpi,
            );
            SetWindowPos(
                self.hwnd.get(),
                null_mut(),
                0,
                0,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    fn selected(&self) -> Option<String> {
        // SAFETY: combo box is live while settings are open. Index zero is auto.
        let index =
            unsafe { SendMessageW(GetDlgItem(self.hwnd.get(), ADAPTER), CB_GETCURSEL, 0, 0) };
        usize::try_from(index)
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| self.adapters.borrow().get(index).cloned())
    }

    fn adapters(&self, available: &[String], selected: Option<&str>) {
        let names = adapter_choices(available, selected);
        // SAFETY: CB_ADDSTRING copies NUL-terminated strings synchronously. Do
        // not reset a dropdown while the user is navigating its options.
        unsafe {
            let combo = GetDlgItem(self.hwnd.get(), ADAPTER);
            if SendMessageW(combo, CB_GETDROPPEDSTATE, 0, 0) != 0 {
                return;
            }
            if *self.adapters.borrow() != names || SendMessageW(combo, CB_GETCOUNT, 0, 0) == 0 {
                SendMessageW(combo, WM_SETREDRAW, 0, 0);
                SendMessageW(combo, CB_RESETCONTENT, 0, 0);
                let automatic = wide("自动（物理以太网和 Wi-Fi）");
                SendMessageW(combo, CB_ADDSTRING, 0, automatic.as_ptr() as isize);
                for name in &names {
                    let name = wide(name);
                    SendMessageW(combo, CB_ADDSTRING, 0, name.as_ptr() as isize);
                }
                *self.adapters.borrow_mut() = names;
                SendMessageW(combo, WM_SETREDRAW, 1, 0);
                InvalidateRect(combo, null(), 1);
            }
            let index = selected
                .and_then(|name| self.adapters.borrow().iter().position(|item| item == name))
                .map_or(0, |index| index + 1);
            SendMessageW(combo, CB_SETCURSEL, index, 0);
        }
    }

    fn fill(&self, context: &Context) {
        let selected = context
            .state
            .settings
            .read()
            .ok()
            .and_then(|settings| settings.adapter.clone());
        let available = context
            .state
            .snapshot
            .read()
            .map(|value| value.adapters.clone())
            .unwrap_or_default();
        self.adapters(&available, selected.as_deref());
    }

    pub fn refresh(&self, context: &Context) {
        if self.hwnd.get().is_null() {
            return;
        }
        let snapshot = context
            .state
            .snapshot
            .read()
            .map(|value| value.clone())
            .unwrap_or_default();
        let selected = self.selected();
        self.adapters(&snapshot.adapters, selected.as_deref());
        let paused = context
            .state
            .paused
            .lock()
            .map(|value| value.paused)
            .unwrap_or(false);
        let status = if paused {
            "监控已暂停"
        } else if snapshot.adapter_missing {
            "所选网卡当前不可用"
        } else if snapshot.ready {
            "正在监控"
        } else {
            "正在等待物理网卡…"
        };
        self.text(CONNECTION, status);
        self.text(
            PAUSE,
            if paused {
                "继续监控"
            } else {
                "暂停监控"
            },
        );
    }

    fn text(&self, id: i32, text: &str) {
        // SAFETY: SetWindowText copies the string while the child is alive.
        unsafe {
            let child = GetDlgItem(self.hwnd.get(), id);
            let text = wide(text);
            let mut old = vec![0; GetWindowTextLengthW(child).max(0) as usize + 1];
            GetWindowTextW(child, old.as_mut_ptr(), old.len() as i32);
            if old != text {
                SetWindowTextW(child, text.as_ptr());
            }
        }
    }

    fn error(&self, error: &str) {
        // SAFETY: owner may be null on initialization failure; text is valid.
        unsafe {
            MessageBoxW(
                self.hwnd.get(),
                wide(error).as_ptr(),
                w!("Tessera"),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    fn command(&self, context: &Context, id: i32, notification: u16) {
        match id {
            1 | SAVE => {
                let result = context
                    .state
                    .settings
                    .read()
                    .map_err(|e| e.to_string())
                    .map(|value| value.clone())
                    .and_then(|mut settings| {
                        settings.adapter = self.selected();
                        context.state.save_settings(settings)
                    });
                match result {
                    Ok(_) => self.text(MESSAGE, "设置已保存。"),
                    Err(error) => self.error(&error),
                }
            }
            2 => self.close(), // Escape through IsDialogMessage.
            REVERT => {
                self.fill(context);
                self.text(MESSAGE, "已撤销尚未保存的更改。");
            }
            RESET => match context.state.set_widget_position(None) {
                Ok(()) => self.text(MESSAGE, "已恢复任务栏默认位置。"),
                Err(error) => self.error(&error),
            },
            PAUSE => {
                if let Ok(mut pause) = context.state.paused.lock() {
                    let next = !pause.paused;
                    pause.set(next);
                }
                self.refresh(context);
            }
            LICENSE => self.open_license(context),
            ADAPTER if notification == CBN_SELCHANGE as u16 => {
                self.text(MESSAGE, "点击“保存”以应用网卡更改。")
            }
            _ => (),
        }
    }

    fn open_license(&self, context: &Context) {
        // SAFETY: license is an owned native window, destroyed before settings.
        unsafe {
            if !self.license.get().is_null() {
                SetForegroundWindow(self.license.get());
                return;
            }
            let dpi = GetDpiForWindow(self.hwnd.get());
            let hwnd = CreateWindowExW(
                WS_EX_CONTROLPARENT,
                LICENSE_CLASS,
                w!("GNU GPL v3 · Tessera"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                scale(720, dpi),
                scale(580, dpi),
                self.hwnd.get(),
                null_mut(),
                GetModuleHandleW(null()),
                (context as *const Context).cast(),
            );
            if hwnd.is_null() {
                self.error("无法打开许可证窗口。");
                return;
            }
            let text = wide(
                &include_str!("../../../../LICENSE")
                    .replace("\r\n", "\n")
                    .replace('\n', "\r\n"),
            );
            let edit = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                text.as_ptr(),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | WS_VSCROLL
                    | ES_MULTILINE as u32
                    | ES_READONLY as u32
                    | ES_AUTOVSCROLL as u32,
                0,
                0,
                0,
                0,
                hwnd,
                LICENSE_TEXT as usize as HMENU,
                GetModuleHandleW(null()),
                null(),
            );
            if edit.is_null() {
                DestroyWindow(hwnd);
                self.error("无法显示许可证。");
                return;
            }
            self.resize_license_font(GetDpiForWindow(hwnd));
            license_layout(hwnd);
            center(hwnd);
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
            SetFocus(edit);
        }
    }

    fn resize_license_font(&self, dpi: u32) {
        // SAFETY: assign the new font before deleting the previously owned one.
        unsafe {
            let font = message_font(dpi);
            if !font.is_null() {
                SendMessageW(
                    GetDlgItem(self.license.get(), LICENSE_TEXT),
                    WM_SETFONT,
                    font as usize,
                    1,
                );
                let old = self.license_font.replace(font);
                if !old.is_null() {
                    DeleteObject(old);
                }
            }
        }
    }

    pub fn translate(&self, message: &MSG) -> bool {
        // SAFETY: IsDialogMessage provides Tab, Shift+Tab, Enter, Escape and
        // native control keyboard navigation on these modeless windows.
        unsafe {
            for hwnd in [self.license.get(), self.hwnd.get()] {
                if !hwnd.is_null() && (message.hwnd == hwnd || IsChild(hwnd, message.hwnd) != 0) {
                    return IsDialogMessageW(hwnd, message) != 0;
                }
            }
        }
        false
    }

    pub fn close(&self) {
        // SAFETY: destruction synchronously clears handles in WM_NCDESTROY.
        unsafe {
            if !self.license.get().is_null() {
                DestroyWindow(self.license.get());
            }
            if !self.hwnd.get().is_null() {
                DestroyWindow(self.hwnd.get());
            }
        }
    }
}

struct Control {
    class: &'static str,
    text: &'static str,
    id: i32,
    style: u32,
    bounds: [i32; 4],
}

fn controls() -> Vec<Control> {
    let label = |text, id, bounds| Control {
        class: "STATIC",
        text,
        id,
        style: 0,
        bounds,
    };
    let button = |text, id, bounds| Control {
        class: "BUTTON",
        text,
        id,
        style: WS_TABSTOP | BS_PUSHBUTTON as u32,
        bounds,
    };
    let group = |text, id, bounds| Control {
        class: "BUTTON",
        text,
        id,
        style: BS_GROUPBOX as u32,
        bounds,
    };
    vec![
        label("Tessera  ·  设置", 200, [24, 20, 400, 24]),
        label("正在初始化…", CONNECTION, [24, 54, 400, 24]),
        button("暂停监控", PAUSE, [492, 44, 124, 32]),
        group("网络配置", 201, [24, 92, 592, 140]),
        label(
            "默认汇总物理以太网和 Wi-Fi，排除 VPN、虚拟组网及回环接口。\n包含局域网流量，也可指定一张物理网卡。",
            202,
            [40, 120, 550, 42],
        ),
        Control {
            class: "COMBOBOX",
            text: "",
            id: ADAPTER,
            style: WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32 | CBS_HASSTRINGS as u32,
            bounds: [40, 180, 552, 240],
        },
        group("任务栏位置", 203, [24, 252, 592, 98]),
        label(
            "按住监控显示区域任意位置拖动。\n松开后自动保存，重启后仍然保留。",
            204,
            [40, 284, 362, 44],
        ),
        button("恢复默认位置", RESET, [442, 286, 150, 32]),
        group("关于", 205, [24, 370, 592, 78]),
        label(
            concat!(
                "Tessera  ",
                env!("CARGO_PKG_VERSION"),
                "\n使用 Rust 构建 · GNU GPL v3"
            ),
            206,
            [40, 398, 382, 40],
        ),
        button("查看许可证", LICENSE, [442, 398, 150, 32]),
        label("", MESSAGE, [24, 462, 592, 36]),
        button("撤销更改", REVERT, [366, 506, 120, 32]),
        Control {
            class: "BUTTON",
            text: "保存",
            id: SAVE,
            style: WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            bounds: [498, 506, 118, 32],
        },
    ]
}

fn adapter_choices(available: &[String], selected: Option<&str>) -> Vec<String> {
    let mut names = available.to_vec();
    if let Some(selected) = selected {
        names.push(selected.to_owned());
    }
    names.sort();
    names.dedup();
    names
}

fn message_font(dpi: u32) -> HFONT {
    // SAFETY: initialized structure and a writable system-font description.
    unsafe {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        if SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            (&mut metrics as *mut NONCLIENTMETRICSW).cast(),
            0,
            dpi.max(96),
        ) != 0
        {
            CreateFontIndirectW(&metrics.lfMessageFont)
        } else {
            null_mut()
        }
    }
}

fn scale(value: i32, dpi: u32) -> i32 {
    (value * dpi.max(96) as i32 + 48) / 96
}

unsafe fn center(hwnd: HWND) {
    // SAFETY: use the nearest monitor work area, including its nonzero origin.
    unsafe {
        let mut bounds = RECT::default();
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetWindowRect(hwnd, &mut bounds) != 0
            && GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) != 0
        {
            let area = info.rcWork;
            let x = area.left + ((area.right - area.left - bounds.right + bounds.left) / 2).max(0);
            let y = area.top + ((area.bottom - area.top - bounds.bottom + bounds.top) / 2).max(0);
            SetWindowPos(
                hwnd,
                null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

unsafe fn license_layout(hwnd: HWND) {
    // SAFETY: edit is a child of hwnd, all dimensions are physical pixels.
    unsafe {
        let mut bounds = RECT::default();
        GetClientRect(hwnd, &mut bounds);
        let margin = scale(16, GetDpiForWindow(hwnd));
        MoveWindow(
            GetDlgItem(hwnd, LICENSE_TEXT),
            margin,
            margin,
            (bounds.right - margin * 2).max(0),
            (bounds.bottom - margin * 2).max(0),
            1,
        );
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: boxed application context outlives settings and all children.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            (*(create.lpCreateParams as *const Context))
                .settings
                .hwnd
                .set(hwnd);
        }
        if let Some(context) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Context).as_ref() {
            let window = &context.settings;
            match message {
                WM_COMMAND => {
                    window.command(context, wparam as u16 as i32, (wparam >> 16) as u16);
                    return 0;
                }
                WM_TIMER => {
                    window.refresh(context);
                    return 0;
                }
                WM_DPICHANGED => {
                    let bounds = &*(lparam as *const RECT);
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        bounds.left,
                        bounds.top,
                        bounds.right - bounds.left,
                        bounds.bottom - bounds.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    window.resize(wparam as u16 as u32);
                    return 0;
                }
                WM_SETTINGCHANGE => {
                    window.resize(GetDpiForWindow(hwnd));
                    return 0;
                }
                WM_CLOSE => {
                    window.close();
                    return 0;
                }
                WM_NCDESTROY => {
                    window.hwnd.set(null_mut());
                    window.adapters.borrow_mut().clear();
                    let font = window.font.replace(null_mut());
                    if !font.is_null() {
                        DeleteObject(font);
                    }
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                _ => (),
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

unsafe extern "system" fn license_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the license window shares its owner's stable context pointer.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            (*(create.lpCreateParams as *const Context))
                .settings
                .license
                .set(hwnd);
        }
        match message {
            WM_SIZE => {
                license_layout(hwnd);
                return 0;
            }
            WM_DPICHANGED => {
                let bounds = &*(lparam as *const RECT);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    bounds.left,
                    bounds.top,
                    bounds.right - bounds.left,
                    bounds.bottom - bounds.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                if let Some(context) =
                    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Context).as_ref()
                {
                    context.settings.resize_license_font(wparam as u16 as u32);
                }
                return 0;
            }
            WM_SETTINGCHANGE => {
                if let Some(context) =
                    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Context).as_ref()
                {
                    context.settings.resize_license_font(GetDpiForWindow(hwnd));
                }
                return 0;
            }
            WM_COMMAND if wparam as u16 == 2 => {
                DestroyWindow(hwnd);
                return 0;
            }
            WM_NCDESTROY => {
                if let Some(context) =
                    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Context).as_ref()
                {
                    context.settings.license.set(null_mut());
                    let font = context.settings.license_font.replace(null_mut());
                    if !font.is_null() {
                        DeleteObject(font);
                    }
                }
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            _ => (),
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_selection_is_preserved_without_duplicate_adapters() {
        let names = vec!["Wi-Fi".into(), "Ethernet".into(), "Wi-Fi".into()];
        assert_eq!(
            adapter_choices(&names, Some("USB Ethernet")),
            ["Ethernet", "USB Ethernet", "Wi-Fi"]
        );
        assert_eq!(
            adapter_choices(&names, Some("Wi-Fi")),
            ["Ethernet", "Wi-Fi"]
        );
        assert!(adapter_choices(&[], None).is_empty());
    }

    #[test]
    fn native_controls_save_revert_reset_and_reopen_without_stopping_monitoring() {
        use crate::state::AppState;
        use std::sync::atomic::Ordering;
        use tessera_core::config::{Settings, load};

        struct Classes(HINSTANCE);
        impl Drop for Classes {
            fn drop(&mut self) {
                unregister(self.0);
            }
        }
        // SAFETY: this test owns its classes, controls and context on one thread;
        // no Explorer attachment or live sampling is started.
        unsafe {
            let instance = GetModuleHandleW(null());
            let controls = INITCOMMONCONTROLSEX {
                dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_STANDARD_CLASSES,
            };
            assert_ne!(InitCommonControlsEx(&controls), 0);
            let icon = LoadIconW(null_mut(), IDI_APPLICATION);
            register(instance, icon).unwrap();
            let _classes = Classes(instance);
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            let context = Box::new(Context::new(AppState::new(
                Settings::default(),
                path.clone(),
            )));
            context.state.snapshot.write().unwrap().adapters =
                vec!["Ethernet".into(), "Wi-Fi".into()];
            let window = &context.settings;
            window.create(&context).unwrap();
            let hwnd = window.hwnd.get();
            assert!(!hwnd.is_null());
            assert!(!window.font.get().is_null());
            let combo = GetDlgItem(hwnd, ADAPTER);
            assert_eq!(SendMessageW(combo, CB_GETCOUNT, 0, 0), 3);
            SendMessageW(combo, CB_SETCURSEL, 2, 0);

            // Hot-plug refresh retains an unsaved, now-disconnected choice.
            context.state.snapshot.write().unwrap().adapters = vec!["Ethernet".into()];
            window.refresh(&context);
            assert_eq!(window.selected().as_deref(), Some("Wi-Fi"));
            context.state.set_widget_position(Some(3_456)).unwrap();
            SendMessageW(hwnd, WM_COMMAND, SAVE as usize, 0);
            let saved = load(&path).unwrap();
            assert_eq!(saved.adapter.as_deref(), Some("Wi-Fi"));
            assert_eq!(saved.taskbar_position, Some(3_456));

            SendMessageW(combo, CB_SETCURSEL, 0, 0);
            SendMessageW(hwnd, WM_COMMAND, REVERT as usize, 0);
            assert_eq!(window.selected().as_deref(), Some("Wi-Fi"));
            SendMessageW(hwnd, WM_COMMAND, RESET as usize, 0);
            assert_eq!(load(&path).unwrap().taskbar_position, None);
            SendMessageW(hwnd, WM_COMMAND, PAUSE as usize, 0);
            assert!(context.state.paused.lock().unwrap().paused);

            SendMessageW(hwnd, WM_COMMAND, LICENSE as usize, 0);
            let license = window.license.get();
            assert!(!license.is_null());
            assert!(GetWindowTextLengthW(GetDlgItem(license, LICENSE_TEXT)) > 30_000);
            SendMessageW(license, WM_CLOSE, 0, 0);
            assert!(window.license.get().is_null());
            assert!(window.license_font.get().is_null());

            // Closing discards unsaved form changes, keeps the sampler alive,
            // and releases native controls/fonts before the next open.
            SendMessageW(combo, CB_SETCURSEL, 0, 0);
            SendMessageW(hwnd, WM_CLOSE, 0, 0);
            assert!(window.hwnd.get().is_null());
            assert!(window.font.get().is_null());
            assert!(!context.state.shutdown.load(Ordering::Relaxed));
            window.create(&context).unwrap();
            assert_eq!(window.selected().as_deref(), Some("Wi-Fi"));
            window.close();
            assert_eq!(load(&path).unwrap().adapter.as_deref(), Some("Wi-Fi"));
        }
    }
}
