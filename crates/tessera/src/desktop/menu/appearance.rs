//! Windows 11 taskbar context-menu styling for inbox MenuFlyout controls.
//! Uses Windows 11 Fluent Design Desktop Acrylic (HostBackdrop) material.
use super::shadow;
use std::cell::RefCell;
use windows_xaml::{
    Foundation::{EventHandler, EventRegistrationToken, IReference, PropertyValue, TimeSpan},
    UI::{
        ViewManagement::AccessibilitySettings,
        Xaml::{
            Controls::{Grid, Image, MenuFlyoutItem, MenuFlyoutPresenter},
            DependencyObject, Duration, DurationType, ElementTheme, FrameworkElement,
            Markup::XamlReader,
            Media::{
                Animation::{
                    CubicEase, DoubleAnimation, EasingFunctionBase, EasingMode, FillBehavior,
                    Storyboard, Timeline,
                },
                Imaging::WriteableBitmap,
                TranslateTransform, VisualTreeHelper,
            },
            ResourceDictionary, SizeChangedEventHandler, Style, UIElement,
        },
    },
    Win32::System::WinRT::IBufferByteAccess,
    core::{Error, HSTRING, IInspectable, Interface, Result},
};

const MARKUP: &str = include_str!("../../../resources/windows/context-menu.xaml");

pub(super) struct Appearance {
    pub presenter: Style,
    pub item: Style,
    pub separator: Style,
}

impl Appearance {
    pub fn load(theme: ElementTheme) -> Result<Option<Self>> {
        // Let Windows supply its complete high-contrast palette and templates.
        if AccessibilitySettings::new()?.HighContrast()? {
            return Ok(None);
        }
        let (
            tint_color,
            fallback_color,
            tint_opacity,
            tint_luminosity_opacity,
            border,
            separator,
            hover_bg,
            pressed_bg,
        ) = if theme == ElementTheme::Dark {
            (
                "#202020",
                "#2C2C2C",
                "0.6",
                "0.8",
                "#18FFFFFF",
                "#14FFFFFF",
                "#10FFFFFF",
                "#18FFFFFF",
            )
        } else {
            (
                "#FCFCFC",
                "#F9F9F9",
                "0.6",
                "0.8",
                "#0F000000",
                "#0D000000",
                "#0A000000",
                "#14000000",
            )
        };
        let markup = MARKUP
            .replace("@SHADOW_GUTTER@", &shadow::GUTTER.to_string())
            .replace("@SHADOW_CAP@", &shadow::CAP.to_string())
            .replace(
                "@THEME@",
                if theme == ElementTheme::Dark {
                    "Dark"
                } else {
                    "Light"
                },
            )
            .replace("@TINT_COLOR@", tint_color)
            .replace("@FALLBACK_COLOR@", fallback_color)
            .replace("@TINT_OPACITY@", tint_opacity)
            .replace("@TINT_LUMINOSITY_OPACITY@", tint_luminosity_opacity)
            .replace("@BORDER@", border)
            .replace("@SEPARATOR@", separator)
            .replace("@HOVER_BG@", hover_bg)
            .replace("@PRESSED_BG@", pressed_bg);
        // StaticResource references keep the palette with each style, including
        // the flyout's separate popup. Nothing is installed in global resources.
        let resources: ResourceDictionary = XamlReader::Load(HSTRING::from(markup))?.cast()?;
        let style = |name: &str| -> Result<Style> {
            resources
                .Lookup(PropertyValue::CreateString(HSTRING::from(name))?)?
                .cast()
        };
        Ok(Some(Self {
            presenter: style("Presenter")?,
            item: style("Item")?,
            separator: style("Separator")?,
        }))
    }
}

