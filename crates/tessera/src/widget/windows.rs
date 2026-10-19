//! Layered taskbar child with DirectWrite text on its own native window thread.
use super::{
    bitmap::Bitmap,
    gauge::{self, Palette, Reading as GaugeReading},
    icons::{Icons, icon_size},
    presentation::{Presentation, TaskbarIdentity},
    startup::Startup,
    surface::make_hit_testable,
    text::{GaugeField, Renderer, Style},
};
use crate::state::AppState;
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr::{null, null_mut},
    sync::{atomic::Ordering, mpsc},
    thread,
    time::{Duration, Instant},
};
use tessera_core::{
    geometry::{DragSession, Rect, TaskbarLayout},
    network::{SPEED_DECIMALS, SPEED_UNITS, SpeedDisplay, format_speed},
};
use windows_sys::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::{Dwm::*, Gdi::*},
        System::{LibraryLoader::GetModuleHandleW, Registry::*},
        UI::{Accessibility::*, HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
    core::w,
};

const SYNC_TASKBAR: u32 = WM_APP + 1;
const REFRESH_TIMER: usize = 1;

thread_local! {
    // OUTOFCONTEXT WinEvents run on the thread that installed the hook.
    static HOST: Cell<HWND> = const { Cell::new(null_mut()) };
}

#[derive(Clone, PartialEq)]
struct Content {
    lines: [SpeedDisplay; 2],
    hardware: [GaugeReading; gauge::COUNT],
    visible: &'static [usize],
}

#[derive(Clone, Copy)]
struct Columns {
    number: i32,
    unit: i32,
    hardware: i32,
    padding: i32,
    icon: i32,
}

impl Columns {
    fn number_left(self) -> i32 {
        self.padding * 2 + self.icon
    }

    fn width(self) -> i32 {
        self.number_left() + self.number + self.padding + self.unit + self.padding
    }

    fn hardware_left(self, column: usize) -> i32 {
        self.width() + self.padding * 2 + column as i32 * (self.hardware + self.padding * 2)
    }

    fn total_width(self, hardware_count: usize) -> i32 {
        self.hardware_left(hardware_count - 1) + self.hardware
    }
}

pub fn show_error(message: &str) {
    let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    // SAFETY: both strings are NUL-terminated and remain alive for this modal call.
    unsafe {
        MessageBoxW(
            null_mut(),
            message.as_ptr(),
            w!("Tessera"),
            MB_OK | MB_ICONERROR,
        );
    }
}

struct Context {
    app: crate::desktop::Handle,
    state: AppState,
    host: Cell<HWND>,
    taskbar: Cell<HWND>,
    widget: Cell<HWND>,
    taskbar_created: u32,
    presentation: Cell<Presentation>,
    startup: Cell<Startup>,
    started_at: Instant,
    syncing: Cell<bool>,
    sync_queued: Cell<bool>,
    dpi: Cell<u32>,
    text: RefCell<Renderer>,
    bitmap: RefCell<Option<Bitmap>>,
    text_settings_changed: Cell<bool>,
    palette: Cell<Palette>,
    columns: Cell<Option<Columns>>,
    icons: RefCell<Option<Icons>>,
    content: RefCell<Option<Content>>,
    dirty: Cell<bool>,
    layout: Cell<Option<TaskbarLayout>>,
    drag: Cell<Option<DragSession>>,
}

pub fn start(app: crate::desktop::Handle, state: AppState) -> std::io::Result<()> {
    let (ready, started) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("taskbar-widget".into())
        .spawn(move || {
            // SAFETY: this thread owns the window, context, GDI handles, and message loop.
            unsafe {
                run(app, state, ready);
            }
        })?;
    started
        .recv_timeout(Duration::from_secs(10))
        .map_err(std::io::Error::other)?
        .map_err(std::io::Error::other)
}

