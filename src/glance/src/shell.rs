//! The AppKit shell: one status item in the menu bar that shows the two
//! charts.
//!
//! The shell is thin. Each rule of layout, color, text, arithmetic,
//! connection, and update lives in a module that has no AppKit. This module
//! only gets the samples on a timer, gives the values to those modules, and
//! hands the answers to AppKit.
//!
//! One repeating timer on the main run loop does all of the work, and the
//! system merges its wakeups with the wakeups of other programs. In local mode,
//! each tick reads the kernel, and the shell starts no thread, no child
//! process, and no async runtime. In remote mode, each tick reads the events of
//! a [`Feed`], which runs one `ssh` child and its two reader threads.

use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::time::Instant;

use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyObject, Sel};
use objc2::{
    define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    NSApplicationActivationPolicy, NSApplicationDidChangeScreenParametersNotification,
    NSApplicationWillTerminateNotification, NSBitmapFormat, NSBitmapImageRep, NSColor,
    NSColorSpace, NSDeviceRGBColorSpace, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSGraphicsContext, NSImage, NSLineBreakMode, NSMenu,
    NSMenuItem, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSScreen, NSStatusBar,
    NSStatusItem, NSStringDrawing, NSTextAlignment, NSVariableStatusItemLength,
};
use objc2_foundation::{
    ns_string, NSArray, NSDictionary, NSDistributedNotificationCenter, NSNotification,
    NSNotificationCenter, NSNotificationName, NSObject, NSPoint, NSRect, NSRunLoop,
    NSRunLoopCommonModes, NSSize, NSString, NSTimer,
};

use crate::chart::{render, Appearance, Coverage, Geometry, Glyphs, Pixmap, Rgba, FRAME_POINTS};
use crate::connection::{Feed, Remote, SSH_PROGRAM};
use crate::interval::Interval;
use crate::reader::Reader;
use crate::text::{menu_titles, Origin, MENU_LINES, QUIT_TITLE};
use crate::view::{KernelInput, RemoteInput, View};

/// The exit status of a start off the main thread.
const EXIT_NOT_MAIN_THREAD: u8 = 1;

/// The text that `glance` prints when it starts off the main thread.
const NOT_MAIN_THREAD: &str = "glance: the status item must start on the main thread";

/// The share of the interval that the timer may drift. The slack lets macOS
/// merge the wakeups of the timer with other wakeups.
const TIMER_TOLERANCE: f64 = 0.1;

/// Starts a repeating timer on a run loop in the common modes.
///
/// The function makes the timer, adds it to the run loop, gives it its share
/// of tolerance, and returns it. The run loop keeps the timer, and the timer
/// keeps its target alive.
///
/// An open menu runs the main run loop in the event tracking mode, which
/// AppKit lists among the common modes. A timer in the default mode only
/// would stop while the menu is open.
///
/// # Safety
///
/// The `target` must answer `selector` with one `NSTimer` argument.
unsafe fn start_repeating_timer(
    run_loop: &NSRunLoop,
    seconds: f64,
    target: &AnyObject,
    selector: Sel,
) -> Retained<NSTimer> {
    // SAFETY: The caller promises that the target answers the selector with
    // one timer argument. No user info is given.
    let timer = unsafe {
        NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            seconds, target, selector, None, true,
        )
    };
    // SAFETY: The mode is a constant of Foundation that exists at run time.
    unsafe { run_loop.addTimer_forMode(&timer, NSRunLoopCommonModes) };
    timer.setTolerance(seconds * TIMER_TOLERANCE);
    timer
}

/// The name of the notification that the system posts when the appearance
/// changes between light and dark.
const THEME_CHANGED_NOTIFICATION: &str = "AppleInterfaceThemeChangedNotification";

/// The key of the quit item. A lower case letter means command plus the letter.
const QUIT_KEY: &str = "q";

/// The scale that the chart takes when the system lists no screen.
const FALLBACK_SCALE: u32 = 2;

/// The bits in one sample of one color channel.
const BITS_PER_SAMPLE: isize = 8;

/// The number of channels in one pixel: red, green, blue, and alpha.
const SAMPLES_PER_PIXEL: isize = 4;

/// The bytes in one pixel.
const BYTES_PER_PIXEL: usize = 4;