/// Return only our rounded card, never another flyout or a native popup HWND.
/// The first item's Loaded event guarantees its presenter template exists.
pub(super) fn card_root(item: &MenuFlyoutItem) -> Result<UIElement> {
    let mut parent: DependencyObject = item.cast()?;
    loop {
        if let Ok(presenter) = parent.cast::<MenuFlyoutPresenter>() {
            let card: Grid = VisualTreeHelper::GetChild(&presenter, 0)?.cast()?;
            return card.cast();
        }
        parent = VisualTreeHelper::GetParent(&parent)?;
    }
}

/// The shadow is an absolutely positioned decoration, not a layout child.
pub(super) fn shadow_image(root: &UIElement) -> Result<Image> {
    let canvas = VisualTreeHelper::GetChild(root, 0)?;
    let bounds = VisualTreeHelper::GetChild(canvas, 0)?;
    VisualTreeHelper::GetChild(bounds, 0)?.cast()
}

fn resize_shadow(root: &UIElement) -> Result<()> {
    let size: FrameworkElement = root.cast()?;
    let canvas = VisualTreeHelper::GetChild(root, 0)?;
    let bounds: FrameworkElement = VisualTreeHelper::GetChild(canvas, 0)?.cast()?;
    bounds.SetWidth(size.ActualWidth()?)?;
    bounds.SetHeight(size.ActualHeight()?)
}

pub(super) type Completed = EventHandler<IInspectable>;

/// A XAML clock with a real completion event and deterministic cancellation.
pub(super) struct Animation {
    storyboard: Storyboard,
    completed: Option<EventRegistrationToken>,
}

impl Animation {
    fn start(storyboard: Storyboard, completed: Option<Completed>) -> Result<Self> {
        let timeline: Timeline = storyboard.cast()?;
        timeline.SetFillBehavior(FillBehavior::Stop)?;
        let completed = completed
            .map(|handler| timeline.Completed(handler))
            .transpose()?;
        let result = Self {
            storyboard,
            completed,
        };
        result.storyboard.Begin()?;
        Ok(result)
    }
}

impl Drop for Animation {
    fn drop(&mut self) {
        if let Some(token) = self.completed
            && let Ok(timeline) = self.storyboard.cast::<Timeline>()
        {
            let _ = timeline.RemoveCompleted(token);
        }
        let _ = self.storyboard.Stop();
    }
}

fn add_animation(
    storyboard: &Storyboard,
    target: &DependencyObject,
    property: &str,
    values: (f64, f64),
    milliseconds: i64,
) -> Result<()> {
    let animation = DoubleAnimation::new()?;
    animation.SetFrom(PropertyValue::CreateDouble(values.0)?.cast::<IReference<f64>>()?)?;
    animation.SetTo(PropertyValue::CreateDouble(values.1)?.cast::<IReference<f64>>()?)?;
    let easing: EasingFunctionBase = CubicEase::new()?.cast()?;
    easing.SetEasingMode(EasingMode::EaseOut)?;
    animation.SetEasingFunction(easing)?;
    let timeline: Timeline = animation.cast()?;
    timeline.SetDuration(Duration {
        TimeSpan: TimeSpan {
            Duration: milliseconds * 10_000,
        },
        Type: DurationType::TimeSpan,
    })?;
    Storyboard::SetTarget(&timeline, target)?;
    Storyboard::SetTargetProperty(&timeline, HSTRING::from(property))?;
    storyboard.Children()?.Append(timeline)
}

pub(super) fn show_static(root: &UIElement) -> Result<()> {
    root.RenderTransform()?
        .cast::<TranslateTransform>()?
        .SetY(0.0)?;
    root.SetOpacity(1.0)?;
    resize_shadow(root)?;
    shadow_image(root)?.cast::<UIElement>()?.SetOpacity(1.0)
}

pub(super) struct Card {
    element: UIElement,
    shadow: UIElement,
    offset: TranslateTransform,
    resized: EventRegistrationToken,
    shadow_animation: RefCell<Option<Animation>>,
}