unsafe fn run(
    app: crate::desktop::Handle,
    state: AppState,
    ready: mpsc::SyncSender<Result<(), String>>,
) {
    // SAFETY: all handles below are owned on this dedicated thread and checked before use.
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let text = match Renderer::new(96) {
            Ok(text) => text,
            Err(error) => {
                let _ = ready.send(Err(format!(
                    "Cannot initialize taskbar text rendering: {error}"
                )));
                return;
            }
        };
        let instance = GetModuleHandleW(null());
        let class = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: w!("TesseraRustWidget"),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            let _ = ready.send(Err(format!(
                "Cannot register widget: {}",
                std::io::Error::last_os_error()
            )));
            return;
        }
        let host_class = WNDCLASSW {
            lpfnWndProc: Some(host_proc),
            hInstance: instance,
            lpszClassName: w!("TesseraRustWidgetHost"),
            ..Default::default()
        };
        if RegisterClassW(&host_class) == 0 {
            let _ = ready.send(Err(format!(
                "Cannot register widget host: {}",
                std::io::Error::last_os_error()
            )));
            UnregisterClassW(class.lpszClassName, instance);
            return;
        }
        let context = Box::new(Context {
            app,
            state,
            host: Cell::new(null_mut()),
            taskbar: Cell::new(null_mut()),
            widget: Cell::new(null_mut()),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            presentation: Cell::new(Presentation::new(None)),
            startup: Cell::new(Startup::default()),
            started_at: Instant::now(),
            syncing: Cell::new(false),
            sync_queued: Cell::new(false),
            dpi: Cell::new(0),
            text: RefCell::new(text),
            bitmap: RefCell::new(None),
            text_settings_changed: Cell::new(false),
            palette: Cell::new(Palette::new(taskbar_dark_mode())),
            columns: Cell::new(None),
            icons: RefCell::new(None),
            content: RefCell::new(None),
            dirty: Cell::new(true),
            layout: Cell::new(None),
            drag: Cell::new(None),
        });
        // This never-visible top-level window survives Explorer and receives
        // TaskbarCreated broadcasts. The visible window is always a taskbar child.
        let host = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            host_class.lpszClassName,
            w!("Tessera widget host"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            (&*context as *const Context).cast::<c_void>(),
        );
        context.host.set(host);
        HOST.set(host);
        // The broadcast receiver must exist before taking the startup snapshot;
        // otherwise a taskbar created between lookup and CreateWindow is missed.
        context
            .presentation
            .set(Presentation::new(taskbar_identity(FindWindowW(
                w!("Shell_TrayWnd"),
                null(),
            ))));
        let hook = SetWinEventHook(
            EVENT_OBJECT_CREATE,
            EVENT_OBJECT_LOCATIONCHANGE,
            null_mut(),
            Some(taskbar_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        let initialized = if host.is_null() || SetTimer(host, REFRESH_TIMER, 250, None) == 0 {
            Err(format!(
                "Cannot initialize widget host: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            if let Err(error) = sync_taskbar(&context) {
                eprintln!("{error}");
            }
            Ok(())
        };
        let running = initialized.is_ok();
        let _ = ready.send(initialized);
        if running {
            let mut message = MSG::default();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if !hook.is_null() {
            UnhookWinEvent(hook);
        }
        HOST.set(null_mut());
        destroy_widget(&context);
        if IsWindow(host) != 0 {
            DestroyWindow(host);
        }
        UnregisterClassW(host_class.lpszClassName, instance);
        UnregisterClassW(class.lpszClassName, instance);
    }
}

unsafe extern "system" fn taskbar_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    event_thread: u32,
    _time: u32,
) {
    // SAFETY: the hook is out of context on this thread. Hide a retiring readout
    // immediately; coalesce attachment/layout work through the host queue.
    unsafe {
        if hwnd.is_null()
            || !(object == OBJID_WINDOW
                || (event == EVENT_OBJECT_REORDER && object == OBJID_CLIENT))
            || child != 0
            || !matches!(
                event,
                EVENT_OBJECT_CREATE
                    | EVENT_OBJECT_DESTROY
                    | EVENT_OBJECT_SHOW
                    | EVENT_OBJECT_HIDE
                    | EVENT_OBJECT_REORDER
                    | EVENT_OBJECT_LOCATIONCHANGE
            )
        {
            return;
        }
        HOST.with(|host| {
            let host = host.get();
            let pointer = GetWindowLongPtrW(host, GWLP_USERDATA) as *const Context;
            if pointer.is_null() {
                return;
            }
            let mut class = [0u16; 32];
            let context = &*pointer;
            let root = GetAncestor(hwnd, GA_ROOT);
            // A child may be created before FindWindow has observed the new
            // Shell_TrayWnd. Inspect its root even when our cached taskbar is gone.
            let candidate = if root.is_null() { hwnd } else { root };
            let length = GetClassNameW(candidate, class.as_mut_ptr(), class.len() as i32);
            if hwnd == context.taskbar.get()
                || (!context.taskbar.get().is_null() && root == context.taskbar.get())
                || class[..length.max(0) as usize]
                    .iter()
                    .copied()
                    .eq("Shell_TrayWnd".encode_utf16())
            {
                if event == EVENT_OBJECT_DESTROY && hwnd == context.taskbar.get() {
                    let mut presentation = context.presentation.get();
                    let retired = presentation.retire(hwnd as usize, event_thread);
                    context.presentation.set(presentation);
                    let widget = context.widget.get();
                    if retired && owns_widget(widget, (context as *const Context).cast()) {
                        hide_widget(widget, context);
                    }
                }
                if event != EVENT_OBJECT_LOCATIONCHANGE {
                    context.dirty.set(true);
                }
                queue_sync(context);
            }
        });
    }
}

unsafe fn queue_sync(context: &Context) {
    // SAFETY: the host is on this thread. Coalesce the shell's startup event burst.
    unsafe {
        if !context.sync_queued.replace(true)
            && PostMessageW(context.host.get(), SYNC_TASKBAR, 0, 0) == 0
        {
            context.sync_queued.set(false);
        }
    }
}

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the boxed context outlives both windows and their message loop.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Context;
        if !pointer.is_null() {
            let context = &*pointer;
            if matches!(message, WM_SETTINGCHANGE | WM_THEMECHANGED) {
                context.palette.set(Palette::new(taskbar_dark_mode()));
                context.text_settings_changed.set(true);
                context.dirty.set(true);
                queue_sync(context);
            }
            if message == WM_TIMER
                || message == SYNC_TASKBAR
                || (context.taskbar_created != 0 && message == context.taskbar_created)
            {
                if message == SYNC_TASKBAR {
                    context.sync_queued.set(false);
                }
                if message == context.taskbar_created {
                    // This also arrives on primary-display DPI changes. Repaint
                    // and repair the attachment even when the readout is unchanged.
                    context.dirty.set(true);
                    let identity = taskbar_identity(FindWindowW(w!("Shell_TrayWnd"), null()));
                    let mut presentation = context.presentation.get();
                    presentation.observe(identity);
                    if let Some(identity) = identity {
                        presentation.acknowledge(identity);
                    }
                    context.presentation.set(presentation);
                }
                if context.state.shutdown.load(Ordering::Relaxed) {
                    DestroyWindow(hwnd);
                } else {
                    // Validate HWND ownership before mouse handling can use it.
                    if let Err(error) = sync_taskbar(context) {
                        eprintln!("{error}");
                    }
                    let widget = context.widget.get();
                    if !widget.is_null() && IsWindowVisible(widget) == 0 {
                        cancel_drag(widget, context);
                    } else if !widget.is_null() && context.drag.get().is_some() {
                        let button = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
                            VK_RBUTTON
                        } else {
                            VK_LBUTTON
                        };
                        if GetAsyncKeyState(button as i32) < 0 {
                            move_drag(widget, context);
                        } else {
                            finish_drag(widget, context);
                        }
                    }
                }
                return 0;
            }
            match message {
                WM_DESTROY => {
                    KillTimer(hwnd, REFRESH_TIMER);
                    destroy_widget(context);
                    PostQuitMessage(0);
                    return 0;
                }
                WM_NCDESTROY => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

unsafe fn create_taskbar_child(
    taskbar: HWND,
    class: *const u16,
    instance: HINSTANCE,
    data: *const c_void,
) -> HWND {
    // SAFETY: create under the parent's DPI awareness to avoid cross-process
    // DPI resets. The caller keeps the class, parent and context alive.
    unsafe {
        let previous = SetThreadDpiAwarenessContext(GetWindowDpiAwarenessContext(taskbar));
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_NOPARENTNOTIFY,
            class,
            w!("Tessera"),
            WS_CHILD,
            0,
            0,
            140,
            40,
            taskbar,
            null_mut(),
            instance,
            data,
        );
        if !previous.is_null() {
            SetThreadDpiAwarenessContext(previous);
        }
        hwnd
    }
}

unsafe fn sync_taskbar(context: &Context) -> Result<(), String> {
    // SAFETY: shell/window APIs can re-enter this thread while Explorer rebuilds.
    // Defer nested syncs until the current creation/destruction has completed.
    unsafe {
        if context.syncing.replace(true) {
            queue_sync(context);
            return Ok(());
        }
        let result = sync_taskbar_inner(context);
        context.syncing.set(false);
        result
    }
}

unsafe fn owns_widget(hwnd: HWND, data: *const c_void) -> bool {
    // SAFETY: query only. HWNDs can be recycled during Explorer teardown, so
    // IsWindow alone is insufficient before hiding or destroying a stored handle.
    unsafe {
        !hwnd.is_null()
            && !data.is_null()
            && IsWindow(hwnd) != 0
            && GetWindowLongPtrW(hwnd, GWLP_USERDATA) == data as isize
    }
}

fn forget_destroyed_widget(current: &Cell<HWND>, destroyed: HWND) -> bool {
    if current.get() == destroyed {
        current.set(null_mut());
        true
    } else {
        false
    }
}

unsafe fn destroy_owned_widget(hwnd: HWND, data: *const c_void) {
    // SAFETY: never manipulate a window that has recycled our previous HWND.
    unsafe {
        if owns_widget(hwnd, data) {
            DestroyWindow(hwnd);
        }
    }
}

unsafe fn destroy_widget(context: &Context) {
    // SAFETY: take the handle before DestroyWindow can re-enter window_proc.
    unsafe {
        let hwnd = context.widget.replace(null_mut());
        context.drag.set(None);
        context.layout.set(None);
        destroy_owned_widget(hwnd, (context as *const Context).cast());
    }
}

unsafe fn sync_taskbar_inner(context: &Context) -> Result<(), String> {
    // SAFETY: only the host thread creates/destroys our child. A missing Explorer
    // leaves no visible fallback window; events and the host timer retry later.
    unsafe {
        let taskbar = FindWindowW(w!("Shell_TrayWnd"), null());
        let mut presentation = context.presentation.get();
        presentation.observe(taskbar_identity(taskbar));
        context.presentation.set(presentation);
        let mut hwnd = context.widget.get();
        let owned = owns_widget(hwnd, (context as *const Context).cast());
        if !hwnd.is_null()
            && (taskbar != context.taskbar.get() || !owned || GetParent(hwnd) != taskbar)
        {
            destroy_widget(context);
            hwnd = null_mut();
        }
        context.taskbar.set(taskbar);
        if taskbar.is_null() {
            SetTimer(context.host.get(), REFRESH_TIMER, 16, None);
            return Ok(());
        }
        if hwnd.is_null() {
            SetTimer(context.host.get(), REFRESH_TIMER, 16, None);
            hwnd = create_taskbar_child(
                taskbar,
                w!("TesseraRustWidget"),
                GetModuleHandleW(null()),
                (context as *const Context).cast(),
            );
            if hwnd.is_null() {
                // Explorer can disappear between lookup and creation. Keep retrying.
                return Err(format!(
                    "Cannot attach taskbar widget: {}",
                    std::io::Error::last_os_error()
                ));
            }
            context.widget.set(hwnd);
            context.content.replace(None);
            context.dirty.set(true);
        }
        refresh(hwnd, context);
        // During shell discovery or transient empty layout, retry within a frame.
        // Windows still animates the child; this timer only discovers/rebuilds it.
        let interval = if context.drag.get().is_some()
            || context.dirty.get()
            || GetWindowLongPtrW(hwnd, GWL_STYLE) & WS_VISIBLE as isize == 0
        {
            16
        } else {
            250
        };
        SetTimer(context.host.get(), REFRESH_TIMER, interval, None);
        Ok(())
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: WM_NCCREATE receives our Box's stable pointer. The box outlives the window;
    // WM_NCDESTROY clears it before the thread can drop the box. Nothing shares this pointer.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Context;
        if !pointer.is_null() {
            // Interior mutability allows Win32 re-entrancy without creating an
            // exclusive Rust reference to the context.
            match message {
                WM_PAINT => {
                    let mut paint = PAINTSTRUCT::default();
                    BeginPaint(hwnd, &mut paint);
                    render(hwnd, &*pointer);
                    EndPaint(hwnd, &paint);
                    return 0;
                }
                WM_ERASEBKGND => return 1,
                WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
                WM_SHOWWINDOW => {
                    if wparam == 0 {
                        cancel_drag(hwnd, &*pointer);
                    } else {
                        (*pointer).dirty.set(true);
                        queue_sync(&*pointer);
                    }
                }
                WM_DWMCOMPOSITIONCHANGED => {
                    (*pointer).dirty.set(true);
                    refresh(hwnd, &*pointer);
                    return 0;
                }
                WM_LBUTTONDOWN => {
                    begin_drag(hwnd, &*pointer);
                    return 0;
                }
                WM_MOUSEMOVE => {
                    move_drag(hwnd, &*pointer);
                    return 0;
                }
                WM_LBUTTONUP => {
                    finish_drag(hwnd, &*pointer);
                    return 0;
                }
                WM_CAPTURECHANGED | WM_CANCELMODE => {
                    cancel_drag(hwnd, &*pointer);
                    return 0;
                }
                WM_SETCURSOR if lparam as u16 == HTCLIENT as u16 => {
                    SetCursor(LoadCursorW(null_mut(), IDC_SIZEALL));
                    return 1;
                }
                WM_LBUTTONDBLCLK => {
                    cancel_drag(hwnd, &*pointer);
                    (*pointer).app.open_settings();
                    return 0;
                }
                WM_CONTEXTMENU => {
                    cancel_drag(hwnd, &*pointer);
                    let point = if lparam == -1 {
                        // Keyboard invocation anchors the menu to the readout.
                        let mut bounds = RECT::default();
                        if GetWindowRect(hwnd, &mut bounds) == 0 {
                            return 0;
                        }
                        POINT {
                            x: bounds.left + (bounds.right - bounds.left) / 2,
                            y: bounds.top + (bounds.bottom - bounds.top) / 2,
                        }
                    } else {
                        // WM_CONTEXTMENU packs signed screen coordinates.
                        POINT {
                            x: lparam as u16 as i16 as i32,
                            y: (lparam >> 16) as u16 as i16 as i32,
                        }
                    };
                    (*pointer).app.open_menu(point.x, point.y);
                    return 0;
                }
                WM_NCDESTROY => {
                    if forget_destroyed_widget(&(*pointer).widget, hwnd) {
                        (*pointer).drag.set(None);
                        (*pointer).layout.set(None);
                        SetTimer((*pointer).host.get(), REFRESH_TIMER, 16, None);
                        queue_sync(&*pointer);
                    }
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                WM_DESTROY => {
                    if (*pointer).widget.get() == hwnd {
                        (*pointer).drag.set(None);
                        (*pointer).layout.set(None);
                    }
                    return 0;
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

unsafe fn begin_drag(hwnd: HWND, context: &Context) {
    // SAFETY: all mouse and window APIs run on the owning window thread.
    unsafe {
        let mut cursor = POINT::default();
        let mut bounds = RECT::default();
        if context.layout.get().is_none()
            || GetCursorPos(&mut cursor) == 0
            || GetWindowRect(hwnd, &mut bounds) == 0
        {
            return;
        }
        let taskbar = context.taskbar.get();
        if ScreenToClient(taskbar, &mut cursor) == 0 {
            return;
        }
        MapWindowPoints(null_mut(), taskbar, (&mut bounds as *mut RECT).cast(), 2);
        let dpi = context.dpi.get().max(96);
        context.drag.set(Some(DragSession::new(
            (cursor.x, cursor.y),
            (bounds.left, bounds.top),
            (
                GetSystemMetricsForDpi(SM_CXDRAG, dpi).max(1),
                GetSystemMetricsForDpi(SM_CYDRAG, dpi).max(1),
            ),
        )));
        SetCapture(hwnd);
        if GetCapture() != hwnd {
            context.drag.set(None);
        } else {
            SetTimer(context.host.get(), REFRESH_TIMER, 16, None);
        }
    }
}

unsafe fn move_drag(hwnd: HWND, context: &Context) {
    // SAFETY: capture and cursor are queried only on the owning thread.
    unsafe {
        let (Some(mut drag), Some(layout)) = (context.drag.get(), context.layout.get()) else {
            return;
        };
        if GetCapture() != hwnd {
            cancel_drag(hwnd, context);
            return;
        }
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor) != 0 && ScreenToClient(context.taskbar.get(), &mut cursor) != 0
        {
            drag.update(layout, (cursor.x, cursor.y));
            context.drag.set(Some(drag));
            if drag.position.is_some() {
                refresh(hwnd, context);
            }
        }
    }
}

unsafe fn finish_drag(hwnd: HWND, context: &Context) {
    // SAFETY: clear state before ReleaseCapture, which re-enters WM_CAPTURECHANGED.
    unsafe {
        // Also account for the final movement if release was detected by the timer.
        move_drag(hwnd, context);
        let drag = context.drag.take();
        SetTimer(context.host.get(), REFRESH_TIMER, 250, None);
        if GetCapture() == hwnd {
            ReleaseCapture();
        }
        if let Some(position) = drag.and_then(|drag| drag.position)
            && let Err(error) = context.state.set_widget_position(Some(position))
        {
            show_error(&format!("Cannot save widget position: {error}"));
        }
        refresh(hwnd, context);
    }
}

unsafe fn cancel_drag(hwnd: HWND, context: &Context) {
    // SAFETY: take before releasing capture makes re-entrant cancellation harmless.
    unsafe {
        if context.drag.take().is_some() {
            SetTimer(context.host.get(), REFRESH_TIMER, 250, None);
            if GetCapture() == hwnd {
                ReleaseCapture();
            }
            refresh(hwnd, context);
        }
    }
}

unsafe fn hide_widget(hwnd: HWND, context: &Context) {
    // SAFETY: clear drag state before hiding or releasing capture, both of which
    // can send synchronous messages back to this window procedure.
    unsafe {
        if context.drag.take().is_some() {
            SetTimer(context.host.get(), REFRESH_TIMER, 250, None);
        }
        context.layout.set(None);
        if GetCapture() == hwnd {
            ReleaseCapture();
        }
        SetWindowPos(
            hwnd,
            null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE
                | SWP_NOSIZE
                | SWP_NOZORDER
                | SWP_NOACTIVATE
                | SWP_NOOWNERZORDER
                | SWP_HIDEWINDOW,
        );
    }
}

unsafe fn reveal_widget(hwnd: HWND) {
    // SAFETY: show above siblings without activating it or raising Explorer.
    // SWP_SHOWWINDOW also works when the process was launched with SW_HIDE.
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
        );
    }
}

fn rect(value: RECT) -> Rect {
    Rect {
        left: value.left,
        top: value.top,
        right: value.right,
        bottom: value.bottom,
    }
}

unsafe fn taskbar_identity(hwnd: HWND) -> Option<TaskbarIdentity> {
    // SAFETY: query only; process/thread identity distinguishes recycled HWNDs.
    unsafe {
        let mut process = 0;
        let thread = GetWindowThreadProcessId(hwnd, &mut process);
        (thread != 0 && process != 0).then_some(TaskbarIdentity {
            window: hwnd as usize,
            process,
            thread,
        })
    }
}

unsafe fn taskbar_surface_ready(taskbar: HWND) -> bool {
    // SAFETY: all queries borrow shell handles. Window visibility alone does
    // not establish that the taskbar's tray layout or DWM presentation is ready.
    unsafe {
        if taskbar.is_null() || IsWindowVisible(taskbar) == 0 {
            return false;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(
            taskbar,
            DWMWA_CLOAKED as u32,
            std::ptr::from_mut(&mut cloaked).cast(),
            size_of_val(&cloaked) as u32,
        ) < 0
            || cloaked != 0
        {
            return false;
        }
        let tray = FindWindowExW(taskbar, null_mut(), w!("TrayNotifyWnd"), null());
        let mut bounds = RECT::default();
        !tray.is_null()
            && IsWindowVisible(tray) != 0
            && GetClientRect(tray, &mut bounds) != 0
            && bounds.right > bounds.left
            && bounds.bottom > bounds.top
    }
}

unsafe fn taskbar_can_present(context: &Context, taskbar: HWND) -> bool {
    // SAFETY: query only. A re-entrant refresh for the retiring window must not
    // reset readiness that was already recorded for its replacement.
    unsafe {
        let presentation = context.presentation.get();
        presentation.matches_taskbar(taskbar_identity(taskbar))
            && presentation.can_present(taskbar_surface_ready(taskbar))
    }
}

unsafe fn restore_child_z_order(hwnd: HWND, taskbar: HWND) -> bool {
    // SAFETY: only reorder our child among taskbar siblings. Explorer creates
    // additional surfaces after Shell_TrayWnd first appears; WS_VISIBLE does
    // not mean that those later siblings aren't covering our layered window.
    // Never change the taskbar's own desktop band or activate another window.
    unsafe {
        GetParent(hwnd) == taskbar
            && GetTopWindow(taskbar) != hwnd
            && SetWindowPos(
                hwnd,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            ) != 0
    }
}

unsafe fn refresh(hwnd: HWND, context: &Context) {
    // SAFETY: called on the owning thread. Window-management calls never retain Rust pointers.
    unsafe {
        let Ok(settings) = context.state.settings.read().map(|s| s.clone()) else {
            return;
        };
        let taskbar = context.taskbar.get();
        if !owns_widget(hwnd, (context as *const Context).cast()) {
            return;
        }
        if taskbar.is_null() || GetParent(hwnd) != taskbar {
            hide_widget(hwnd, context);
            return;
        }
        let Ok(snapshot) = context.state.snapshot.read().map(|s| s.clone()) else {
            return;
        };
        let mut startup = context.startup.get();
        let hardware_ready = startup.can_present(&snapshot.hardware, context.started_at.elapsed());
        context.startup.set(startup);
        if !hardware_ready {
            return;
        }
        let can_present = taskbar_can_present(context, taskbar);
        if !can_present
            && (GetWindowLongPtrW(hwnd, GWL_STYLE) & WS_VISIBLE as isize != 0
                || context.drag.get().is_some())
        {
            hide_widget(hwnd, context);
        }
        // Lay out in parent-client coordinates. Native child composition supplies
        // every animation frame, including clipping and taskbar visibility.
        let mut bounds = RECT::default();
        if GetClientRect(taskbar, &mut bounds) == 0 {
            hide_widget(hwnd, context);
            return;
        }
        let dpi = GetDpiForWindow(taskbar).max(96);
        let dpi_changed = dpi != context.dpi.get();
        if dpi_changed {
            let Some(icons) = Icons::new(dpi) else {
                return;
            };
            context.icons.replace(Some(icons));
            let Ok(text) = Renderer::new(dpi) else {
                return;
            };
            context.text.replace(text);
            context.columns.set(None);
            context.dpi.set(dpi);
        }
        let columns = match context.columns.get() {
            Some(columns) => columns,
            None => {
                let Ok(columns) = measured_columns(&mut context.text.borrow_mut(), dpi) else {
                    return;
                };
                context.columns.set(Some(columns));
                columns
            }
        };
        let tray = FindWindowExW(taskbar, null_mut(), w!("TrayNotifyWnd"), null());
        let mut tray_bounds = RECT::default();
        let tray_rect = if !tray.is_null() && GetWindowRect(tray, &mut tray_bounds) != 0 {
            MapWindowPoints(
                null_mut(),
                taskbar,
                (&mut tray_bounds as *mut RECT).cast(),
                2,
            );
            Some(rect(tray_bounds))
        } else {
            None
        };
        let lines = [snapshot.upload_bps, snapshot.download_bps].map(|speed| {
            if snapshot.paused {
                SpeedDisplay {
                    value: "Paused".into(),
                    unit: "",
                }
            } else if snapshot.adapter_missing {
                SpeedDisplay {
                    value: "—".into(),
                    unit: "",
                }
            } else {
                // Use the regular numeric/unit format from the first frame,
                // without displaying stale rates while a new baseline is pending.
                let speed = if snapshot.ready { speed } else { 0.0 };
                format_speed(speed)
            }
        });
        let mut visible = gauge::visible(snapshot.hardware.gpu.presence);
        // Keep both the width and slot mapping stable until a drag finishes.
        if context.drag.get().is_some()
            && let Some(content) = context.content.borrow().as_ref()
        {
            visible = content.visible;
        }
        let content = Content {
            lines,
            hardware: GaugeReading::from_snapshot(&snapshot.hardware, snapshot.paused),
            visible,
        };
        let content_changed = context.content.borrow().as_ref() != Some(&content);
        // Keep the hit rectangle stable during a gesture even if network values change.
        let width = if context.drag.get().is_some() {
            context
                .layout
                .get()
                .map(|layout| layout.position(0, None).width())
        } else {
            None
        }
        .unwrap_or_else(|| columns.total_width(visible.len()));
        context.content.replace(Some(content));
        let Some(layout) =
            TaskbarLayout::with_width(rect(bounds), tray_rect, f64::from(dpi) / 96.0, width)
        else {
            hide_widget(hwnd, context);
            return;
        };
        context.layout.set(Some(layout));
        let dragged = context.drag.get().and_then(|drag| drag.position);
        let position = layout.position(settings.offset_x, dragged.or(settings.taskbar_position));
        let mut current = RECT::default();
        let had_bounds = GetWindowRect(hwnd, &mut current) != 0;
        if had_bounds {
            MapWindowPoints(null_mut(), taskbar, (&mut current as *mut RECT).cast(), 2);
        }
        let size_changed = !had_bounds
            || current.right - current.left != position.width()
            || current.bottom - current.top != position.height();
        if !had_bounds
            || current.left != position.left
            || current.top != position.top
            || size_changed
        {
            // Moving a window can generate another WM_MOUSEMOVE. Avoid moving to
            // an unchanged rectangle, including when dragging against a boundary.
            SetWindowPos(
                hwnd,
                null_mut(),
                position.left,
                position.top,
                position.width(),
                position.height(),
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOZORDER,
            );
        }
        // IsWindowVisible also includes parent visibility. Never toggle our own
        // visibility just because Explorer is hidden mid-animation.
        let hidden = GetWindowLongPtrW(hwnd, GWL_STYLE) & WS_VISIBLE as isize == 0;
        let restacked = can_present && restore_child_z_order(hwnd, taskbar);
        // Supply the layered surface before showing it, including the first frame
        // after hardware warm-up. A failed presentation stays dirty and is retried.
        if content_changed
            || dpi_changed
            || size_changed
            || context.dirty.get()
            || hidden
            || restacked
        {
            context.dirty.set(true);
            render(hwnd, context);
        }
        // Recheck after rendering: shell calls may have re-entered us while the
        // taskbar was being retired. A prepared surface alone never permits show.
        if hidden
            && !context.dirty.get()
            && owns_widget(hwnd, (context as *const Context).cast())
            && GetParent(hwnd) == taskbar
            && taskbar_can_present(context, taskbar)
        {
            reveal_widget(hwnd);
        }
    }
}

unsafe fn taskbar_dark_mode() -> bool {
    // SAFETY: read one local preference DWORD into a correctly sized buffer.
    unsafe {
        let mut light = 0u32;
        let mut size = size_of_val(&light) as u32;
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            null_mut(),
            std::ptr::from_mut(&mut light).cast(),
            &mut size,
        );
        light == 0
    }
}

fn measured_columns(text: &mut Renderer, dpi: u32) -> windows::core::Result<Columns> {
    let scale = dpi.max(96) as f32 / 96.0;
    let mut digit = 0.0_f32;
    for character in '0'..='9' {
        digit = digit.max(text.layout(&character.to_string(), Style::Network)?.advance);
    }
    let fraction = text.layout(".", Style::Network)?.advance + digit * SPEED_DECIMALS as f32;
    let mut number = digit * 3.0 + fraction;
    for label in ["Paused", "999+", "—"] {
        number = number.max(text.layout(label, Style::Network)?.width());
    }
    let mut unit = 0.0_f32;
    for label in SPEED_UNITS {
        unit = unit.max(text.layout(label, Style::Network)?.width());
    }
    Ok(Columns {
        number: number.ceil() as i32 + scale.ceil() as i32,
        unit: unit.ceil() as i32 + scale.ceil() as i32,
        hardware: (gauge::COLUMN_WIDTH * f64::from(scale)).round() as i32,
        padding: (4.0 * scale).round() as i32,
        icon: icon_size(dpi) as i32,
    })
}

fn frame(
    context: &Context,
    columns: Columns,
    width: i32,
    height: i32,
    pixels: &mut [u32],
) -> windows::core::Result<()> {
    let mut text = context.text.borrow_mut();
    if context.text_settings_changed.get() {
        text.refresh_rendering_params()?;
        context.text_settings_changed.set(false);
    }
    let content = context.content.borrow();
    let content = content.as_ref().unwrap();
    let palette = context.palette.get();
    let mut runs = Vec::new();
    let padding = columns.padding as f32;
    let text_left = columns.number_left() as f32;
    let half = height / 2;
    for (row, reading) in content.lines.iter().enumerate() {
        let top = row as f32 * half as f32;
        let bottom = top + half as f32;
        let unit_right = (columns.width().min(width) - columns.padding) as f32;
        let number_right = if reading.unit.is_empty() {
            text_left + columns.number as f32
        } else {
            let unit = text.layout(reading.unit, Style::Network)?;
            let unit_left = unit_right - unit.width();
            runs.push(unit.at(unit_left, (top + bottom) * 0.5, palette.network));
            unit_left - padding
        };
        let layout = text.layout(&reading.value, Style::Network)?;
        runs.push(layout.centered([text_left, top, number_right, bottom], palette.network));
    }
    let gauge_size = (gauge::SIZE * context.dpi.get() as f32 / 96.0).round() as i32;
    let gauge_size = gauge_size.min(height).max(1);
    let gauge_top = (height - gauge_size) / 2;
    let gauge_scale = gauge_size as f32 / gauge::SIZE;
    for (slot, &column) in content.visible.iter().enumerate() {
        let left = columns.hardware_left(slot);
        let reading = &content.hardware[column];
        for (row, value) in [reading.value.as_str(), reading.detail.as_str()]
            .iter()
            .enumerate()
        {
            let [x, top, right, bottom] = gauge::TEXT_BOUNDS[row];
            runs.extend(text.gauge(
                value,
                match (row, column) {
                    (0, _) => GaugeField::Usage,
                    (_, 0) => GaugeField::Temperature,
                    _ => GaugeField::Memory,
                },
                [
                    left as f32 + x * gauge_scale,
                    gauge_top as f32 + top * gauge_scale,
                    (left as f32 + right * gauge_scale).min(width as f32),
                    gauge_top as f32 + bottom * gauge_scale,
                ],
                if row == 0 {
                    palette.text
                } else {
                    palette.secondary
                },
            )?);
        }
        runs.extend(text.labels(
            column,
            [left as f32, gauge_top as f32],
            gauge_scale,
            palette.label(column, reading.value == "Ⅱ"),
        )?);
    }
    text.render((width as u32, height as u32), &runs, pixels)?;
    if let Some(icons) = context.icons.borrow().as_ref() {
        icons.paint(
            pixels,
            width as usize,
            height as usize,
            columns.padding as usize,
            palette.network,
        );
    }
    for (slot, &column) in content.visible.iter().enumerate() {
        gauge::paint(
            pixels,
            (width as usize, height as usize),
            (
                columns.hardware_left(slot).max(0) as usize,
                gauge_top as usize,
            ),
            gauge_size as u32,
            &content.hardware[column],
            column,
            palette,
        );
    }
    make_hit_testable(pixels);
    Ok(())
}

unsafe fn render(hwnd: HWND, context: &Context) {
    // SAFETY: the cached top-down DIB and DC belong to this window thread.
    // DirectWrite copies into its pixels; UpdateLayeredWindow consumes them synchronously.
    unsafe {
        if context.content.borrow().is_none() {
            return;
        }
        let Some(columns) = context.columns.get() else {
            return;
        };
        let mut bounds = RECT::default();
        if GetWindowRect(hwnd, &mut bounds) == 0 {
            return;
        }
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        if width <= 0 || height <= 0 {
            return;
        }
        context.dirty.set(true);
        // Native presentation can re-enter the window procedure. Leave the frame
        // dirty for a later refresh instead of borrowing the active surface twice.
        let Ok(mut cached) = context.bitmap.try_borrow_mut() else {
            return;
        };
        if cached
            .as_ref()
            .is_none_or(|bitmap| bitmap.size() != (width, height))
        {
            // Drop the previous allocation before creating a surface at a new DPI/size.
            *cached = None;
            match Bitmap::new(width, height) {
                Ok(bitmap) => *cached = Some(bitmap),
                Err(error) => {
                    eprintln!("Cannot create taskbar surface: {error}");
                    return;
                }
            }
        }
        let bitmap = cached.as_mut().unwrap();
        if let Err(error) = frame(context, columns, width, height, bitmap.pixels_mut()) {
            eprintln!("Cannot render taskbar text: {error}");
            return;
        }
        let source = POINT::default();
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let presented = UpdateLayeredWindow(
            hwnd,
            null_mut(),
            // Position is managed in parent coordinates by SetWindowPos. Leaving
            // pptDst null avoids racing Explorer's animation with stale screen bounds.
            null(),
            &size,
            bitmap.dc(),
            &source,
            0,
            &blend,
            ULW_ALPHA,
        );
        // Retry on the next refresh if Explorer/display recreation interrupted presentation.
        context.dirty.set(presented == 0);
        if presented == 0 {
            // A display/Explorer reset can invalidate a cached native DC. Rebuild
            // it on retry instead of repeatedly submitting the same failed surface.
            *cached = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_gpu_removes_one_column_and_keeps_ram_adjacent_at_each_dpi() {
        use tessera_core::hardware::gpu::Presence;
        for scale in [1, 2, 3] {
            let columns = Columns {
                number: 32 * scale,
                unit: 28 * scale,
                hardware: 48 * scale,
                padding: 4 * scale,
                icon: 16 * scale,
            };
            let visible = gauge::visible(Presence::Absent);
            let ram_slot = visible.iter().position(|&metric| metric == 2).unwrap();
            assert_eq!(ram_slot, 1);
            assert_eq!(
                columns.hardware_left(ram_slot) - columns.hardware_left(0) - columns.hardware,
                columns.padding * 2
            );
            let width = columns.total_width(visible.len());
            assert_eq!(width, columns.hardware_left(ram_slot) + columns.hardware);
            assert_eq!(
                columns.total_width(3) - width,
                columns.hardware + columns.padding * 2
            );
        }
    }

    #[test]
    fn fixed_columns_fit_directwrite_numbers_status_and_units_at_each_dpi() {
        for dpi in [96, 120, 144, 192, 288] {
            let mut text = Renderer::new(dpi).unwrap();
            let columns = measured_columns(&mut text, dpi).unwrap();
            for number in 0..1_000 {
                for exponent in [0, 3, 6, 18] {
                    let display = format_speed((f64::from(number) + 0.987) * 10_f64.powi(exponent));
                    assert!(
                        text.layout(&display.value, Style::Network)
                            .unwrap()
                            .width()
                            .ceil()
                            <= columns.number as f32
                    );
                    assert!(
                        text.layout(display.unit, Style::Network)
                            .unwrap()
                            .width()
                            .ceil()
                            <= columns.unit as f32
                    );
                }
            }
            for label in ["Paused", "—", "999+"] {
                assert!(
                    text.layout(label, Style::Network).unwrap().width().ceil()
                        <= columns.number as f32
                );
            }
            assert!(columns.hardware_left(0) > columns.width());
            assert!(columns.hardware_left(1) > columns.hardware_left(0) + columns.hardware);
            assert!(columns.hardware_left(2) > columns.hardware_left(1) + columns.hardware);
            assert_eq!(
                columns.hardware_left(2) + columns.hardware,
                columns.total_width(3)
            );
            assert_eq!(
                columns.hardware_left(1) + columns.hardware,
                columns.total_width(2)
            );
            assert_eq!(
                columns.total_width(3) - columns.total_width(2),
                columns.hardware + columns.padding * 2
            );
        }
    }

    struct TestWindow(HWND);

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: this hidden test window is owned by the current test thread.
            unsafe { DestroyWindow(self.0) };
        }
    }

    unsafe fn test_parent() -> TestWindow {
        // SAFETY: all test windows use the built-in STATIC class on this thread.
        unsafe {
            let parent = TestWindow(CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("Tessera taskbar stand-in"),
                WS_POPUP,
                100,
                100,
                600,
                48,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            ));
            assert!(!parent.0.is_null());
            parent
        }
    }

    unsafe fn test_child(parent: HWND) -> TestWindow {
        // SAFETY: the parent is an isolated test window on this thread.
        unsafe {
            let previous = GetThreadDpiAwarenessContext();
            let child = TestWindow(create_taskbar_child(
                parent,
                w!("STATIC"),
                GetModuleHandleW(null()),
                null(),
            ));
            assert!(!child.0.is_null());
            assert_eq!(GetParent(child.0), parent);
            assert_ne!(
                AreDpiAwarenessContextsEqual(previous, GetThreadDpiAwarenessContext()),
                0
            );
            assert_ne!(
                AreDpiAwarenessContextsEqual(
                    GetWindowDpiAwarenessContext(parent),
                    GetWindowDpiAwarenessContext(child.0)
                ),
                0
            );
            child
        }
    }

    unsafe fn bounds(hwnd: HWND) -> (i32, i32, i32, i32) {
        // SAFETY: only reads the rectangle of our test window.
        unsafe {
            let mut rect = RECT::default();
            assert_ne!(GetWindowRect(hwnd, &mut rect), 0);
            (rect.left, rect.top, rect.right, rect.bottom)
        }
    }

    #[test]
    fn first_show_and_restore_preserve_focus_and_geometry() {
        // SAFETY: use an isolated native window, never the user's Explorer window.
        unsafe {
            let parent = test_parent();
            reveal_widget(parent.0);
            let window = test_child(parent.0);
            assert_eq!(IsWindowVisible(window.0), 0);
            let foreground = GetForegroundWindow();
            let original = bounds(window.0);

            for _ in 0..2 {
                reveal_widget(window.0);
                assert_ne!(IsWindowVisible(window.0), 0);
                assert_eq!(
                    GetWindowLongPtrW(window.0, GWL_EXSTYLE) & WS_EX_TOPMOST as isize,
                    0
                );
                assert_eq!(GetForegroundWindow(), foreground);
                assert_eq!(bounds(window.0), original);
                SetWindowPos(
                    window.0,
                    null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_HIDEWINDOW,
                );
                assert_eq!(IsWindowVisible(window.0), 0);
            }
        }
    }

    #[test]
    fn child_follows_parent_animation_and_visibility_without_refresh() {
        // SAFETY: simulate taskbar frames using our own windows only.
        unsafe {
            let parent = test_parent();
            let child = test_child(parent.0);
            reveal_widget(child.0);
            // Prepare the child before Explorer shows its taskbar.
            assert_eq!(IsWindowVisible(child.0), 0);
            reveal_widget(parent.0);
            assert_ne!(IsWindowVisible(child.0), 0);
            let original = bounds(child.0);
            let origin = bounds(parent.0);
            for (x, y) in [(100, 110), (100, 130), (-400, 140), (-450, -200)] {
                SetWindowPos(
                    parent.0,
                    null_mut(),
                    x,
                    y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let actual = bounds(child.0);
                assert_eq!(actual.0 - original.0, x - origin.0);
                assert_eq!(actual.1 - original.1, y - origin.1);
                assert_eq!(actual.2 - actual.0, original.2 - original.0);
                assert_eq!(actual.3 - actual.1, original.3 - original.1);
            }
            SetWindowPos(
                parent.0,
                null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW,
            );
            assert_eq!(IsWindowVisible(child.0), 0);
            assert_ne!(
                GetWindowLongPtrW(child.0, GWL_STYLE) & WS_VISIBLE as isize,
                0
            );
            reveal_widget(parent.0);
            assert_ne!(IsWindowVisible(child.0), 0);
        }
    }

    #[test]
    fn destroyed_parent_removes_child_and_replacement_accepts_new_child() {
        // SAFETY: destroy/recreate our stand-in, never the actual taskbar.
        unsafe {
            let parent = test_parent();
            let child = test_child(parent.0);
            reveal_widget(parent.0);
            reveal_widget(child.0);
            DestroyWindow(parent.0);
            assert_eq!(IsWindow(child.0), 0);
            drop(child);
            drop(parent);

            let replacement = test_parent();
            let child = test_child(replacement.0);
            reveal_widget(child.0);
            assert_eq!(IsWindowVisible(child.0), 0);
            reveal_widget(replacement.0);
            assert_ne!(IsWindowVisible(child.0), 0);
        }
    }

    #[test]
    fn later_taskbar_surface_cannot_leave_visible_widget_covered() {
        // SAFETY: simulate Explorer adding its content after our child was shown.
        unsafe {
            let parent = test_parent();
            let child = test_child(parent.0);
            reveal_widget(parent.0);
            reveal_widget(child.0);
            let original = bounds(child.0);
            let foreground = GetForegroundWindow();
            let surface = test_child(parent.0);
            reveal_widget(surface.0);
            assert_eq!(GetTopWindow(parent.0), surface.0);
            // The old visibility-only check reported success in this state.
            assert_ne!(IsWindowVisible(child.0), 0);
            assert_ne!(
                GetWindowLongPtrW(child.0, GWL_STYLE) & WS_VISIBLE as isize,
                0
            );

            assert!(restore_child_z_order(child.0, parent.0));
            assert_eq!(GetTopWindow(parent.0), child.0);
            assert_eq!(bounds(child.0), original);
            assert_eq!(GetForegroundWindow(), foreground);
            assert_eq!(
                GetWindowLongPtrW(child.0, GWL_EXSTYLE) & WS_EX_TOPMOST as isize,
                0
            );
            // A normal refresh must not keep raising or invalidating the window.
            assert!(!restore_child_z_order(child.0, parent.0));

            // Repeat the shell reordering and ensure recovery works more than once.
            reveal_widget(surface.0);
            assert!(restore_child_z_order(child.0, parent.0));
            assert_eq!(GetTopWindow(parent.0), child.0);
        }
    }

    #[test]
    fn stale_handle_with_different_identity_is_never_destroyed() {
        // SAFETY: a stand-in with different user data models a recycled HWND.
        unsafe {
            let previous_context = Box::new(1u8);
            let replacement_context = Box::new(2u8);
            let old_data = std::ptr::from_ref(&*previous_context).cast::<c_void>();
            let new_data = std::ptr::from_ref(&*replacement_context).cast::<c_void>();
            let replacement = test_parent();
            SetWindowLongPtrW(replacement.0, GWLP_USERDATA, new_data as isize);
            reveal_widget(replacement.0);
            assert!(!owns_widget(replacement.0, old_data));
            assert!(owns_widget(replacement.0, new_data));
            destroy_owned_widget(replacement.0, old_data);
            assert_ne!(IsWindow(replacement.0), 0);
            assert_ne!(IsWindowVisible(replacement.0), 0);
            destroy_owned_widget(replacement.0, new_data);
            assert_eq!(IsWindow(replacement.0), 0);
        }
    }

    #[test]
    fn old_window_destruction_does_not_forget_replacement() {
        // SAFETY: both stand-ins are owned by this test thread.
        unsafe {
            let previous = test_parent();
            let replacement = test_parent();
            let current = Cell::new(replacement.0);
            DestroyWindow(previous.0);
            assert!(!forget_destroyed_widget(&current, previous.0));
            assert_eq!(current.get(), replacement.0);
            assert!(forget_destroyed_widget(&current, replacement.0));
            assert!(current.get().is_null());
        }
    }
}