/// The bits in one pixel.
const BITS_PER_PIXEL: isize = 32;

/// The size of the system font of the label at the top of the charts, in points.
const LABEL_FONT_POINTS: f64 = 7.0;

/// Where the samples come from.
#[derive(Debug)]
enum Source {
    /// The kernel of this Mac.
    Kernel {
        /// The reader of the kernel.
        reader: Reader,
        /// The rules that turn each reading into a sample.
        input: KernelInput,
    },
    /// A second Mac, through one `ssh` connection at a time.
    Remote {
        /// The connection that starts again after each end.
        feed: Feed,
        /// The rules that apply each event, and name the host.
        input: RemoteInput,
    },
}

/// What changes from one sample to the next.
#[derive(Debug)]
struct State {
    /// Where the samples come from.
    source: Source,
    /// The samples that the charts show, and the last error. The samples stay
    /// across a new connection.
    view: View,
}

impl State {
    /// A state with an empty view that holds `capacity` samples.
    fn new(source: Source, capacity: NonZeroUsize) -> State {
        State {
            source,
            view: View::new(capacity),
        }
    }

    /// Gets the samples since the last tick, and gives them to the rules.
    fn update(&mut self) {
        match &mut self.source {
            Source::Kernel { reader, input } => input.apply(reader.read(), &mut self.view),
            Source::Remote { feed, input } => {
                for event in feed.poll(Instant::now()) {
                    input.apply(event, &mut self.view);
                }
            }
        }
    }

    /// The Mac that the values come from, for the menu and the label.
    fn origin(&self) -> Origin<'_> {
        match &self.source {
            Source::Kernel { reader, .. } => Origin {
                host: reader.host_name().as_str(),
                destination: None,
            },
            Source::Remote { input, .. } => input.origin(),
        }
    }

    /// Stops the `ssh` child of remote mode. Local mode has nothing to stop.
    fn stop(&mut self) {
        if let Source::Remote { feed, .. } = &mut self.source {
            feed.stop();
        }
    }
}

/// The values that a `Monitor` holds.
#[derive(Debug)]
struct Ivars {
    /// The time between two samples.
    interval: Interval,
    /// The status item in the menu bar.
    status_item: Retained<NSStatusItem>,
    /// The five disabled info items of the menu.
    info_items: [Retained<NSMenuItem>; MENU_LINES],
    /// The values that change.
    state: RefCell<State>,
}

define_class!(
    /// The object that owns the timer target and the notification target.
    // SAFETY:
    // - `NSObject` has no subclassing requirement.
    // - `Monitor` does not implement `Drop`.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct Monitor;

    impl Monitor {
        /// Gets the new samples, updates the history, and redraws.
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            autoreleasepool(|_| self.sample_and_redraw());
        }

        /// Redraws after a change of the appearance or of the displays.
        #[unsafe(method(redraw:))]
        fn redraw_on_notification(&self, _notification: &NSNotification) {
            autoreleasepool(|_| self.redraw());
        }

        /// Stops the `ssh` child before the application stops.
        #[unsafe(method(stop:))]
        fn stop_on_notification(&self, _notification: &NSNotification) {
            self.ivars().state.borrow_mut().stop();
        }
    }
);

impl Monitor {
    /// Makes a monitor that holds `state`.
    fn new(
        mtm: MainThreadMarker,
        interval: Interval,
        state: State,
        status_item: Retained<NSStatusItem>,
        info_items: [Retained<NSMenuItem>; MENU_LINES],
    ) -> Retained<Monitor> {
        let this = Monitor::alloc(mtm).set_ivars(Ivars {
            interval,
            status_item,
            info_items,
            state: RefCell::new(state),
        });
        // SAFETY: The signature of the `init` method of `NSObject` is correct.
        unsafe { msg_send![super(this), init] }
    }

    /// Gets the new samples once, keeps them, and redraws.
    fn sample_and_redraw(&self) {
        self.ivars().state.borrow_mut().update();
        self.redraw();
    }

