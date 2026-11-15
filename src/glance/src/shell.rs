//! The AppKit shell: one status item in the menu bar that shows the two
//! charts.
//!
//! The shell is thin. Each rule of layout, color, text, and arithmetic lives
//! in a pure module. This module only reads the kernel on a timer, gives the
//! values to the pure modules, and hands the answers to AppKit.
//!
//! The shell starts no thread and no async runtime. One repeating timer on the
//! main run loop does all of the work, and the system merges its wakeups with
//! the wakeups of other programs.

use std::cell::RefCell;
use std::process::ExitCode;

use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyObject, Sel};
use objc2::{
    define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    NSApplicationActivationPolicy, NSApplicationDidChangeScreenParametersNotification,
    NSBitmapFormat, NSBitmapImageRep, NSColorSpace, NSDeviceRGBColorSpace, NSImage, NSMenu,
    NSMenuItem, NSScreen, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{
    ns_string, NSArray, NSDistributedNotificationCenter, NSNotification, NSNotificationCenter,
    NSNotificationName, NSObject, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer,
};

use crate::chart::{render, Appearance, Geometry, Pixmap};
use crate::history::{History, DEFAULT_CAPACITY};
use crate::interval::Interval;
use crate::reader::Reader;
use crate::sample::{Reading, Sample};
use crate::text::{menu_titles, QUIT_TITLE};

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

/// The number of info lines in the menu.
const INFO_LINES: usize = 4;

/// The bits in one sample of one color channel.
const BITS_PER_SAMPLE: isize = 8;

/// The number of channels in one pixel: red, green, blue, and alpha.
const SAMPLES_PER_PIXEL: isize = 4;

/// The bytes in one pixel.
const BYTES_PER_PIXEL: usize = 4;

/// The bits in one pixel.
const BITS_PER_PIXEL: isize = 32;

/// What changes from one sample to the next.
#[derive(Debug)]
struct State {
    /// The last good reading, or `None` before the first one.
    previous: Option<Reading>,
    /// The samples that the charts show.
    history: History<Sample>,
    /// The text of the last read error, or `None` when the last read worked.
    error: Option<String>,
}

/// The values that a `Monitor` holds.
#[derive(Debug)]
struct Ivars {
    /// The reader of the kernel.
    reader: Reader,
    /// The time between two samples.
    interval: Interval,
    /// The status item in the menu bar.
    status_item: Retained<NSStatusItem>,
    /// The four disabled info items of the menu.
    info_items: [Retained<NSMenuItem>; INFO_LINES],
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
        /// Reads the kernel, updates the history, and redraws.
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            autoreleasepool(|_| self.sample_and_redraw());
        }

        /// Redraws after a change of the appearance or of the displays.
        #[unsafe(method(redraw:))]
        fn redraw_on_notification(&self, _notification: &NSNotification) {
            autoreleasepool(|_| self.redraw());
        }
    }
);

impl Monitor {
    /// Makes a monitor with an empty history.
    fn new(
        mtm: MainThreadMarker,
        interval: Interval,
        status_item: Retained<NSStatusItem>,
        info_items: [Retained<NSMenuItem>; INFO_LINES],
    ) -> Retained<Monitor> {
        let this = Monitor::alloc(mtm).set_ivars(Ivars {
            reader: Reader::new(),
            interval,
            status_item,
            info_items,
            state: RefCell::new(State {
                previous: None,
                history: History::new(DEFAULT_CAPACITY),
                error: None,
            }),
        });
        // SAFETY: The signature of the `init` method of `NSObject` is correct.
        unsafe { msg_send![super(this), init] }
    }

