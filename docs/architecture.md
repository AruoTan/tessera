# Architecture

Tessera is a Windows taskbar monitor written entirely in Rust. The taskbar
widget, native settings controls, context menu, and application lifecycle use
Windows APIs directly.

All source paths below are relative to the repository root. For setup, packaging,
and desktop validation, see [Development](development.md).

## Overview

- Physical Ethernet/Wi-Fi byte counters sampled in Rust every second, with selectable adapters.
- System-wide CPU, GPU, and physical memory usage, in three fixed-width sections to the right of network speeds.
- A Windows taskbar readout with DirectWrite/Direct2D text on a dedicated native window thread.
- Drag the readout along the taskbar; its position is remembered across restarts.
- Right-click the readout for Lock/unlock position, Pause/resume and Quit; left-double-click it to open Settings. The application has no notification-area icon.
- A native Win32 settings window created on demand, using system fonts, keyboard navigation, and per-monitor DPI scaling. Closing it destroys the window while sampling continues.
- Settings for adapter selection and taskbar position reset. Speeds always use bytes per second and two decimal places, with text adapting to the taskbar theme.
- Validated settings saved atomically in `%APPDATA%\io.tessera\settings.json`.
- Windows CI and a manual action producing an unsigned NSIS installer, portable ZIP, and checksums.

The widget currently targets the **primary taskbar**, with DPI-aware layout and
automatic attachment to a replacement Explorer taskbar.
These integration paths need real Windows desktop testing; cross-compilation is
not a substitute for testing the desktop behavior.
Narrow vertical taskbars can clip the readout; horizontal taskbars are the first target.

## Taskbar rendering

The readout is always enabled while the app runs.
Its initial layered surface is drawn before explicitly showing the window without
activation; no taskbar click is required. The readout is a layered **child window**
of the taskbar, so Windows moves, clips, shows, and hides it with its parent,
including the taskbar's own auto-hide animation. It stays with the taskbar during
thumbnail/desktop Peek and follows the taskbar's actual visibility in full-screen
applications. It has no independent desktop topmost band or animation timer.
Upload and download use transparent, rounded cloud-arrow SVG icons in
`crates/tessera/assets/icons/`. Both icons and their network text use black (`#000000`)
on a light taskbar and white (`#FFFFFF`) on a dark taskbar, updating automatically
when the Windows system theme changes. The 16-DIP icons are rasterized and
cached at the taskbar's DPI. The readout reserves space for icons, numbers, and
right-aligned units. Its width is measured from the font, DPI, and fixed
two-decimal precision, rather than the current reading. Each number is centered between the
icon's right edge and its actual unit text's left edge, with equal padding on
both sides. Shorter units such as `B/s` give the number more room than `KB/s`;
the number's center adjusts to that space while the units retain their shared
right edge. Values and units can change without resizing the widget or shifting
its taskbar position. The full readout,
including blank column space, remains draggable while its position is unlocked.
The context menu offers “固定位置” (Lock position) with the system pin icon, changing
to “取消固定” (Unlock) with the system unpin icon when locked. Locking disables drag
capture and uses the normal arrow cursor; double-click and right-click actions
remain available. The lock persists across restarts. Settings saves preserve
the current lock and dragged position; explicitly resetting the position remains
available without unlocking.

## XAML context menu