    /// Starts the repeating timer on the main run loop, in the common modes,
    /// so that it keeps firing while the menu is open.
    ///
    /// The timer keeps its target alive, and the caller keeps the monitor
    /// alive for the whole run.
    fn start_timer(&self) {
        // SAFETY: The target is this monitor, which answers `tick:` with one
        // timer argument.
        unsafe {
            start_repeating_timer(
                &NSRunLoop::mainRunLoop(),
                self.ivars().interval.seconds(),
                self,
                sel!(tick:),
            )
        };
    }

    /// Draws the charts into the status item and writes the menu lines.
    fn redraw(&self) {
        self.draw_charts();
        self.update_menu();
    }

    /// Draws the charts. The old image stays when AppKit refuses a step.
    fn draw_charts(&self) {
        let mtm = self.mtm();
        let Some(button) = self.ivars().status_item.button(mtm) else {
            return;
        };
        let appearance = {
            let names = NSArray::from_slice(&[
                // SAFETY: These are constants of AppKit that exist at run time.
                unsafe { NSAppearanceNameAqua },
                // SAFETY: As above.
                unsafe { NSAppearanceNameDarkAqua },
            ]);
            let best = button
                .effectiveAppearance()
                .bestMatchFromAppearancesWithNames(&names);
            // SAFETY: The constant of AppKit exists at run time.
            if best.is_some_and(|name| &*name == unsafe { NSAppearanceNameDarkAqua }) {
                Appearance::Dark
            } else {
                Appearance::Light
            }
        };
        let thickness = NSStatusBar::systemStatusBar().thickness();
        let mut scales: Vec<u32> = NSScreen::screens(mtm)
            .iter()
            .map(|screen| {
                Geometry::from_status_bar(thickness, screen.backingScaleFactor(), None).scale()
            })
            .collect();
        if scales.is_empty() {
            scales.push(FALLBACK_SCALE);
        }
        scales.sort_unstable();
        scales.dedup();

        let line_height = label_line_height();
        let state = self.ivars().state.borrow();
        let mut drawings: Vec<(Geometry, Pixmap)> = scales
            .iter()
            .map(|&scale| {
                let geometry =
                    Geometry::from_status_bar(thickness, f64::from(scale), Some(line_height));
                (geometry, render(state.view.history(), geometry, appearance))
            })
            .collect();
        let Some((_, first)) = drawings.first() else {
            return;
        };
        let points = points_of(first);
        let label = state.origin().host;
        let image = NSImage::initWithSize(NSImage::alloc(), points);
        for (geometry, pixmap) in &mut drawings {
            if let Some(rect) = label_rect(*geometry) {
                // A refused step leaves the charts of this scale with no label.
                draw_label(pixmap, label, rect, appearance);
            }
            let Some(representation) = bitmap_representation(
                pixmap.width(),
                pixmap.height(),
                &pixmap.premultiplied_bytes(),
                points,
            ) else {
                return;
            };
            image.addRepresentation(&representation);
        }
        image.setTemplate(false);
        button.setImage(Some(&image));
    }

    /// Sets the five info lines of the menu to the titles of `menu_titles`.
    fn update_menu(&self) {
        let state = self.ivars().state.borrow();
        let items = &self.ivars().info_items;
        let titles = menu_titles(
            &state.origin(),
            state.view.error(),
            state.view.history().newest().as_ref(),
        );
        for (item, title) in items.iter().zip(titles) {
            item.setTitle(&NSString::from_str(&title));
        }
    }
}