impl Card {
    pub fn new(element: UIElement) -> Result<Self> {
        let image = shadow_image(&element)?;
        let dark = element
            .cast::<windows_xaml::UI::Xaml::FrameworkElement>()?
            .ActualTheme()?
            == ElementTheme::Dark;
        let pixels = shadow::pixels(dark);
        let bitmap = WriteableBitmap::CreateInstanceWithDimensions(
            shadow::SIZE as i32,
            shadow::SIZE as i32,
        )?;
        let buffer = bitmap.PixelBuffer()?;
        let invalid_buffer = || {
            Error::new(
                windows_xaml::core::HRESULT(0x80004005u32 as i32),
                HSTRING::from("Invalid menu shadow pixel buffer"),
            )
        };
        if buffer.Capacity()? < pixels.len() as u32 {
            return Err(invalid_buffer());
        }
        let access: IBufferByteAccess = buffer.cast()?;
        // SAFETY: this WriteableBitmap owns a SIZE*SIZE*4 BGRA buffer. Capacity
        // was checked; the buffer/access objects stay alive through the copy.
        unsafe {
            let address = access.Buffer()?;
            if address.is_null() {
                return Err(invalid_buffer());
            }
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), address, pixels.len());
        }
        bitmap.Invalidate()?;
        image.SetSource(bitmap)?;
        resize_shadow(&element)?;
        let offset = element.RenderTransform()?.cast::<TranslateTransform>()?;
        let resized =
            element
                .cast::<FrameworkElement>()?
                .SizeChanged(SizeChangedEventHandler::new(|sender, _| {
                    if let Some(sender) = sender {
                        resize_shadow(&sender.cast::<UIElement>()?)?;
                    }
                    Ok(())
                }))?;
        Ok(Self {
            element,
            shadow: image.cast()?,
            offset,
            resized,
            shadow_animation: RefCell::new(None),
        })
    }

    pub fn show_static(&self) -> Result<()> {
        self.shadow_animation.borrow_mut().take();
        show_static(&self.element)
    }

    pub fn enter(&self, completed: Completed) -> Result<Animation> {
        // Base values equal the final frame so stopping a completed clock
        // cannot reset opacity or move the menu back to its initial offset.
        self.element.SetOpacity(1.0)?;
        self.offset.SetY(0.0)?;
        self.shadow.SetOpacity(0.0)?;
        let storyboard = Storyboard::new()?;
        add_animation(
            &storyboard,
            &self.element.cast()?,
            "Opacity",
            (0.0, 1.0),
            200,
        )?;
        add_animation(&storyboard, &self.offset.cast()?, "Y", (50.0, 0.0), 200)?;
        Animation::start(storyboard, Some(completed))
    }

    pub fn reveal_shadow(&self, animate: bool) -> Result<()> {
        if !animate {
            return self.show_static();
        }
        self.shadow.SetOpacity(1.0)?;
        let storyboard = Storyboard::new()?;
        add_animation(
            &storyboard,
            &self.shadow.cast()?,
            "Opacity",
            (0.0, 1.0),
            400,
        )?;
        *self.shadow_animation.borrow_mut() = Some(Animation::start(storyboard, None)?);
        Ok(())
    }

    pub fn frame(&self) -> Result<(f64, f64)> {
        Ok((self.element.Opacity()?, self.offset.Y()?))
    }

    pub fn exit(&self, frame: (f64, f64), completed: Completed) -> Result<Animation> {
        self.element.SetOpacity(0.0)?;
        self.offset.SetY(0.0)?;
        let storyboard = Storyboard::new()?;
        add_animation(
            &storyboard,
            &self.element.cast()?,
            "Opacity",
            (frame.0, 0.0),
            100,
        )?;
        add_animation(&storyboard, &self.offset.cast()?, "Y", (frame.1, 0.0), 100)?;
        Animation::start(storyboard, Some(completed))
    }
}

impl Drop for Card {
    fn drop(&mut self) {
        self.shadow_animation.get_mut().take();
        if let Ok(element) = self.element.cast::<FrameworkElement>() {
            let _ = element.RemoveSizeChanged(self.resized);
        }
    }
}