The menu is an actual [`Windows.UI.Xaml.Controls.MenuFlyout`](https://learn.microsoft.com/en-us/uwp/api/windows.ui.xaml.controls.menuflyout),
with standard `MenuFlyoutItem`, `MenuFlyoutSeparator` and `FontIcon` objects.
`resources/windows/context-menu.xaml` supplies an 8-DIP rounded presenter,
Fluent Desktop Acrylic (`HostBackdrop`) material with balanced tint and luminosity opacity
that naturally responds to light and dark underlying application surfaces, a subtle outline,
compact 32-DIP rows and 12-DIP text. A separate hollow edge-shadow image sits
behind the rounded card. Both the presenter's automatic shadow and the card's
`ThemeShadow` are absent. A custom item template supplies icon/text columns and
pointer/pressed/disabled states.
Icons use 16-DIP Segoe Fluent Icons with Segoe MDL2 Assets as the Windows 10 fallback.
XAML supplies accessibility, keyboard navigation and popup placement;
XAML storyboards animate the card motion and the edge image's opacity. No HMENU, bitmap
menu icons, or shell-private interface is used.
This is the Windows inbox XAML control, not a claim to call Explorer's private
tray-menu implementation or to reproduce its exact appearance on every release.

The first right-click starts a dedicated STA menu thread. It initializes
`WindowsXamlManager` and attaches `DesktopWindowXamlSource` to a tiny native
anchor, using the documented [XAML Islands hosting API](https://learn.microsoft.com/en-us/windows/uwp/xaml-islands/host-standard-control-with-xaml-islands-cpp).
The anchor's own region is empty; the MenuFlyout is explicitly unconstrained and
uses the framework's separate top-level popup. Presentation waits for `Loaded`
and assigns the anchor's actual `XamlRoot` before `ShowAt`. Physical cursor
coordinates position the anchor, while XAML handles popup DPI/layout.
`IDesktopWindowXamlSourceNative2.PreTranslateMessage` receives the worker's
messages so XAML handles keyboard input.

Each opening creates fresh menu items, reads lock/pause state and selects the
Windows system light/dark theme. Styles are loaded from embedded markup on the
menu STA, with palette references resolved locally rather than modifying global
resources. In high contrast, custom styles are bypassed so colors, outlines
and item templates remain framework-owned.
`UISettings.AnimationsEnabled` controls all animation; the app never changes the
system-wide setting. Inbox `FlyoutBase.Opened` means the presenter is loaded,
not that its entrance transition has completed. For the custom template,
`AreOpenCloseAnimationsEnabled` is false and XAML `Storyboard` owns the card's
entry/exit motion. `DoubleAnimation` targets the card opacity and its ordinary
`TranslateTransform.Y`. Completion is observed through the storyboard's own
`Completed` event; no handed-out Composition visual or custom visual property
is involved. Resting dependency-property values match each final frame, and
`FillBehavior.Stop` plus explicit cancellation prevents retained animation clocks.
The first item's `Loaded` message starts a 200-ms upward slide and fade with
the shadow hidden. Only the entry storyboard's `Completed` callback can start the
400-ms opacity reveal on the separate shadow image. The Acrylic surface stays
at Z=0 with unchanged material/opacity during the entire shadow reveal. There
is no timer racing the framework's transition, and no shadow depth animation
on the material itself.

`menu/shadow.rs` generates a 64×64 premultiplied BGRA atlas (16 KiB). Its original
rounded-card footprint is completely transparent. The shadow starts at the card's
outer edge without an expanded cutout that would expose a bright backdrop seam;
corner antialiasing is confined to the first half pixel outside. A Gaussian edge
with 4.5-DIP blur and downward offset gives the sides and bottom a wider falloff
with no shadow above the top edge. Both side shadows have mirrored 45-degree
upper cutoffs: each DIP outward moves the start one DIP downward, with a 2-DIP
smooth transition and the existing corner fade. The complete diagonal caps stay
inside the fixed nine-grid corner tiles, so resizing cannot flatten their slope.
Both sides retain a 12-DIP reach; the bottom fades
to zero over the target 12-DIP card/taskbar gap. Each tail eases smoothly to zero
in its last two DIPs. Shadow amplitude is reduced by 20% after alpha quantization;
the faintest nonzero tail pixels retain their positions, preserving the fade-out
extent without changing the Gaussian falloff or geometry. The center tile is empty. XAML `Image.NineGrid` preserves
corner/edge thickness while stretching to the menu dimensions. The image is inside a sized grid hosted by
an absolute `Canvas`, so it cannot participate in the presenter's `DesiredSize`.
The foreground border and its three menu items alone determine the menu size.
`SizeChanged` updates only the decoration's bounds. Image width/height remain
Auto so nine-grid rendering stays enabled. `WriteableBitmap.PixelBuffer` is filled once
per opening; the temporary CPU pixels are then dropped. There is no added image
file, decoder, native shadow window, or per-frame CPU rendering.
The popup reserves a 14-DIP transparent gutter, which becomes fully transparent
before its outer boundary. The image does not participate in hit testing.
Moving only the anchor cannot enforce the visible gap: FlyoutBase can reposition
the full presenter, including its shadow padding. After layout and before entry,
`GetOpenPopupsForXamlRoot` selects only the popup whose child is this card's
presenter. `TransformToVisual` measures the opaque bottom relative to that child;
the island's actual screen origin and `XamlRoot.RasterizationScale` map the target
12-DIP gap into `Popup.VerticalOffset`. XAML owns the resulting native window update.
The transparent gutter may extend past the taskbar boundary without pushing the
opaque card upward. If there is not enough room above the target, the framework's
placement is retained. A placement error also retains the framework position and
does not prevent card entry. High contrast retains native placement throughout.
The presenter explicitly requests the light/dark palette's theme. With system
animations disabled, card and shadow appear directly at rest. High contrast
retains the framework's complete template, shadow and transitions. The minimum
supported Windows version remains 10 1903.

`Closing` defers dismissal for a 100-ms card fade. Beginning exit revokes the
entry completion callback; queued entry messages are ignored while closing, so
an interrupted opening cannot start a shadow during dismissal. The exit storyboard's
completion permits `Hide`. A command is sent back to the main application only
after XAML raises `Closed`. Animation setup failures fall back to static
presentation or immediate closing rather than blocking menu actions.
The anchor is placed at its final coordinates before `ShowAt`; there is no
animation thread, popup HWND discovery or `SetWindowPos` on a framework-owned
popup. Moving that popup independently caused it to jump back when XAML next
updated its placement.
Each opening tags callbacks with a generation so a delayed event from an old
menu cannot close or select a new one. Closing releases the flyout and item
event handlers. The STA thread and XAML source remain for subsequent openings
until application shutdown; XAML can increase resident memory after first use.
No memory-reduction claim is made for this control change.

Windows 10 1903 (build 18362) or newer is required for this hosting path. The
application manifest declares a tested version above the XAML Islands minimum.
Windows supplies the runtime; installers and portable ZIPs do not bundle WinUI,
Windows App SDK, .NET, or a browser. Microsoft's `windows` 0.37 bindings are
pinned under the isolated `windows-xaml` dependency because later windows-rs
releases removed inbox XAML projections. Monitoring and widget rendering retain
the current bindings; only HWND/message values cross the binding boundary.
Initialization/presentation failures report their HRESULT, rather than silently
falling back to the old Win32 menu.
Before the first valid sample, both rows use formatted zero placeholders with
two decimal places (`0.00` and `B/s` beside each icon).
Valid samples replace the values using the same format; an unavailable adapter
still shows a dash, and paused monitoring retains its pause label.

## Startup readiness

On launch, sampling starts before the widget. Its first surface waits for CPU
and RAM usage plus a GPU detection result and, if a GPU is present, its first
usage measurement. CPU/GPU rate counters need two reads about one second apart;
the first frame usually appears after 1–2 seconds. Network has its own worker and
never gates hardware publication or rendering; speeds may start at formatted
zero if no interval is available yet. Optional CPU temperature and GPU memory
sources do not gate the first frame. A three-second startup limit allows the
available metrics to appear if a counter or driver is unavailable or stalled.
Warm-up runs only once; later read failures or Explorer restarts do not repeat it.

## Hardware gauges and text

The sections appear as **network → CPU → GPU → RAM**. If DXGI confirms no hardware
GPU is present, they become **network → CPU → RAM**, with no reserved GPU column
or extra gap. A failed detection remains unknown, and unsupported counters do
not imply absent hardware. Device changes update the layout; width and slot
mapping remain stable during a drag until release. CPU, GPU, and RAM are transparent
open circular gauges. The final part of each arc is replaced by curved `CPU`,
`GPU`, or `RAM` lettering around the rim, leaving roughly 135 degrees open
across the bottom. The remaining solid stroke shows unrounded usage; its
rounded end stops before the lettering, including at 100%.
The center shows a larger integer usage value with a smaller, baseline-aligned
percent sign. Center readouts reserve separate number and unit columns: numbers
stay centered and percent signs keep a fixed right edge with a 1-DIP gutter.
Bottom numbers align right toward the units, which align left, separated by a
4-DIP gutter. Gutters round up to a physical pixel. Each bottom readout uses the
actual number and unit widths to center the complete group beneath the arc,
including after digit counts change. Missing and paused readings center
without reserving space for an absent unit. Readouts keep a narrower available
width for temperatures than memory capacities. Bottom readings show CPU temperature
in °C; GPU memory in use and used physical memory always use GiB with one decimal
place (for example `52 °C`, `0.8 GiB`, `16.0 GiB`). One GiB is 1,073,741,824 bytes;
readings below one GiB stay in GiB, and larger readings never promote to TiB.
Rounding uses integer arithmetic across the full `u64` byte range, including
half-tenths and carry boundaries. CPU/GPU percentages and decimal network-rate
conversion retain their existing precision.
The CPU arc is sky blue, GPU is violet, and RAM is mint green, with text/track contrast adjusted
to Windows' light/dark taskbar
preference. Each hardware section reserves 48 DIP for a 40-DIP gauge; changing
values never moves its neighbors. Fonts and antialiased vector strokes scale
with taskbar DPI. There is no opaque gauge background.
The curved labels use the metric's accent color, inset slightly toward the circle
center without changing the solid arc. CPU/GPU lettering sits 1 DIP farther inward
than RAM to balance its narrower glyphs. All readout text uses DirectWrite layouts
and Direct2D grayscale antialiasing, including the individually rotated CPU/GPU/RAM
letters rendered at their final size. The installed Windows UI font is used at
regular weight: Segoe UI Variable when available, with Segoe UI as the Windows 10
fallback. Settings and the license dialog use the same Windows font family stack,
with Microsoft YaHei UI for simplified Chinese. No font is bundled.

Text renders directly in its theme color into a native-size premultiplied BGRA
WIC surface, then copies unchanged into the layered window. It is not recolored
from a GDI black/white mask. Whole-pixel horizontal placement and snapped baselines
keep upright readings stable; numbers and units share a fixed baseline in each
row. Percentage suffixes use 9 DIP and bottom units 10 DIP, with smaller number
fallbacks for three-digit usage and long binary-unit details. The circular strokes
and SVG network icons retain their vector rendering. Fully transparent background
pixels receive alpha 1 only after composition, preserving blank-space dragging
without changing the text's antialiased coverage.

Transparent output deliberately uses grayscale antialiasing rather than ClearType,
which assumes an appropriate opaque background. The WIC render target uses
Direct2D software rendering for this small surface; this change does not introduce
a GPU device or capture/repaint the taskbar background. Text layouts have a bounded
cache, and the transparent surface is reused until its physical dimensions change.
The layered-window DC and top-down DIB are also reused until the widget size
changes. Text copies directly into that DIB, avoiding an intermediate full-frame
array and a second pixel copy; native GDI resources are released on resize or shutdown.
The cache retains at most 32 recently used layouts, evicting old numeric readings
individually while keeping frequently used units and labels. System text rendering
parameters are created once per renderer and reused by each render target.
Windows settings/theme notifications refresh those parameters before repainting.
The icon SVGs are parsed during the build into vector drawing commands; the
application rasterizes those paths directly with `tiny-skia`. No SVG parser runs
in the application. Pixel-comparison tests check the generated paths against the
original SVGs across taskbar DPI scales.
DPI changes rebuild formats/layout measurements at the new scale; failed drawing
is retried and its target recreated. Actual small-text quality still needs visual
validation on Windows at 100%, 125%, 150%, and 200% scaling, in both taskbar themes.

All visible sections, including the gaps, form one draggable taskbar child window.
Network values always use two decimal places; hardware usage remains an integer percentage.

## CPU and physical memory

CPU sampling uses the Windows PDH counter
`\Processor Information(_Total)\% Processor Time`, covering overall processor
busy time rather than this application's process. The language-neutral
[PdhAddEnglishCounterW](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhaddenglishcounterw)
API supports localized Windows installations. Two CPU counter reads establish
an interval before normal initial presentation. Physical memory usage is
`(total physical - available physical) / total physical`, using
[GlobalMemoryStatusEx](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-globalmemorystatusex);
it does not include page-file capacity, and reclaimable standby pages remain
available. Both readings share a fixed one-second hardware sampler, independent
of the network sampler; each updates only its own fields in shared state. The
taskbar thread only draws published results. An unavailable reading shows a
dash independently, without interrupting other metrics or retaining stale data.
CPU query failures discard the baseline and retry after ten seconds. Pausing
shows `Paused` for network and a pause symbol on the gauges; the baselines keep
refreshing while the gauge arcs and details are suppressed.

## GPU sampling

GPU sampling uses Windows WDDM's `\GPU Engine(*)\Utilization Percentage`
performance counters, without restricting engine type: 3D, copy, compute, video
encode/decode, and unnamed engines all count. NVIDIA, AMD, and Intel use the same
OS interface; a WDDM 2.0+ driver exposing these counters is required. DXGI enumerates
hardware adapters by LUID and excludes software renderers. Counters are collected
as wildcard arrays every second, so newly started applications are included.
The first collection establishes a baseline; normal initial presentation waits
for the next measurement. Missing/invalid readings show a dash after the bounded
startup wait or on later failures.

Per-process values are summed **only within the same physical GPU and engine**;
the busiest engine represents that GPU. The busiest GPU supplies the single
GPU gauge, with the previous GPU retained on exact ties. Independent engines,
linked physical GPUs, and separate adapters are never added together; percentages
are bounded at 100%. This follows the busiest-engine approach described by
[Microsoft](https://devblogs.microsoft.com/directx/gpus-in-the-task-manager/).
Counter arrays are consumed through callbacks using one reusable UTF-8 name buffer;
no collection of owned instance-name strings is created. Aggregation reuses compact
process/engine records, sorts them in place, merges duplicates using their maximum
value, and reduces contiguous groups without intermediate trees. Normal PDH buffer
capacity is reused across collections, while buffers exceeding 256 KiB are released
after each read. Compact record capacity shrinks after a large process-count drop.
GPU memory counters are reduced directly without an intermediate result vector.

The bottom GPU readout shows **dedicated + shared GPU memory in use** on the
same selected physical GPU, in binary units with one decimal place. Shared memory
is included so integrated GPUs are represented. These are adapter-wide
`\GPU Adapter Memory(*)\Dedicated Usage` and `Shared Usage` counters, not a sum
of process memory or
[`IDXGIAdapter3::QueryVideoMemoryInfo`](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_4/nf-dxgi1_4-idxgiadapter3-queryvideomemoryinfo)
(which is process-local). If DXGI reports no dedicated memory capacity, only
shared usage is needed. Otherwise missing memory counters show a dash instead of
a fabricated zero or partial total.
GPU memory can overlap physical RAM; the GPU and RAM readouts must not be added.
GPU temperature is not shown in this gauge.

A separate Rust worker keeps GPU driver/counter delays off the network/CPU/RAM
sampler and taskbar threads. Failed queries discard their baselines and retry
after ten seconds; data older than five seconds expires. Adapter inventory and
baselines are rebuilt after DXGI reports a device change, supporting eGPU hotplug
and driver restarts. No administrator privileges, vendor CLI, external hardware
monitor, or graphics device creation is required for GPU usage/memory sampling.

## CPU temperature

CPU temperature is optional. An isolated Rust worker tries these sources
in order: local `ROOT\LibreHardwareMonitor` / `ROOT\OpenHardwareMonitor`
WMI sensors, PDH thermal-zone counters, then `ROOT\WMI`'s
`MSAcpi_ThermalZoneTemperature`. A compatible running hardware monitor is useful
for CPU die readings (for example LibreHardwareMonitor 0.9.4 or OpenHardwareMonitor;
versions that omit WMI cannot supply that interface), but is not required
when Windows exposes usable thermal zones. No monitor, .NET runtime, kernel
driver, or elevation is bundled or launched.
For LHM/OHM, `/intelcpu/` and `/amdcpu/` package/Tdie sensors are preferred,
then Tctl, then the hottest CPU core. An explicitly CPU-labelled `/lpc/` Super I/O
sensor is also accepted as a fallback. Distance-to-TjMax, GPU, unrelated board
sensors, invalid values, and missing readings are excluded.
PDH uses language-neutral counter paths and prefers High Precision Temperature
(0.1 K); standard Temperature uses whole kelvin. ACPI WMI uses 0.1 K, with
direct-Celsius OEM compatibility. A zone stuck on a round ten-kelvin
value (such as 290 K or 300 K) is ignored until it changes. Zone history is keyed
by instance name, so enumeration order does not affect that filter.
These compatibility fallbacks are firmware thermal-zone readings and may measure
a board or ambient zone rather than the CPU die; see Microsoft's
[thermal-zone explanation](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/examples--requirements-and-diagnostics).
When no valid source is available, the bottom gap shows `— °C`; CPU utilization
and RAM continue working. Failed connections are retried after ten seconds,
and readings older than five seconds expire so a stalled WMI provider cannot
leave a stale temperature on screen. Temperature polling never blocks the normal sampler
or the taskbar thread. Temperature follows the same fixed one-second cadence.

## Network units

Speeds use decimal byte units: 1 KB = 1,000 B, followed by MB, GB, TB, PB, and EB.
Each row selects its unit independently and keeps at most three integer digits.
Rounding also promotes the unit: 999.996 KB/s displays as
1.00 MB/s, rather than 1000.00 KB/s. An out-of-range value beyond the EB/s column's
capacity displays `999+ EB/s`. This formatting does not alter sampled rates or
traffic accounting.

## Explorer lifecycle

A never-visible controller survives Explorer restarts. Creation, visibility,
layout, and reorder events from the taskbar and its descendants, plus the
`TaskbarCreated` broadcast, trigger attachment and repaint on the new taskbar.
Attachment does not authorize display: a replacement readout is prepared hidden
until its current taskbar has broadcast `TaskbarCreated`, its notification area
is visible with nonempty layout, and DWM reports that the taskbar is not cloaked.
HWND/process/thread identity ties this permission to one taskbar incarnation;
destroying the old taskbar revokes it. The taskbar already present when the app
starts can use the layout/DWM checks without waiting for a past broadcast.
Missing or rebuilding taskbars are retried every 16 ms; settled content/layout
uses a 250 ms timer. Neither timer drives the hide/show animation. While no
taskbar exists, no readout is displayed. Late-created taskbar surfaces cannot
leave the readout behind its siblings: its sibling order is repaired only when
needed, without raising the taskbar above desktop windows or shell flyouts.
Stored HWNDs are checked against the widget context before destruction, and an
old window's teardown cannot clear the replacement window's reference.
These readiness checks prevent showing merely because `Shell_TrayWnd` exists;
they do not expose or control Explorer's private XAML composition timeline.
The child is created with the parent's DPI awareness, and the application manifest
declares Windows 10/11 compatibility to enable layered child windows. See the
Windows documentation for [child window behavior](https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features#child-windows)
and [layered window compatibility](https://learn.microsoft.com/en-us/windows/win32/winmsg/using-windows#using-layered-windows).

## Network sampling and accounting

**自动（物理以太网和 Wi-Fi）** is the default, including for existing settings
with no adapter selected. Windows `GetIfTable2` supplies interface types, hardware
flags, physical media, stable LUIDs, and 64-bit byte counters. Only hardware-backed
Ethernet and Wi-Fi interfaces are eligible. VPN/TUN/TAP, virtual LANs, Hyper-V/WSL/
VMware virtual interfaces, filters, endpoints, tunnels, and loopback interfaces
are excluded based on OS metadata, rather than adapter names. Bluetooth and WWAN
are outside the supported Ethernet/Wi-Fi scope. No unfiltered fallback is used.

The default sums each eligible physical interface once. Manually selecting an
adapter also uses this physical-only list; a previously selected virtual adapter
shows as unavailable until you select automatic mode or a physical adapter.
Baselines use LUIDs, so renaming an adapter keeps its identity and a replacement
with the same name does not inherit its counters. Counter resets, read failures,
and newly discovered adapters establish a baseline instead of creating spikes.
An uncertain interval is omitted rather than estimated.

Sampling uses a fixed one-second cadence. Upload/download rates are the changes
in cumulative byte counters divided by the actual elapsed monotonic time, so
scheduling delays do not inflate the result. Each reading represents the average
over that sample interval, without additional smoothing. The settings window
offers adapter selection; the sampling interval is fixed.

This measures Windows-reported traffic through the physical adapters, including
LAN traffic, broadcasts, and VPN encapsulation actually sent/received there.
VPN traffic is not added a second time from its virtual adapter; VPN overhead
and retransmissions visible to the physical adapter still consume real traffic.
With Ethernet and Wi-Fi both carrying traffic, both contribute. This is not an
Internet-only or ISP billing counter; accuracy depends on the adapter driver.
Session totals reset when the adapter selection changes and are not persisted.
Pausing excludes paused traffic and the first interval after resuming.

## Structure

| Path | Responsibility |
| --- | --- |
| `crates/tessera-core` | Settings validation/persistence, counter accounting, hardware formatting, taskbar geometry |
| `crates/tessera/src/monitor.rs` | Background sampling into shared Rust state |
| `crates/tessera/src/monitor/hardware.rs` | Windows PDH CPU counters, physical memory, and published GPU readings |
| `crates/tessera/src/monitor/gpu.rs` | Isolated DXGI/WDDM GPU selection, engine usage, and adapter memory sampling |
| `crates/tessera/src/monitor/pdh.rs` | Bounded, language-neutral wildcard counter array reads |
| `crates/tessera/src/monitor/temperature.rs` | Isolated WMI CPU-temperature polling with PDH/ACPI fallbacks |
| `crates/tessera/src/widget/gauge.rs` | DPI-scaled open gauges with curved CPU/GPU/RAM labels |
| `crates/tessera/src/widget/text.rs` | DirectWrite layouts, system fonts, and transparent Direct2D text rendering |
| `crates/tessera/src/widget/windows.rs` | Native widget thread, layered presentation, taskbar ownership |
| `crates/tessera/src/widget/startup.rs` | Bounded initial hardware warm-up, independent of network readiness |
| `crates/tessera/src/desktop/windows.rs` | Main message loop, single instance, menu action dispatch and application lifecycle |
| `crates/tessera/src/desktop/menu.rs` | Context menu command labels, state and system icon selection |
| `crates/tessera/src/desktop/menu/xaml.rs` | Lazy STA, XAML Islands host, real MenuFlyout controls and event lifecycle |
| `crates/tessera/src/desktop/menu/appearance.rs` | Per-opening theme palette and embedded menu styles |
| `crates/tessera/resources/windows/context-menu.xaml` | Rounded presenter, compact rows and separators |
| `crates/tessera/src/desktop/settings.rs` | Win32 settings controls, adapter choices and license window |
| `crates/tessera/build.rs` | Windows icon and manifest resources |
| `crates/tessera/build/icons.rs` | Build-time SVG compilation into vector drawing commands |
| `crates/tessera/assets/icons/` | Embedded application icon and widget SVG icons |
| `crates/tessera/resources/windows/app.manifest` | DPI, visual styles, and Windows compatibility |
| `packaging/` | Windows and Linux packaging entry scripts |
| `packaging/windows/installer.nsi` | Per-user NSIS installer and uninstaller |

Acquisition, settings, and the widget share `AppState` directly in Rust. The settings
window reads status on a one-second timer only while it exists. Requests to open
settings or the context menu are posted to the main Windows message loop with
the menu's screen coordinates. The widget and sampler keep their own threads,
so the readout continues updating while the menu is open.