/// Makes one bitmap representation of `width` by `height` pixels, tagged as
/// sRGB, that holds the bytes `premultiplied`.
///
/// The bytes are RGBA with the alpha premultiplied, row by row from the top
/// left, with no padding. AppKit makes a graphics context for text only for
/// such a bitmap. Returns `None` when AppKit refuses to make or to retag the
/// image.
fn bitmap_representation(
    width_pixels: u32,
    height_pixels: u32,
    premultiplied: &[u8],
    points: NSSize,
) -> Option<Retained<NSBitmapImageRep>> {
    let width = isize::try_from(width_pixels).ok()?;
    let height = isize::try_from(height_pixels).ok()?;
    let source_stride = usize::try_from(width_pixels).ok()? * BYTES_PER_PIXEL;
    let rows = usize::try_from(height_pixels).ok()?;
    // SAFETY: The planes pointer is null, so AppKit allocates the buffer. The
    // color space name is a constant of AppKit. The sizes describe one
    // non-planar buffer of 8-bit RGBA pixels.
    let representation = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            width,
            height,
            BITS_PER_SAMPLE,
            SAMPLES_PER_PIXEL,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::empty(),
            isize::try_from(source_stride).ok()?,
            BITS_PER_PIXEL,
        )
    }?;
    let destination_stride = usize::try_from(representation.bytesPerRow()).ok()?;
    let data = representation.bitmapData();
    if data.is_null() || destination_stride < source_stride {
        return None;
    }
    for (row, source) in premultiplied
        .chunks_exact(source_stride)
        .take(rows)
        .enumerate()
    {
        // SAFETY: AppKit allocated `destination_stride * rows` bytes at
        // `data`. The row index is below `rows`, and the source row is not
        // longer than the destination stride. The two buffers do not overlap
        // because AppKit allocated the destination.
        unsafe {
            std::ptr::copy_nonoverlapping(
                source.as_ptr(),
                data.add(row * destination_stride),
                source_stride,
            );
        }
    }
    let tagged =
        representation.bitmapImageRepByRetaggingWithColorSpace(&NSColorSpace::sRGBColorSpace())?;
    tagged.setSize(points);
    Some(tagged)
}

/// The alpha of each pixel of `representation`, row by row from the top left.
///
/// Returns `None` when the representation has no buffer, or when its layout
/// is not the layout that [`bitmap_representation`] makes: one plane of four
/// 8-bit samples for each pixel, with the alpha last.
fn alpha_channel(representation: &NSBitmapImageRep) -> Option<Vec<u8>> {
    if representation.isPlanar()
        || representation.samplesPerPixel() != SAMPLES_PER_PIXEL
        || representation.bitsPerPixel() != BITS_PER_PIXEL
        || representation
            .bitmapFormat()
            .contains(NSBitmapFormat::AlphaFirst)
    {
        return None;
    }
    let stride = usize::try_from(representation.bytesPerRow()).ok()?;
    let width = usize::try_from(representation.pixelsWide()).ok()?;
    let height = usize::try_from(representation.pixelsHigh()).ok()?;
    let row_bytes = width.checked_mul(BYTES_PER_PIXEL)?;
    let data = representation.bitmapData();
    if data.is_null() || row_bytes > stride {
        return None;
    }
    let mut alpha = Vec::with_capacity(width.checked_mul(height)?);
    for row in 0..height {
        // SAFETY: AppKit holds `stride * height` bytes at `data`. The row
        // index is below `height`, and the row reads `row_bytes` bytes, which
        // is not more than `stride`.
        let bytes = unsafe { std::slice::from_raw_parts(data.add(row * stride), row_bytes) };
        alpha.extend(
            bytes
                .as_chunks::<BYTES_PER_PIXEL>()
                .0
                .iter()
                .map(|&[_, _, _, value]| value),
        );
    }
    Some(alpha)
}

/// The size of `pixmap` in points.
fn points_of(pixmap: &Pixmap) -> NSSize {
    NSSize::new(
        f64::from(pixmap.width_points()),
        f64::from(pixmap.height_points()),
    )
}

/// `color` with its alpha at the maximum.
fn opaque([red, green, blue, _]: Rgba) -> Rgba {
    [red, green, blue, u8::MAX]
}

/// The share of each pixel of `pixmap` that the glyphs of `text` cover when
/// AppKit draws them into `rect` in `ink`.
///
/// AppKit draws the glyphs alone into a transparent bitmap of the size of
/// `pixmap`. The ink must be opaque, so the alpha of each pixel is the
/// coverage. Returns `None` when AppKit refuses a step.
fn coverage_in(pixmap: &Pixmap, text: &str, rect: NSRect, ink: Rgba) -> Option<Coverage> {
    let transparent = vec![0; pixmap.bytes().len()];
    let representation = bitmap_representation(
        pixmap.width(),
        pixmap.height(),
        &transparent,
        points_of(pixmap),
    )?;
    if !draw_text(&representation, text, rect, ink) {
        return None;
    }
    Coverage::new(
        pixmap.width(),
        pixmap.height(),
        alpha_channel(&representation)?,
    )
}