    /// Reads the kernel once, keeps the result, and redraws.
    fn sample_and_redraw(&self) {
        let outcome = self.ivars().reader.read();
        {
            let mut state = self.ivars().state.borrow_mut();
            match outcome {
                Ok(reading) => {
                    if let Some(previous) = state.previous {
                        state.history.push(Sample::between(&previous, &reading));
                    }
                    state.previous = Some(reading);
                    state.error = None;
                }
                Err(error) => state.error = Some(error.to_string()),
            }
        }
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
            .map(|screen| Geometry::from_status_bar(thickness, screen.backingScaleFactor()).scale())
            .collect();
        if scales.is_empty() {
            scales.push(FALLBACK_SCALE);
        }
        scales.sort_unstable();
        scales.dedup();

        let state = self.ivars().state.borrow();
        let pixmaps: Vec<Pixmap> = scales
            .iter()
            .map(|&scale| {
                let geometry = Geometry::from_status_bar(thickness, f64::from(scale));
                render(&state.history, geometry, appearance)
            })
            .collect();
        let Some(first) = pixmaps.first() else {
            return;
        };
        let points = NSSize::new(
            f64::from(first.width_points()),
            f64::from(first.height_points()),
        );
        let image = NSImage::initWithSize(NSImage::alloc(), points);
        for pixmap in &pixmaps {
            let Some(representation) = bitmap_representation(pixmap, points) else {
                return;
            };
            image.addRepresentation(&representation);
        }
        image.setTemplate(false);
        button.setImage(Some(&image));
    }

    /// Sets the four info lines of the menu to the titles of `menu_titles`.
    fn update_menu(&self) {
        let state = self.ivars().state.borrow();
        let items = &self.ivars().info_items;
        let titles = menu_titles(state.error.as_deref(), state.history.newest().as_ref());
        for (item, title) in items.iter().zip(titles) {
            item.setTitle(&NSString::from_str(&title));
        }
    }
}

/// Makes one bitmap representation of a pixmap, tagged as sRGB.
///
/// Returns `None` when AppKit refuses to make or to retag the image.
fn bitmap_representation(pixmap: &Pixmap, points: NSSize) -> Option<Retained<NSBitmapImageRep>> {
    let width = isize::try_from(pixmap.width()).ok()?;
    let height = isize::try_from(pixmap.height()).ok()?;
    let source_stride = usize::try_from(pixmap.width()).ok()? * BYTES_PER_PIXEL;
    let rows = usize::try_from(pixmap.height()).ok()?;
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
            NSBitmapFormat::AlphaNonpremultiplied,
            isize::try_from(source_stride).ok()?,
            BITS_PER_PIXEL,
        )
    }?;
    let destination_stride = usize::try_from(representation.bytesPerRow()).ok()?;
    let data = representation.bitmapData();
    if data.is_null() || destination_stride < source_stride {
        return None;
    }
    for (row, source) in pixmap
        .bytes()
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

/// Makes the menu and gives it to the status item. Returns the info items.
fn build_menu(
    mtm: MainThreadMarker,
    status_item: &NSStatusItem,
) -> [Retained<NSMenuItem>; INFO_LINES] {
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let titles = menu_titles(None, None);
    let info_items: [Retained<NSMenuItem>; INFO_LINES] = std::array::from_fn(|index| {
        let title = titles[index].as_str();
        // SAFETY: The selector is absent, which AppKit allows for an item
        // that does nothing.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
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

/// Asks for a redraw when the appearance or the displays change.
fn register_observers(monitor: &Monitor) {
    let observer: &AnyObject = monitor;
    let theme = NSNotificationName::from_str(THEME_CHANGED_NOTIFICATION);
    // SAFETY: `redraw:` takes one notification, and the observer is alive for
    // the whole run. No object filter is given.
    unsafe {
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
/// The function takes the first reading before the run loop starts, so the
/// status item has its frames and its width from the start.
#[must_use]
pub fn run(interval: Interval) -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("{NOT_MAIN_THREAD}");
        return ExitCode::from(EXIT_NOT_MAIN_THREAD);
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let status_item =
        NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    let info_items = build_menu(mtm, &status_item);
    let monitor = Monitor::new(mtm, interval, status_item, info_items);
    monitor.sample_and_redraw();
    monitor.start_timer();
    register_observers(&monitor);
    app.run();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass};
    use objc2_app_kit::NSEventTrackingRunLoopMode;
    use objc2_core_foundation::{CFRunLoop, CFString};
    use objc2_foundation::{NSDate, NSObject, NSRunLoop, NSTimer};

    use super::{start_repeating_timer, TIMER_TOLERANCE};

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
