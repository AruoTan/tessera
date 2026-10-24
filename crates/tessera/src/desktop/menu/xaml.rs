//! Inbox Windows.UI.Xaml.Controls.MenuFlyout hosted through XAML Islands.
//! The worker owns every XAML object; only integer commands cross UI threads.
use super::{
    Icon, QUIT,
    appearance::{self, Appearance},
    commands, shadow,
};
use std::{
    cell::{Cell, RefCell},
    mem::size_of,
    ptr::{null, null_mut},
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use windows_sys::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::{
            ClientToScreen, CreateRectRgn, DeleteObject, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST,
            MONITORINFO, MonitorFromPoint, MonitorFromWindow, SetWindowRgn,
        },
        System::{LibraryLoader::GetModuleHandleW, Registry::*, Threading::GetCurrentThreadId},
        UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
    },
    core::w,
};
use windows_xaml::{
    Foundation::{EventHandler, EventRegistrationToken, Point, TypedEventHandler},
    UI::{
        ViewManagement::UISettings,
        Xaml::{
            Controls::Primitives::{
                FlyoutBase, FlyoutBaseClosingEventArgs, FlyoutPlacementMode, FlyoutShowMode, Popup,
            },
            Controls::{FontIcon, Grid, MenuFlyout, MenuFlyoutItem, MenuFlyoutSeparator, Symbol},
            ElementTheme, FrameworkElement,
            Hosting::{DesktopWindowXamlSource, WindowsXamlManager},
            Media::{FontFamily, VisualTreeHelper},
            RoutedEventHandler, UIElement,
        },
    },
    Win32::{
        Foundation::{BOOL as XamlBool, HWND as XamlHwnd},
        System::WinRT::{
            RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize,
            Xaml::IDesktopWindowXamlSourceNative2,
        },
        UI::WindowsAndMessaging::MSG as XamlMessage,
    },
    core::{Error, HRESULT, HSTRING, Interface, Result},
};

pub(in crate::desktop) const ACTION: u32 = WM_APP + 22;
pub(in crate::desktop) const FAILED: u32 = WM_APP + 23;
const WAKE: u32 = WM_APP + 40;
const LOADED: u32 = WM_APP + 41;
const CHOSEN: u32 = WM_APP + 42;
const CLOSED: u32 = WM_APP + 43;
const CARD_READY: u32 = WM_APP + 44;
const ENTRY_COMPLETED: u32 = WM_APP + 45;
const BEGIN_CLOSE: u32 = WM_APP + 46;
const EXIT_COMPLETED: u32 = WM_APP + 47;
const CHECK_ACTIVATION: u32 = WM_APP + 48;
const WATCHDOG: usize = 1;
const WATCHDOG_INTERVAL_MS: u32 = 100;
const CLOSE_TIMEOUT: Duration = Duration::from_millis(350);
const HOST_CLASS: windows_sys::core::PCWSTR = w!("TesseraXamlMenuHost");

struct Request {
    x: i32,
    y: i32,
    locked: bool,
    paused: bool,
    foreground: usize,
}

#[derive(Default)]
pub(in crate::desktop) struct Controller {
    worker: RefCell<Option<Worker>>,
}

impl Controller {
    pub fn open(
        &self,
        owner: HWND,
        x: i32,
        y: i32,
        locked: bool,
        paused: bool,
    ) -> std::result::Result<(), String> {
        // Snapshot before the potentially slow first-use XAML initialization.
        // SAFETY: only an opaque HWND value crosses the worker queue.
        let foreground = unsafe { GetForegroundWindow() } as usize;
        let mut worker = self.worker.borrow_mut();
        if worker
            .as_ref()
            .is_some_and(|worker| worker.thread.is_finished())
        {
            *worker = None;
        }
        if worker.is_none() {
            *worker = Some(Worker::spawn(owner as usize).map_err(|error| error.to_string())?);
        }
        let worker = worker.as_ref().unwrap();
        worker
            .requests
            .send(Request {
                x,
                y,
                locked,
                paused,
                foreground,
            })
            .map_err(|error| error.to_string())?;
        let id = worker.id.load(Ordering::Acquire);
        if id != 0 {
            // SAFETY: the worker creates its queue before publishing this ID.
            unsafe {
                PostThreadMessageW(id, WAKE, 0, 0);
            }
        }
        Ok(())
    }

    pub fn close(&self) {
        self.worker.borrow_mut().take();
    }
}