/// The glyphs of `text` in `rect` of `pixmap`, in the label color of each
/// appearance. Returns `None` when AppKit refuses a step.
fn label_glyphs(pixmap: &Pixmap, text: &str, rect: NSRect) -> Option<Glyphs> {
    let ink = |appearance: Appearance| opaque(appearance.label());
    Glyphs::new(
        coverage_in(pixmap, text, rect, ink(Appearance::Light))?,
        coverage_in(pixmap, text, rect, ink(Appearance::Dark))?,
    )
}

/// Paints the label `text` into `rect` of `pixmap`, in the colors of
/// `appearance`.
///
/// `rect` is in points, with its origin at the bottom left of the image. The
/// glyphs come from AppKit, and [`Pixmap::paint_label`] paints them. Returns
/// `false` when AppKit refuses a step. The pixmap then stays as it was.
fn draw_label(pixmap: &mut Pixmap, text: &str, rect: NSRect, appearance: Appearance) -> bool {
    label_glyphs(pixmap, text, rect)
        .is_some_and(|glyphs| pixmap.paint_label(&glyphs, appearance).is_ok())
}

/// The line height of the font of the label, in points.
///
/// The chart module rounds it up to whole points for the label rows.
fn label_line_height() -> f64 {
    let font = NSFont::systemFontOfSize(LABEL_FONT_POINTS);
    font.ascender() - font.descender() + font.leading()
}

/// Converts one channel of a color from 0 to 255 into the range 0 to 1.
fn unit_channel(value: u8) -> f64 {
    f64::from(value) / f64::from(u8::MAX)
}

/// The rectangle of the label, in points, with its origin at the bottom left
/// of the image. It is `None` when the geometry has no label.
///
/// The rectangle spans the image between the two outer frames, so the text
/// centers across the whole image and never paints over an outer frame. A
/// centered name crosses the shared border between the two charts.
fn label_rect(geometry: Geometry) -> Option<NSRect> {
    let rows = geometry.label_rows_points()?;
    Some(NSRect::new(
        NSPoint::new(
            f64::from(FRAME_POINTS),
            f64::from(geometry.image_height_points() - rows.end),
        ),
        NSSize::new(
            f64::from(geometry.image_width_points() - 2 * FRAME_POINTS),
            f64::from(rows.end - rows.start),
        ),
    ))
}

/// Draws `text` into `rect` of `representation`, in `color`.
///
/// `rect` is in points, with its origin at the bottom left of the image. The
/// text is in the system font at [`LABEL_FONT_POINTS`], centered, and
/// truncated at its end with an ellipsis, so it never makes the image wider.
/// Returns `false` when AppKit refuses to make a graphics context.
fn draw_text(representation: &NSBitmapImageRep, text: &str, rect: NSRect, color: Rgba) -> bool {
    let Some(context) = NSGraphicsContext::graphicsContextWithBitmapImageRep(representation) else {
        return false;
    };
    let font = NSFont::systemFontOfSize(LABEL_FONT_POINTS);
    let [red, green, blue, alpha] = color;
    let text_color = NSColor::colorWithSRGBRed_green_blue_alpha(
        unit_channel(red),
        unit_channel(green),
        unit_channel(blue),
        unit_channel(alpha),
    );
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(NSTextAlignment::Center);
    paragraph.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    // SAFETY: These are constants of AppKit that exist at run time.
    let keys = unsafe {
        [
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
        ]
    };
    let values: [&AnyObject; 3] = [&font, &text_color, &paragraph];
    let attributes = NSDictionary::from_slices(&keys, &values);
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    // SAFETY: Each key of `attributes` holds an object of the class that
    // AppKit expects for it: a font, a color, and a paragraph style.
    unsafe { NSString::from_str(text).drawInRect_withAttributes(rect, Some(&attributes)) };
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    true
}

/// Makes the menu and gives it to the status item. Returns the info items.
///
/// The first info line shows the host of `origin`.
fn build_menu(
    mtm: MainThreadMarker,
    status_item: &NSStatusItem,
    origin: &Origin<'_>,
) -> [Retained<NSMenuItem>; MENU_LINES] {
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let titles = menu_titles(origin, None, None);
    let info_items: [Retained<NSMenuItem>; MENU_LINES] = titles.map(|title| {
        // SAFETY: The selector is absent, which AppKit allows for an item
        // that does nothing.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(&title),
                None,
                ns_string!(""),
            )
        };
        item.setEnabled(false);
        menu.addItem(&item);
        item
    });
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    // SAFETY: `terminate:` is a method of `NSApplication`. The item has no
    // target, so the action goes up the responder chain to the application.
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(QUIT_TITLE),
            Some(sel!(terminate:)),
            &NSString::from_str(QUIT_KEY),
        )
    };
    menu.addItem(&quit);
    status_item.setMenu(Some(&menu));
    info_items
}

/// Asks for a redraw when the appearance or the displays change, and for a
/// stop of the connection when the application stops.
///
/// **Quit glance** sends `terminate:`, which posts the notification of the
/// stop and then ends the process. No destructor runs after that.
fn register_observers(monitor: &Monitor) {
    let observer: &AnyObject = monitor;
    let theme = NSNotificationName::from_str(THEME_CHANGED_NOTIFICATION);
    // SAFETY: `redraw:` and `stop:` each take one notification, and the
    // observer is alive for the whole run. No object filter is given.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            observer,
            sel!(stop:),
            Some(NSApplicationWillTerminateNotification),
            None,
        );
        NSDistributedNotificationCenter::defaultCenter().addObserver_selector_name_object(
            observer,
            sel!(redraw:),
            Some(&theme),
            None,
        );
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            observer,
            sel!(redraw:),
            Some(NSApplicationDidChangeScreenParametersNotification),
            None,
        );
    }
}

/// Starts the status item and runs until the user quits.
///
/// With no `remote`, the status item shows the kernel of this Mac. With a
/// `remote`, it shows the second Mac through `ssh`.
///
/// The function does the first update before the run loop starts, so the
/// status item has its frames and its width from the start. In remote mode,
/// that update starts the first connection.
///
/// The status item gets `autosave_name` before its first update. AppKit
/// keeps the position of the item under that name, and a menu bar manager
/// keys the item on it (see [`crate::instance::Target::autosave_name`]).
///
/// The function calls `on_ready` once, when the status item is up and just
/// before the run loop starts.
#[must_use]
pub fn run(
    interval: Interval,
    remote: Option<Remote>,
    autosave_name: &str,
    on_ready: impl FnOnce(),
) -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("{NOT_MAIN_THREAD}");
        return ExitCode::from(EXIT_NOT_MAIN_THREAD);
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let status_item =
        NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    status_item.setAutosaveName(Some(&NSString::from_str(autosave_name)));
    let source = match remote {
        None => Source::Kernel {
            reader: Reader::new(),
            input: KernelInput::new(),
        },
        Some(remote) => Source::Remote {
            input: RemoteInput::new(remote.destination.as_str().to_owned(), remote.naming),
            feed: Feed::new(SSH_PROGRAM, remote, interval),
        },
    };
    // The history holds one sample for each column of a chart.
    let capacity =
        Geometry::from_status_bar(NSStatusBar::systemStatusBar().thickness(), 1.0, None).columns();
    let state = State::new(source, capacity);
    let info_items = build_menu(mtm, &status_item, &state.origin());
    let monitor = Monitor::new(mtm, interval, state, status_item, info_items);
    monitor.sample_and_redraw();
    monitor.start_timer();
    register_observers(&monitor);
    on_ready();
    app.run();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass};
    use objc2_app_kit::{NSEventTrackingRunLoopMode, NSFont, NSFontAttributeName, NSStringDrawing};
    use objc2_core_foundation::{CFRunLoop, CFString};
    use objc2_foundation::{NSDate, NSDictionary, NSObject, NSRunLoop, NSString, NSTimer};

    use super::{
        draw_label, label_line_height, label_rect, start_repeating_timer, LABEL_FONT_POINTS,
        TIMER_TOLERANCE,
    };
    use crate::chart::{render, Appearance, Geometry, Pixmap, Rgba, FRAME_POINTS, SYSTEM};
    use crate::cpu::{CpuShare, Ticks};
    use crate::history::History;
    use crate::pressure::Pressure;
    use crate::sample::Sample;

    /// The thickness of the status bar in the label test, in points.
    const THICKNESS: f64 = 22.0;

    /// The scale of the label test. Each point is 2 pixels high.
    const LABEL_SCALE: u32 = 2;

    /// A host name for the label test. It is the longest name of the Macs of
    /// the author, and it must fit the label with no ellipsis.
    const LABEL: &str = "M4-Pro-Mac-Mini";

    /// Tells if any pixel of row `y` differs between the two images.
    fn row_changed(before: &Pixmap, after: &Pixmap, y: u32) -> bool {
        (0..after.width()).any(|x| before.pixel(x, y) != after.pixel(x, y))
    }

    #[test]
    fn the_label_paints_the_top_of_the_interior_and_nothing_in_the_bottom_half() {
        let geometry =
            Geometry::from_status_bar(THICKNESS, f64::from(LABEL_SCALE), Some(label_line_height()));
        let rows = geometry
            .label_rows_points()
            .expect("the status bar holds a label");
        let mut pixmap = render(
            &History::new(geometry.columns()),
            geometry,
            Appearance::Light,
        );
        let before = pixmap.clone();
        assert!(draw_label(
            &mut pixmap,
            LABEL,
            label_rect(geometry).expect("the status bar holds a label"),
            Appearance::Light
        ));

        let top = rows.start * LABEL_SCALE;
        let bottom = rows.end * LABEL_SCALE;
        let middle = (top + bottom) / 2;
        for y in pixmap.height() / 2..pixmap.height() {
            assert!(
                !row_changed(&before, &pixmap, y),
                "the label changed row {y}, in the bottom half"
            );
        }
        // A drawing in pixels, not in points, fills only one half of the rows.
        assert!(
            (top..middle).any(|y| row_changed(&before, &pixmap, y)),
            "the upper half of the label rows did not change"
        );
        assert!(
            (middle..bottom).any(|y| row_changed(&before, &pixmap, y)),
            "the lower half of the label rows did not change"
        );
    }

    /// Over a red column, black contrasts more than white, so the label is
    /// black in both appearances. Each pixel that it paints over the column
    /// thus gets darker in each channel.
    #[test]
    fn the_label_only_darkens_full_columns_in_each_appearance() {
        let geometry =
            Geometry::from_status_bar(THICKNESS, f64::from(LABEL_SCALE), Some(label_line_height()));
        let rows = geometry
            .label_rows_points()
            .expect("the status bar holds a label");
        let mut history = History::new(geometry.columns());
        for _ in 0..geometry.columns().get() {
            history.push(Sample {
                cpu: CpuShare::between(
                    Ticks::default(),
                    Ticks {
                        user: 0,
                        system: 100,
                        idle: 0,
                        nice: 0,
                    },
                ),
                pressure: Pressure::from_raw(1, 0),
            });
        }
        let columns = u32::try_from(geometry.columns().get()).unwrap();
        let columns = FRAME_POINTS * LABEL_SCALE..(FRAME_POINTS + columns) * LABEL_SCALE;
        let lines = rows.start * LABEL_SCALE..rows.end * LABEL_SCALE;
        let rgb = |[red, green, blue, _]: Rgba| [red, green, blue];

        for appearance in [Appearance::Light, Appearance::Dark] {
            let mut pixmap = render(&history, geometry, appearance);
            let before = pixmap.clone();
            assert!(draw_label(
                &mut pixmap,
                LABEL,
                label_rect(geometry).expect("the status bar holds a label"),
                appearance
            ));

            let mut painted = 0;
            for y in lines.clone() {
                for x in columns.clone() {
                    let old = before.pixel(x, y);
                    let new = pixmap.pixel(x, y);
                    assert_eq!(rgb(old), rgb(SYSTEM), "({x}, {y}) is not on a column");
                    if new != old {
                        painted += 1;
                        assert!(
                            rgb(new).iter().zip(rgb(old)).all(|(new, old)| *new <= old),
                            "{appearance:?}: the label made ({x}, {y}) lighter: {old:?} to {new:?}"
                        );
                    }
                }
            }
            assert!(
                painted > 0,
                "{appearance:?}: the label painted no pixel of the columns"
            );
        }
    }

    #[test]
    fn a_thin_bar_has_no_label_rect() {
        let geometry = Geometry::from_status_bar(10.0, 2.0, Some(label_line_height()));
        assert!(label_rect(geometry).is_none());
    }

    #[test]
    fn the_name_of_a_mac_fits_the_label_with_no_ellipsis() {
        let geometry =
            Geometry::from_status_bar(THICKNESS, f64::from(LABEL_SCALE), Some(label_line_height()));
        let rect = label_rect(geometry).expect("the status bar holds a label");
        let font = NSFont::systemFontOfSize(LABEL_FONT_POINTS);
        // SAFETY: This is a constant of AppKit that exists at run time.
        let key = unsafe { NSFontAttributeName };
        let value: &AnyObject = &font;
        let attributes = NSDictionary::from_slices(&[key], &[value]);
        // SAFETY: The font key holds a font, which is the class that AppKit
        // expects for it.
        let size = unsafe { NSString::from_str(LABEL).sizeWithAttributes(Some(&attributes)) };
        assert!(
            size.width <= rect.size.width,
            "{LABEL} is {} points wide, and the label is {} points wide",
            size.width,
            rect.size.width
        );
    }

    /// The interval of the probe timer, in seconds.
    const PROBE_INTERVAL: f64 = 0.02;

    /// How long one run of the loop lasts, in seconds. A timer fire does not
    /// end a run, so the test runs the loop in short slices and checks the
    /// count between them.
    const LOOP_SLICE: f64 = 0.1;

    /// How long the test waits for one fire, in seconds. The bound is wide
    /// because a full workspace run is often heavily loaded.
    const FIRE_DEADLINE: f64 = 5.0;

    define_class!(
        /// A timer target that counts its fires.
        // SAFETY:
        // - `NSObject` has no subclassing requirement.
        // - `Probe` does not implement `Drop`.
        #[unsafe(super = NSObject)]
        #[name = "GlanceTimerProbe"]
        #[ivars = Cell<u32>]
        struct Probe;

        impl Probe {
            #[unsafe(method(fire:))]
            fn fire(&self, _timer: &NSTimer) {
                let ivars = self.ivars();
                ivars.set(ivars.get() + 1);
            }
        }
    );

    impl Probe {
        /// Makes a probe with a count of zero.
        fn new() -> objc2::rc::Retained<Self> {
            let this = Self::alloc().set_ivars(Cell::new(0));
            // SAFETY: `NSObject` answers `init`.
            unsafe { msg_send![super(this), init] }
        }
    }

    #[test]
    fn a_repeating_timer_fires_while_a_menu_tracks_events() {
        // SAFETY: This is a constant of AppKit that exists at run time.
        let mode = unsafe { NSEventTrackingRunLoopMode };
        // The main run loop of an application lists the tracking mode among
        // its common modes, and the run loop of a test thread does not.
        CFRunLoop::current()
            .expect("the test thread has a run loop")
            .add_common_mode(Some(&CFString::from_str(&mode.to_string())));
        let probe = Probe::new();
        let run_loop = NSRunLoop::currentRunLoop();
        // SAFETY: The target answers `fire:` with one timer argument.
        let timer =
            unsafe { start_repeating_timer(&run_loop, PROBE_INTERVAL, &probe, sel!(fire:)) };
        let deadline = NSDate::dateWithTimeIntervalSinceNow(FIRE_DEADLINE);
        while probe.ivars().get() == 0 && deadline.timeIntervalSinceNow() > 0.0 {
            let slice = NSDate::dateWithTimeIntervalSinceNow(LOOP_SLICE);
            if !run_loop.runMode_beforeDate(mode, &slice) {
                std::thread::sleep(Duration::from_secs_f64(PROBE_INTERVAL));
            }
        }
        timer.invalidate();
        assert!(
            probe.ivars().get() > 0,
            "the timer did not fire in the mode {mode}"
        );
    }

    #[test]
    fn a_repeating_timer_takes_its_share_of_tolerance() {
        let probe = Probe::new();
        // SAFETY: The target answers `fire:` with one timer argument.
        let timer = unsafe {
            start_repeating_timer(
                &NSRunLoop::currentRunLoop(),
                PROBE_INTERVAL,
                &probe,
                sel!(fire:),
            )
        };
        let tolerance = timer.tolerance();
        timer.invalidate();
        assert!(
            (tolerance - PROBE_INTERVAL * TIMER_TOLERANCE).abs() < f64::EPSILON,
            "the tolerance of the timer is {tolerance}"
        );
    }
}