struct Worker {
    requests: Sender<Request>,
    id: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Worker {
    fn spawn(owner: usize) -> std::io::Result<Self> {
        let (requests, receiver) = mpsc::channel();
        let id = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_id = id.clone();
        let worker_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("tessera-menu".into())
            .spawn(move || {
                if let Err(error) = run(owner, receiver, &worker_id, &worker_stop)
                    && !worker_stop.load(Ordering::Acquire)
                {
                    report(owner, &error);
                }
            })?;
        Ok(Self {
            requests,
            id,
            stop,
            thread,
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let id = self.id.load(Ordering::Acquire);
        if id != 0 && !self.thread.is_finished() {
            // SAFETY: no handles or XAML objects cross threads; wake the owner.
            unsafe {
                PostThreadMessageW(id, WM_QUIT, 0, 0);
            }
        }
    }
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances this worker's successful RoInitialize.
        unsafe {
            RoUninitialize();
        }
    }
}

struct Manager(WindowsXamlManager);
impl Drop for Manager {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}

fn run(owner: usize, requests: Receiver<Request>, id: &AtomicU32, stop: &AtomicBool) -> Result<()> {
    // SAFETY: all windows, XAML interfaces and the STA belong to this worker.
    unsafe {
        let mut message = MSG::default();
        PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
        id.store(GetCurrentThreadId(), Ordering::Release);
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        RoInitialize(RO_INIT_SINGLETHREADED)?;
        let _apartment = Apartment;
        let _manager = Manager(WindowsXamlManager::InitializeForCurrentThread()?);
        let session = Box::new(Session::new(owner)?);
        SetWindowLongPtrW(
            session.hwnd,
            GWLP_USERDATA,
            (&*session as *const Session) as isize,
        );
        while !stop.load(Ordering::Acquire) {
            // A request sent before queue publication is picked up here as well.
            while let Ok(request) = requests.try_recv() {
                if !session.is_open()
                    && let Err(error) = session.open(request)
                {
                    session.dismiss();
                    report(owner, &error);
                }
            }
            if stop.load(Ordering::Acquire) {
                break;
            }
            let result = GetMessageW(&mut message, null_mut(), 0, 0);
            if result == -1 {
                return Err(Error::from_win32());
            }
            if result == 0 {
                break;
            }
            if message.message != WAKE && !session.translate(&message) {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    Ok(())
}

fn report(owner: usize, error: &Error) {
    // SAFETY: posting a numeric HRESULT never dereferences the receiver address.
    unsafe {
        PostMessageW(owner as HWND, FAILED, error.code().0 as u32 as usize, 0);
    }
}

struct Session {
    owner: usize,
    hwnd: HWND,
    source: Option<SourceGuard>,
    native: IDesktopWindowXamlSourceNative2,
    anchor: FrameworkElement,
    loaded: EventRegistrationToken,
    active: RefCell<Option<Rc<Flyout>>>,
    sequence: Cell<u32>,
}

struct Flyout {
    menu: MenuFlyout,
    base: FlyoutBase,
    closed: EventRegistrationToken,
    closing: Option<EventRegistrationToken>,
    allow_close: Arc<AtomicBool>,
    closing_started: Cell<bool>,
    close_deadline: Arc<OnceLock<Instant>>,
    entry_complete: Cell<bool>,
    card: RefCell<Option<Rc<appearance::Card>>>,
    animation: RefCell<Option<appearance::Animation>>,
    serial: u32,
    shown: Cell<bool>,
    command: Cell<u32>,
    custom_shadow: bool,
    shadow_started: Cell<bool>,
    target_edge: Cell<i32>,
    work_top: Cell<i32>,
}

impl Flyout {
    fn new(hwnd: HWND, request: &Request, serial: u32) -> Result<Self> {
        let appearance = Appearance::load(theme())?;
        let menu = MenuFlyout::new()?;
        if let Some(appearance) = &appearance {
            menu.SetMenuFlyoutPresenterStyle(&appearance.presenter)?;
        }
        let base: FlyoutBase = menu.cast()?;
        base.SetPlacement(FlyoutPlacementMode::TopEdgeAlignedLeft)?;
        base.SetShowMode(FlyoutShowMode::Standard)?;
        // The anchor is intentionally tiny; use the framework's top-level popup.
        base.SetShouldConstrainToRootBounds(false)?;
        // Inbox FlyoutBase has no public entry-animation completion event.
        // The custom card therefore owns its entry/exit animations; high
        // contrast retains the complete platform template and transitions.
        base.SetAreOpenCloseAnimationsEnabled(
            appearance.is_none() && UISettings::new()?.AnimationsEnabled()?,
        )?;
        let items = menu.Items()?;
        for (index, command) in commands(request.locked, request.paused)
            .into_iter()
            .enumerate()
        {
            if command.id == QUIT {
                let separator = MenuFlyoutSeparator::new()?;
                if let Some(appearance) = &appearance {
                    separator
                        .cast::<FrameworkElement>()?
                        .SetStyle(&appearance.separator)?;
                }
                items.Append(&separator)?;
            }
            let item = MenuFlyoutItem::new()?;
            item.SetText(HSTRING::from(command.label))?;
            if let Some(appearance) = &appearance {
                item.cast::<FrameworkElement>()?
                    .SetStyle(&appearance.item)?;
            }
            let symbol = match command.icon {
                Icon::Pin => Symbol::Pin,
                Icon::Unpin => Symbol::UnPin,
                Icon::Pause => Symbol::Pause,
                Icon::Play => Symbol::Play,
                Icon::Exit => Symbol::Cancel,
            };
            let icon = FontIcon::new()?;
            icon.SetFontFamily(FontFamily::CreateInstanceWithName(HSTRING::from(
                "Segoe Fluent Icons, Segoe MDL2 Assets",
            ))?)?;
            icon.SetFontSize(16.0)?;
            // Symbol values are the documented MDL2/Fluent icon code points.
            let glyph = char::from_u32(symbol.0 as u32).unwrap_or(' ');
            icon.SetGlyph(HSTRING::from(glyph.to_string()))?;
            item.SetIcon(&icon)?;
            let address = hwnd as usize;
            if index == 0 && appearance.is_some() {
                item.cast::<FrameworkElement>()?
                    .Loaded(RoutedEventHandler::new(move |_, _| {
                        // Loaded only starts card entry. The entry storyboard completion
                        // is the sole animated path that may reveal the shadow.
                        unsafe {
                            PostMessageW(address as HWND, CARD_READY, serial as usize, 0);
                        }
                        Ok(())
                    }))?;
            }
            item.Click(RoutedEventHandler::new(move |_, _| {
                // SAFETY: only integer command/generation values cross the queue.
                unsafe {
                    PostMessageW(
                        address as HWND,
                        CHOSEN,
                        command.id as usize,
                        serial as isize,
                    );
                }
                Ok(())
            }))?;
            items.Append(&item)?;
        }
        let address = hwnd as usize;
        let allow_close = Arc::new(AtomicBool::new(false));
        let close_deadline = Arc::new(OnceLock::new());
        let closing = if appearance.is_some() {
            let allow = allow_close.clone();
            let deadline = close_deadline.clone();
            Some(base.Closing(
                TypedEventHandler::<FlyoutBase, FlyoutBaseClosingEventArgs>::new(move |_, args| {
                    if !allow.load(Ordering::Acquire)
                        && UISettings::new()?.AnimationsEnabled()?
                        && let Some(args) = args
                    {
                        // The native watchdog remains independent of XAML clocks.
                        // Repeated light-dismiss attempts must not extend the deadline.
                        if deadline.get().is_none() {
                            let _ = deadline.set(Instant::now() + CLOSE_TIMEOUT);
                        }
                        // Only cancel when the deferred close was actually queued.
                        // SAFETY: this HWND and its timer belong to this STA.
                        if unsafe { PostMessageW(address as HWND, BEGIN_CLOSE, serial as usize, 0) }
                            != 0
                        {
                            args.SetCancel(true)?;
                        }
                    }
                    Ok(())
                }),
            )?)
        } else {
            None
        };
        let closed = base.Closed(EventHandler::new(move |_, _| {
            // SAFETY: callbacks post to their owner; never pass object pointers.
            unsafe {
                PostMessageW(address as HWND, CLOSED, serial as usize, 0);
            }
            Ok(())
        }))?;
        Ok(Self {
            menu,
            base,
            closed,
            closing,
            allow_close,
            closing_started: Cell::new(false),
            close_deadline,
            entry_complete: Cell::new(false),
            card: RefCell::new(None),
            animation: RefCell::new(None),
            serial,
            shown: Cell::new(false),
            command: Cell::new(0),
            custom_shadow: appearance.is_some(),
            shadow_started: Cell::new(false),
            target_edge: Cell::new(request.y),
            work_top: Cell::new(i32::MIN),
        })
    }
}

impl Drop for Flyout {
    fn drop(&mut self) {
        self.animation.get_mut().take();
        if let Some(token) = self.closing {
            let _ = self.base.RemoveClosing(token);
        }
        let _ = self.base.RemoveClosed(self.closed);
        let _ = self.base.Hide();
        if let Ok(items) = self.menu.Items() {
            let _ = items.Clear();
        }
    }
}

// Close the source before the HWND, even if construction fails halfway through.
struct SourceGuard(DesktopWindowXamlSource);
impl Drop for SourceGuard {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}
struct WindowGuard(HWND);
impl Drop for WindowGuard {
    fn drop(&mut self) {
        // SAFETY: this guard owns the menu worker's top-level window.
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

impl Session {
    fn new(owner: usize) -> Result<Self> {
        // SAFETY: native handles stay on this thread; guards cover partial failures.
        unsafe {
            let module = GetModuleHandleW(null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: module,
                lpszClassName: HOST_CLASS,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(Error::from_win32());
            }
            let window = WindowGuard(CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                HOST_CLASS,
                w!("Tessera menu"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                owner as HWND,
                null_mut(),
                module,
                null(),
            ));
            if window.0.is_null() {
                return Err(Error::from_win32());
            }
            // XAML needs a loaded root HWND. Clip only the empty anchor window;
            // MenuFlyout renders in its own system-managed popup outside this root.
            let region = CreateRectRgn(0, 0, 0, 0);
            if region.is_null() {
                return Err(Error::from_win32());
            }
            if SetWindowRgn(window.0, region, 0) == 0 {
                let error = Error::from_win32();
                DeleteObject(region);
                return Err(error);
            }
            let source = SourceGuard(DesktopWindowXamlSource::new()?);
            let native: IDesktopWindowXamlSourceNative2 = source.0.cast()?;
            native.AttachToWindow(XamlHwnd(window.0 as isize))?;
            let child = native.WindowHandle()?.0 as HWND;
            SetWindowPos(
                child,
                null_mut(),
                0,
                0,
                1,
                1,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let grid = Grid::new()?;
            let anchor: FrameworkElement = grid.cast()?;
            anchor.SetWidth(1.0)?;
            anchor.SetHeight(1.0)?;
            let hwnd = window.0 as usize;
            let loaded = anchor.Loaded(RoutedEventHandler::new(move |_, _| {
                PostMessageW(hwnd as HWND, LOADED, 0, 0);
                Ok(())
            }))?;
            source.0.SetContent(&grid)?;
            let result = Self {
                owner,
                hwnd: window.0,
                source: Some(source),
                native,
                anchor,
                loaded,
                active: RefCell::new(None),
                sequence: Cell::new(0),
            };
            // The HWND transfers only after all fallible setup has succeeded.
            std::mem::forget(window);
            Ok(result)
        }
    }

    fn is_open(&self) -> bool {
        self.active.borrow().is_some()
    }

    fn open(&self, request: Request) -> Result<()> {
        // A foreground switch during worker startup makes this request obsolete.
        if unsafe { GetForegroundWindow() } as usize != request.foreground {
            return Ok(());
        }
        self.anchor.SetRequestedTheme(theme())?;
        let serial = self.sequence.get().wrapping_add(1);
        self.sequence.set(serial);
        let flyout = Rc::new(Flyout::new(self.hwnd, &request, serial)?);
        if unsafe { GetForegroundWindow() } as usize != request.foreground {
            return Ok(());
        }
        // SAFETY: moving the empty root to physical coordinates lets XAML
        // handle monitor DPI, popup layout and work-area clipping.
        // Set the final anchor once. XAML animates the card; moving its
        // framework-owned HWND would be undone by XAML's next placement pass.
        unsafe {
            let point = POINT {
                x: request.x,
                y: request.y,
            };
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let have_monitor = GetMonitorInfoW(monitor, &mut mi) != 0;
            flyout.work_top.set(if have_monitor {
                mi.rcWork.top
            } else {
                i32::MIN
            });
            let mut taskbar_top = if have_monitor && mi.rcWork.bottom < mi.rcMonitor.bottom {
                mi.rcWork.bottom
            } else {
                request.y
            };
            let taskbar = FindWindowW(w!("Shell_TrayWnd"), null());
            if !taskbar.is_null() && MonitorFromWindow(taskbar, MONITOR_DEFAULTTONEAREST) == monitor
            {
                let mut tb_rect = RECT::default();
                if GetWindowRect(taskbar, &mut tb_rect) != 0
                    && tb_rect.top > 0
                    && request.y >= tb_rect.top - 8
                {
                    taskbar_top = tb_rect.top;
                }
            }
            let dpi = GetDpiForWindow(self.hwnd);
            let dpi = if dpi == 0 { 96 } else { dpi };
            // Initial hint only: after layout, position_card corrects the actual
            // Popup offset independently of FlyoutBase's target-gap adjustment.
            let gap = ((shadow::TASKBAR_GAP - 3) * dpi as i32 + 48) / 96;
            let target_edge = if request.y >= taskbar_top - 16 {
                taskbar_top
            } else {
                request.y
            };
            flyout.target_edge.set(target_edge);
            let target_anchor_y = target_edge - gap;

            // The shadow gutter belongs to the popup, not the visible card.
            // Compensate it once so the card keeps its original taskbar anchor.
            let gutter = if flyout.custom_shadow {
                (shadow::GUTTER * dpi as i32 + 48) / 96
            } else {
                0
            };
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                request.x - gutter,
                target_anchor_y + gutter,
                1,
                1,
                SWP_NOACTIVATE,
            );
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            let activated = SetForegroundWindow(self.hwnd) != 0;
            if !activated && !self.has_foreground() {
                // Do not leave a topmost menu open without working activation.
                ShowWindow(self.hwnd, SW_HIDE);
                return Ok(());
            }
        }

        *self.active.borrow_mut() = Some(flyout);
        // SAFETY: active only while the menu is open, with no callback pointer.
        if unsafe { SetTimer(self.hwnd, WATCHDOG, WATCHDOG_INTERVAL_MS, None) } == 0 {
            let error = Error::from_win32();
            self.dismiss();
            return Err(error);
        }

        let element: UIElement = self.anchor.cast()?;
        element.UpdateLayout()?;
        // On first use, Loaded resumes presentation after a XamlRoot exists.
        self.show_if_loaded()
    }

    fn show_if_loaded(&self) -> Result<()> {
        // Release the RefCell borrow before a potentially reentrant framework call.
        let active = self.active.borrow().clone();
        if let Some(active) = active
            && !active.shown.get()
            && self.anchor.IsLoaded()?
        {
            // Loaded can arrive after the user has already switched away.
            if !self.has_foreground() || active.closing_started.get() {
                self.dismiss();
                return Ok(());
            }
            let element: UIElement = self.anchor.cast()?;
            active.base.SetXamlRoot(element.XamlRoot()?)?;
            active.shown.set(true);
            active.base.ShowAt(&self.anchor)?;
        }
        Ok(())
    }

    fn position_card(&self, flyout: &Flyout, root: &UIElement) -> Result<()> {
        let popup = card_popup(root)?;
        let child = popup.Child()?;
        let size: FrameworkElement = root.cast()?;
        let local_bottom = root
            .TransformToVisual(&child)?
            .TransformPoint(Point {
                X: 0.0,
                Y: (size.ActualHeight()? - shadow::GUTTER as f64) as f32,
            })?
            .Y as f64;
        let scale = root.XamlRoot()?.RasterizationScale()?;
        let mut origin = POINT::default();
        // SAFETY: this is our own island's client origin, not a popup HWND.
        unsafe {
            let island = self.native.WindowHandle()?.0 as HWND;
            if ClientToScreen(island, &mut origin) == 0 {
                return Err(Error::from_win32());
            }
        }
        let bottom = flyout.target_edge.get() as f64 - (shadow::TASKBAR_GAP as f64 * scale).round();
        let top = bottom - local_bottom * scale;
        if top < flyout.work_top.get() as f64 {
            // Keep the framework's fallback when there is not enough room above.
            return Ok(());
        }
        // Set the documented XAML property so XAML owns native window updates.
        // Shadow padding may extend into the taskbar; its tail is transparent
        // there. It must not determine the visible card's distance from the bar.
        popup.SetVerticalOffset((bottom - origin.y as f64) / scale - local_bottom)?;
        root.UpdateLayout()
    }

    fn completed(&self, serial: u32, message: u32) -> appearance::Completed {
        let address = self.hwnd as usize;
        EventHandler::new(move |_, _| {
            // SAFETY: only generation and phase values cross the message queue.
            unsafe {
                PostMessageW(address as HWND, message, serial as usize, 0);
            }
            Ok(())
        })
    }

    fn enter_card(&self, serial: u32) -> Result<()> {
        let active = self.active.borrow().clone();
        let Some(active) = active.filter(|active| {
            active.serial == serial && active.custom_shadow && !active.closing_started.get()
        }) else {
            return Ok(());
        };
        if active.card.borrow().is_some() {
            return Ok(());
        }
        let item: MenuFlyoutItem = active.menu.Items()?.GetAt(0)?.cast()?;
        let element = appearance::card_root(&item)?;
        // Correct the measured popup while the card is still fully transparent,
        // before its entry clock starts; placement failures must not hide it.
        if let Err(error) = self.position_card(&active, &element) {
            eprintln!("Cannot align the menu card: {error}");
        }
        let card = match appearance::Card::new(element.clone()) {
            Ok(card) => Rc::new(card),
            Err(error) => {
                // Keep the menu usable even if animation setup fails.
                appearance::show_static(&element)?;
                active.entry_complete.set(true);
                active.shadow_started.set(true);
                return Err(error);
            }
        };
        *active.card.borrow_mut() = Some(card.clone());
        if !UISettings::new()?.AnimationsEnabled()? {
            active.entry_complete.set(true);
            card.show_static()?;
            active.shadow_started.set(true);
            return Ok(());
        }
        match card.enter(self.completed(serial, ENTRY_COMPLETED)) {
            Ok(animation) => *active.animation.borrow_mut() = Some(animation),
            Err(error) => {
                card.show_static()?;
                active.entry_complete.set(true);
                active.shadow_started.set(true);
                return Err(error);
            }
        }
        Ok(())
    }

    fn entry_completed(&self, serial: u32) -> Result<()> {
        let active = self.active.borrow().clone();
        let Some(active) = active.filter(|active| {
            active.serial == serial
                && active.custom_shadow
                && !active.closing_started.get()
                && !active.entry_complete.get()
        }) else {
            return Ok(());
        };
        let card = active.card.borrow().clone();
        let Some(card) = card else {
            return Ok(());
        };
        active.animation.borrow_mut().take();
        active.entry_complete.set(true);
        card.reveal_shadow(UISettings::new()?.AnimationsEnabled()?)
            .or_else(|_| card.show_static())?;
        active.shadow_started.set(true);
        Ok(())
    }

    fn begin_close(&self, serial: u32) -> Result<()> {
        let active = self.active.borrow().clone();
        let Some(active) = active.filter(|active| active.serial == serial) else {
            return Ok(());
        };
        if active.closing_started.replace(true) {
            return Ok(());
        }
        if active.close_deadline.get().is_none() {
            let _ = active.close_deadline.set(Instant::now() + CLOSE_TIMEOUT);
        }
        if !active.shown.get() {
            self.dismiss();
            return Ok(());
        }
        // Revoke entry completion before replacing its animation. A completion
        // message already queued is rejected by closing_started above.
        let card = active.card.borrow().clone();
        let frame = card.as_ref().map(|card| card.frame()).transpose()?;
        active.animation.borrow_mut().take();
        if UISettings::new()?.AnimationsEnabled()?
            && let Some(card) = card
        {
            match card.exit(frame.unwrap(), self.completed(serial, EXIT_COMPLETED)) {
                Ok(animation) => {
                    *active.animation.borrow_mut() = Some(animation);
                    return Ok(());
                }
                Err(error) => eprintln!("Cannot animate menu dismissal: {error}"),
            }
        }
        self.exit_completed(serial)
    }

    fn exit_completed(&self, serial: u32) -> Result<()> {
        let active = self.active.borrow().clone();
        if let Some(active) =
            active.filter(|active| active.serial == serial && active.closing_started.get())
        {
            active.animation.borrow_mut().take();
            active.allow_close.store(true, Ordering::Release);
            if let Err(error) = active.base.Hide() {
                // Drop removes the Closing handler before retrying Hide.
                self.dismiss();
                return Err(error);
            }
            if !active.base.IsOpen()? {
                // Do not depend solely on delivery of the posted Closed callback.
                self.closed(serial);
            }
        }
        Ok(())
    }

    fn queue_activation_check(&self) {
        let active = self.active.borrow().clone();
        if let Some(active) = active {
            // Defer until activation messages have settled. Moving focus between
            // the anchor and XAML's popup must not dismiss the menu.
            unsafe {
                PostMessageW(self.hwnd, CHECK_ACTIVATION, active.serial as usize, 0);
            }
        }
    }

    fn is_menu_window(window: HWND) -> bool {
        // XAML's popup may hold activation instead of the empty anchor. Both
        // belong to this dedicated STA; another application/settings does not.
        // SAFETY: querying HWND/thread identity does not dereference a pointer.
        unsafe {
            !window.is_null()
                && GetWindowThreadProcessId(window, null_mut()) == GetCurrentThreadId()
        }
    }

    fn has_foreground(&self) -> bool {
        Self::is_menu_window(unsafe { GetForegroundWindow() })
    }

    fn check_activation(&self, serial: u32) -> Result<()> {
        let active = self.active.borrow().clone();
        if active.is_some_and(|active| active.serial == serial) {
            // Read once: activation may change between two foreground queries.
            // NULL is transient during handoff; the watchdog checks again later.
            let foreground = unsafe { GetForegroundWindow() };
            if !foreground.is_null() && !Self::is_menu_window(foreground) {
                self.begin_close(serial)?;
            }
        }
        Ok(())
    }

    fn watchdog(&self) -> Result<()> {
        let active = self.active.borrow().clone();
        let Some(active) = active else {
            return Ok(());
        };
        if active
            .close_deadline
            .get()
            .is_some_and(|deadline| Instant::now() >= *deadline)
        {
            active.closing_started.set(true);
            return self.exit_completed(active.serial);
        }
        self.check_activation(active.serial)
    }

    fn dismiss(&self) {
        // KillTimer does not remove already queued WM_TIMER messages. The handler
        // reads only the current menu/deadline, so an old tick cannot close a new one.
        unsafe {
            KillTimer(self.hwnd, WATCHDOG);
        }
        let active = self.active.borrow_mut().take();
        drop(active);
        // SAFETY: hide only the empty anchor; XAML owns the actual popup.
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn closed(&self, serial: u32) {
        let active = self.active.borrow().clone();
        let Some(active) = active.filter(|active| active.serial == serial) else {
            return;
        };
        let command = active.command.get();
        self.dismiss();
        // Dispatch after confirmed dismissal, normally after the exit animation.
        if command != 0 {
            // SAFETY: the main application receives only an integer command.
            unsafe {
                PostMessageW(self.owner as HWND, ACTION, command as usize, 0);
            }
        }
    }

    fn translate(&self, message: &MSG) -> bool {
        // Explicit value conversion keeps the two versions of bindings isolated.
        let native = XamlMessage {
            hwnd: XamlHwnd(message.hwnd as isize),
            message: message.message,
            wParam: windows_xaml::Win32::Foundation::WPARAM(message.wParam),
            lParam: windows_xaml::Win32::Foundation::LPARAM(message.lParam),
            time: message.time,
            pt: windows_xaml::Win32::Foundation::POINT {
                x: message.pt.x,
                y: message.pt.y,
            },
        };
        let mut handled = XamlBool(0);
        // SAFETY: both records are correctly typed locals on the XAML owner thread.
        unsafe {
            self.native
                .PreTranslateMessage(&native, &mut handled)
                .is_ok()
                && handled.0 != 0
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: detach callbacks before closing the source and destroying HWND.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
        }
        unsafe {
            KillTimer(self.hwnd, WATCHDOG);
        }
        let _ = self.anchor.RemoveLoaded(self.loaded);
        self.active.get_mut().take();
        self.source.take();
        // SAFETY: this worker owns the host and the source is now detached.
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

fn card_popup(root: &UIElement) -> Result<Popup> {
    let presenter: UIElement = VisualTreeHelper::GetParent(root)?.cast()?;
    let popups = VisualTreeHelper::GetOpenPopupsForXamlRoot(root.XamlRoot()?)?;
    for index in 0..popups.Size()? {
        let popup = popups.GetAt(index)?;
        if popup.Child()? == presenter {
            return Ok(popup);
        }
    }
    Err(Error::new(
        HRESULT(0x80004005u32 as i32),
        HSTRING::from("Menu popup not found"),
    ))
}

fn theme() -> ElementTheme {
    let mut light = 1u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: query one preference DWORD. XAML still owns high-contrast resources.
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut light as *mut u32).cast(),
            &mut size,
        );
    }
    if light == 0 {
        ElementTheme::Dark
    } else {
        ElementTheme::Light
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the boxed Session outlives this HWND and clears userdata at teardown.
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Session;
        if let Some(session) = pointer.as_ref() {
            match message {
                LOADED => {
                    if let Err(error) = session.show_if_loaded() {
                        session.dismiss();
                        report(session.owner, &error);
                    }
                    return 0;
                }
                CHOSEN => {
                    let active = session.active.borrow().clone();
                    if let Some(active) = active.filter(|active| active.serial == lparam as u32) {
                        active.command.set(wparam as u32);
                    }
                    return 0;
                }
                CARD_READY | ENTRY_COMPLETED | BEGIN_CLOSE | EXIT_COMPLETED => {
                    let serial = wparam as u32;
                    let result = match message {
                        CARD_READY => session.enter_card(serial),
                        ENTRY_COMPLETED => session.entry_completed(serial),
                        BEGIN_CLOSE => session.begin_close(serial),
                        _ => session.exit_completed(serial),
                    };
                    if let Err(error) = result {
                        eprintln!("Cannot animate the menu: {error}");
                        if message == BEGIN_CLOSE {
                            // Closing was cancelled for animation; never leave
                            // the menu stuck open if animation setup fails.
                            let _ = session.exit_completed(serial);
                        }
                    }
                    return 0;
                }
                CLOSED => {
                    session.closed(wparam as u32);
                    return 0;
                }
                WM_ACTIVATEAPP if wparam == 0 => {
                    session.queue_activation_check();
                }
                WM_ACTIVATE if wparam as u16 == WA_INACTIVE as u16 => {
                    session.queue_activation_check();
                }
                CHECK_ACTIVATION => {
                    if let Err(error) = session.check_activation(wparam as u32) {
                        eprintln!("Cannot dismiss an inactive menu: {error}");
                        let _ = session.exit_completed(wparam as u32);
                    }
                    return 0;
                }
                WM_TIMER if wparam == WATCHDOG => {
                    if let Err(error) = session.watchdog() {
                        eprintln!("Cannot complete menu dismissal: {error}");
                        session.dismiss();
                    }
                    return 0;
                }
                WM_CLOSE | WM_CANCELMODE => {
                    let active = session.active.borrow().clone();
                    if let Some(active) = active
                        && let Err(error) = session.begin_close(active.serial)
                    {
                        eprintln!("Cannot close the menu: {error}");
                        let _ = session.exit_completed(active.serial);
                    }
                    return 0;
                }
                _ => (),
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn pump_until(session: &Session, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        // SAFETY: run the same STA message forwarding as the real menu worker.
        unsafe {
            while !done() {
                assert!(Instant::now() < deadline, "XAML popup event timed out");
                let mut message = MSG::default();
                while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if !session.translate(&message) {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                MsgWaitForMultipleObjectsEx(0, null(), 10, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
        }
    }

    // A different UI thread is essential: our foreground predicate intentionally
    // accepts both the anchor and XAML popup on the menu STA.
    struct OutsideWindow {
        hwnd: usize,
        id: u32,
        thread: Option<JoinHandle<()>>,
    }

    impl OutsideWindow {
        fn new() -> Self {
            let (sender, receiver) = mpsc::channel();
            let thread = std::thread::spawn(move || unsafe {
                let window = WindowGuard(CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    w!("STATIC"),
                    w!("Tessera dismissal test"),
                    WS_OVERLAPPEDWINDOW,
                    20,
                    20,
                    180,
                    100,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                ));
                assert!(!window.0.is_null());
                ShowWindow(window.0, SW_SHOWNOACTIVATE);
                sender
                    .send((window.0 as usize, GetCurrentThreadId()))
                    .unwrap();
                let mut message = MSG::default();
                while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            });
            let (hwnd, id) = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            Self {
                hwnd,
                id,
                thread: Some(thread),
            }
        }
    }

    impl Drop for OutsideWindow {
        fn drop(&mut self) {
            unsafe {
                PostThreadMessageW(self.id, WM_QUIT, 0, 0);
            }
            let thread = self.thread.take().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !thread.is_finished() && Instant::now() < deadline {
                unsafe {
                    let mut message = MSG::default();
                    while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    MsgWaitForMultipleObjectsEx(0, null(), 10, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                }
            }
            assert!(thread.is_finished(), "outside window teardown timed out");
            thread.join().unwrap();
        }
    }

    #[test]
    #[ignore = "Requires an interactive Windows 10 1903+/11 desktop with inbox XAML"]
    fn desktop_flyout_uses_system_controls_opens_closes_and_ignores_stale_events() {
        // Test framework threads may already have another apartment model.
        std::thread::spawn(|| -> Result<()> {
            // SAFETY: this test owns its STA, XAML source, HWND and message pump.
            unsafe {
                RoInitialize(RO_INIT_SINGLETHREADED)?;
            }
            let _apartment = Apartment;
            let _manager = Manager(WindowsXamlManager::InitializeForCurrentThread()?);
            // Exercise XamlReader and instantiate both palettes/templates: a
            // successful cross-build alone cannot catch invalid runtime markup.
            for theme in [ElementTheme::Light, ElementTheme::Dark] {
                if let Some(appearance) = Appearance::load(theme)? {
                    use windows_xaml::UI::Xaml::{
                        Controls::{Border, Control, Grid, Image, MenuFlyoutPresenter},
                        Media::{
                            AcrylicBackgroundSource, AcrylicBrush, VisualTreeHelper,
                            XamlCompositionBrushBase,
                        },
                    };
                    let presenter = MenuFlyoutPresenter::new()?;
                    presenter
                        .cast::<FrameworkElement>()?
                        .SetStyle(&appearance.presenter)?;
                    assert_eq!(
                        presenter.cast::<FrameworkElement>()?.RequestedTheme()?,
                        theme
                    );
                    let control: Control = presenter.cast()?;
                    assert!(control.ApplyTemplate()?);
                    let root: Grid = VisualTreeHelper::GetChild(&presenter, 0)?.cast()?;
                    let image: Image = appearance::shadow_image(&root.cast()?)?;
                    let border: Border = VisualTreeHelper::GetChild(&root, 1)?.cast()?;
                    assert_eq!(border.CornerRadius()?.TopLeft, 8.0);
                    assert_eq!(image.NineGrid()?.Left, shadow::CAP);
                    assert_eq!(
                        border.cast::<FrameworkElement>()?.Margin()?.Left,
                        shadow::GUTTER as f64
                    );
                    assert!(!presenter.IsDefaultShadowEnabled()?);
                    let card: UIElement = root.cast()?;
                    assert!(card.Shadow().is_err());
                    assert!(border.cast::<UIElement>()?.Shadow().is_err());
                    let motion = appearance::Card::new(card.clone())?;
                    // Revealing only the image must not change the card plane,
                    // material, or fill opacity. Its entire center is clear.
                    motion.show_static()?;
                    assert_eq!(
                        card.RenderTransform()?
                            .cast::<windows_xaml::UI::Xaml::Media::TranslateTransform>()?
                            .Y()?,
                        0.0
                    );
                    assert_eq!(border.cast::<UIElement>()?.Opacity()?, 1.0);
                    assert_eq!(image.cast::<UIElement>()?.Opacity()?, 1.0);
                    image
                        .Source()?
                        .cast::<windows_xaml::UI::Xaml::Media::Imaging::WriteableBitmap>()?;
                    let brush: AcrylicBrush = control.Background()?.cast()?;
                    assert_eq!(
                        brush.BackgroundSource()?,
                        AcrylicBackgroundSource::HostBackdrop
                    );
                    let tint = brush.TintColor()?;
                    let fallback = brush.cast::<XamlCompositionBrushBase>()?.FallbackColor()?;
                    assert!((brush.TintOpacity()? - 0.6).abs() < f64::EPSILON);
                    if let Ok(lum) = brush.TintLuminosityOpacity()
                        && let Ok(lum_val) = lum.Value()
                    {
                        assert!((lum_val - 0.8).abs() < f64::EPSILON);
                    }
                    assert_eq!(
                        (tint.R, tint.G, tint.B),
                        if theme == ElementTheme::Dark {
                            (0x20, 0x20, 0x20)
                        } else {
                            (0xFC, 0xFC, 0xFC)
                        }
                    );
                    assert_eq!(
                        (fallback.R, fallback.G, fallback.B),
                        if theme == ElementTheme::Dark {
                            (0x2C, 0x2C, 0x2C)
                        } else {
                            (0xF9, 0xF9, 0xF9)
                        }
                    );
                }
            }
            let session = Box::new(Session::new(0)?);
            // SAFETY: boxed session stays alive until native window teardown.
            unsafe {
                SetWindowLongPtrW(
                    session.hwnd,
                    GWLP_USERDATA,
                    (&*session as *const Session) as isize,
                );
            }
            let mut monitor = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: get a real work-area boundary to exercise taskbar placement,
            // including the opaque card's gap rather than only its host hint.
            unsafe {
                let handle = MonitorFromPoint(POINT { x: 100, y: 100 }, MONITOR_DEFAULTTONEAREST);
                if GetMonitorInfoW(handle, &mut monitor) == 0 {
                    return Err(Error::from_win32());
                }
            }
            let mut previous = 0;
            for locked in [false, true] {
                let request = Request {
                    x: monitor.rcWork.left + 100,
                    y: monitor.rcWork.bottom + 1,
                    locked,
                    paused: locked,
                    foreground: unsafe { GetForegroundWindow() } as usize,
                };
                session.open(request)?;
                let opened_at = Instant::now();
                let active = session.active.borrow().clone().unwrap();
                pump_until(&session, || active.base.IsOpen().unwrap_or(false));
                if active.custom_shadow {
                    pump_until(&session, || active.shadow_started.get());
                    assert!(active.entry_complete.get());
                    if UISettings::new()?.AnimationsEnabled()? {
                        assert!(
                            opened_at.elapsed() >= Duration::from_millis(190),
                            "shadow started before the 200-ms entry could complete"
                        );
                    }
                }
                assert!(!active.base.IsConstrainedToRootBounds()?);
                assert_eq!(active.menu.Items()?.Size()?, 4);
                active.menu.Items()?.GetAt(2)?.cast::<MenuFlyoutSeparator>()?;
                if active.custom_shadow {
                    let first: MenuFlyoutItem = active.menu.Items()?.GetAt(0)?.cast()?;
                    let root = appearance::card_root(&first)?;
                    let root_fe: FrameworkElement = root.cast()?;
                    let mut rows = 0.0;
                    for index in 0..4 {
                        let row: FrameworkElement = active.menu.Items()?.GetAt(index)?.cast()?;
                        let margin = row.Margin()?;
                        rows += row.ActualHeight()? + margin.Top + margin.Bottom;
                    }
                    let expected = rows + 4.0 + 2.0 + 2.0 * shadow::GUTTER as f64;
                    assert!(
                        (root_fe.ActualHeight()? - expected).abs() <= 1.0,
                        "shadow decoration enlarged the menu: {} vs {expected}",
                        root_fe.ActualHeight()?
                    );
                    assert!(
                        root_fe.ActualWidth()? < 300.0,
                        "compact menu expanded to its maximum width"
                    );
                    // Transform all the way to the XAML root, independently of
                    // the popup's requested offset, to catch layout repositioning.
                    let bottom = root.TransformToVisual(None::<UIElement>)?
                        .TransformPoint(Point {
                            X: 0.0,
                            Y: (root_fe.ActualHeight()? - shadow::GUTTER as f64) as f32,
                        })?;
                    let scale = root.XamlRoot()?.RasterizationScale()?;
                    let mut origin = POINT::default();
                    // SAFETY: only read our island's client-to-screen mapping.
                    unsafe {
                        assert_ne!(ClientToScreen(session.native.WindowHandle()?.0 as HWND,
                            &mut origin), 0);
                    }
                    let actual_bottom = origin.y as f64 + bottom.Y as f64 * scale;
                    let expected_bottom = active.target_edge.get() as f64
                        - (shadow::TASKBAR_GAP as f64 * scale).round();
                    assert!(
                        (actual_bottom - expected_bottom).abs() <= 1.0,
                        "visible card did not reach the taskbar gap: {actual_bottom} vs {expected_bottom}"
                    );
                }
                let item: MenuFlyoutItem = active.menu.Items()?.GetAt(0)?.cast()?;
                assert_eq!(
                    item.Text()?.to_string_lossy(),
                    if locked {
                        "取消固定"
                    } else {
                        "固定位置"
                    }
                );
                let icon: FontIcon = item.Icon()?.cast()?;
                assert_eq!(icon.FontSize()?, 16.0);
                let symbol = if locked { Symbol::UnPin } else { Symbol::Pin };
                assert_eq!(
                    icon.Glyph()?.to_string_lossy(),
                    char::from_u32(symbol.0 as u32).unwrap().to_string()
                );
                assert_eq!(
                    active.base.AreOpenCloseAnimationsEnabled()?,
                    !active.custom_shadow && UISettings::new()?.AnimationsEnabled()?
                );
                if previous != 0 {
                    session.closed(previous);
                    assert!(session.is_open());
                    // SAFETY: stale callback must not set the current menu's command.
                    unsafe {
                        SendMessageW(
                            session.hwnd,
                            CHOSEN,
                            super::super::QUIT as usize,
                            previous as isize,
                        );
                    }
                    assert_eq!(active.command.get(), 0);
                    session.entry_completed(previous)?;
                }
                previous = active.serial;
                active.base.Hide()?;
                pump_until(&session, || !session.is_open());
            }
            session.open(Request {
                x: 100,
                y: 100,
                locked: false,
                paused: false,
                foreground: unsafe { GetForegroundWindow() } as usize,
            })?;
            let interrupted = session.active.borrow().clone().unwrap();
            if interrupted.custom_shadow {
                session.begin_close(interrupted.serial)?;
                // A queued entry completion cannot start a shadow during exit.
                session.entry_completed(interrupted.serial)?;
                session.enter_card(interrupted.serial)?;
                assert!(!interrupted.shadow_started.get());
                assert!(interrupted.closing_started.get());
                session.entry_completed(previous)?;
            } else {
                interrupted.base.Hide()?;
            }
            pump_until(&session, || !session.is_open());

            // A request from before a foreground switch must never show later.
            session.open(Request {
                x: 100, y: 100, locked: false, paused: false,
                foreground: usize::MAX,
            })?;
            assert!(!session.is_open());

            let outside = OutsideWindow::new();
            for lose_focus in [false, true] {
                session.open(Request {
                    x: monitor.rcWork.left + 100,
                    y: monitor.rcWork.bottom + 1,
                    locked: false, paused: false,
                    foreground: unsafe { GetForegroundWindow() } as usize,
                })?;
                let active = session.active.borrow().clone().unwrap();
                pump_until(&session, || active.base.IsOpen().unwrap_or(false));
                if active.custom_shadow {
                    pump_until(&session, || active.card.borrow().is_some());
                }
                assert!(session.has_foreground());
                // Spurious/stale notifications during internal focus handoff
                // must not dismiss a menu that still owns the foreground.
                unsafe {
                    SendMessageW(session.hwnd, CHECK_ACTIVATION, previous as usize, 0);
                    SendMessageW(session.hwnd, CHECK_ACTIVATION, active.serial as usize, 0);
                    SendMessageW(session.hwnd, WM_TIMER, WATCHDOG, 0);
                }
                assert!(!active.closing_started.get());
                if lose_focus {
                    // Real cross-thread activation, not a direct Hide call.
                    unsafe { assert_ne!(SetForegroundWindow(outside.hwnd as HWND), 0); }
                    pump_until(&session, || !session.is_open());
                    assert_eq!(active.command.get(), 0);
                } else {
                    session.begin_close(active.serial)?;
                    // Cancel the storyboard and remove its Completed handler.
                    // The native timer must still release the actual popup.
                    active.animation.borrow_mut().take();
                    let deadline = *active.close_deadline.get().unwrap();
                    session.begin_close(active.serial)?;
                    assert_eq!(*active.close_deadline.get().unwrap(), deadline);
                    pump_until(&session, || !session.is_open());
                    if active.custom_shadow && UISettings::new()?.AnimationsEnabled()? {
                        assert!(Instant::now() >= deadline);
                        assert!(active.allow_close.load(Ordering::Acquire));
                    }
                }
                assert!(!active.base.IsOpen()?);
                // Already-queued ticks after KillTimer are harmless.
                unsafe { SendMessageW(session.hwnd, WM_TIMER, WATCHDOG, 0); }
                assert!(!session.is_open());
            }
            Ok(())
        })
        .join()
        .expect("XAML test thread panicked")
        .unwrap();
    }
}
