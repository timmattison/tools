//! The `AppKit` shell: one status item in the menu bar, with the mini-map of
//! the sessions and a menu, a global hotkey, and a floating panel that shows
//! the cctop screen.
//!
//! The shell is thin. Each rule lives in a pure module of this crate or in
//! the `cctop` library: the mini-map, the pixels, the menu entries, the
//! controller. This module gives `AppKit` the answers.
//!
//! Events reach the shell through one pipe. The watcher of the state
//! directory and the jobs call [`post`], which sends the event to the main
//! queue. [`deliver`] runs there and hands the event to the shell. A repeating
//! timer of 1 s sends the tick. No timer polls for events. The hotkey posts
//! its own event, which opens or closes the panel.

mod front_window;
mod hotkey;
mod panel_view;

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use cctop::controller::{AppEvent, Controller, Effect};
use cctop::focus::{FocusTarget, FocusTools};
use cctop::jobs::{EventSink, Jobs, NotifyGate};
use cctop::liveness::SystemClock;
use cctop::minimap::{Minimap, TILE_LIMIT};
use cctop::paths::ClaudeDirs;
use cctop::runner::SystemRunner;
use cctop::state::EpochMillis;
use cctop::sync::Sync;
use cctop::wezterm::DeviceWriter;
use dispatch2::DispatchQueue;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{
    define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    NSApplicationActivationPolicy, NSApplicationDidChangeScreenParametersNotification,
    NSBitmapFormat, NSBitmapImageRep, NSColorSpace, NSControlStateValueOff, NSControlStateValueOn,
    NSDeviceRGBColorSpace, NSEvent, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSScreen,
    NSStatusBar, NSStatusItem, NSVariableStatusItemLength, NSWindowDelegate,
};
use objc2_foundation::{
    NSArray, NSDistributedNotificationCenter, NSNotification, NSNotificationCenter,
    NSNotificationName, NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes, NSSize,
    NSString, NSTimer,
};
use ratatui::Terminal;

use self::front_window::front_window;
use self::hotkey::hotkey_of;
use self::panel_view::{make_panel, GridPanel, GridView, SharedTerminal};
use crate::cellgrid::CellGrid;
use crate::chord::Chord;
use crate::menu::{self, MenuEntry, MenuInput};
use crate::painter::{self, Appearance, Pixmap};
use crate::panel::{Panel, PanelInput, PanelJob};
use crate::placement::{self, Placement, PlacementInput, Point, Rect, Screen};
use crate::settings::Settings;

/// The exit status of a start off the main thread.
const EXIT_NOT_MAIN_THREAD: u8 = 1;

/// The exit status of a failed start of the state directory, the watcher or the controller.
const EXIT_START_FAILED: u8 = 1;

/// The text that `cctop-bar` prints when it starts off the main thread.
const NOT_MAIN_THREAD: &str = "cctop-bar: the status item must start on the main thread";

/// The time between two ticks, in seconds.
const TICK_SECONDS: f64 = 1.0;

/// The share of the tick that the timer may drift. The slack lets macOS merge
/// the wakeups of the timer with other wakeups.
const TIMER_TOLERANCE: f64 = 0.1;

/// The name of the notification that the system posts when the appearance
/// changes between light and dark.
const THEME_CHANGED_NOTIFICATION: &str = "AppleInterfaceThemeChangedNotification";

/// The key of the quit item. A lower case letter means command plus the letter.
const QUIT_KEY: &str = "q";

/// The scale that the item takes when the system lists no screen.
const FALLBACK_SCALE: u32 = 2;

/// The bits in one sample of one color channel.
const BITS_PER_SAMPLE: isize = 8;

/// The number of channels in one pixel: red, green, blue, and alpha.
const SAMPLES_PER_PIXEL: isize = 4;

/// The bytes in one pixel.
const BYTES_PER_PIXEL: usize = 4;

/// The bits in one pixel.
const BITS_PER_PIXEL: isize = 32;

/// The columns of the terminal before the first show. The first show resizes it.
const INITIAL_COLUMNS: u16 = placement::MIN_COLUMNS;

/// The rows of the terminal before the first show. The first show resizes it.
const INITIAL_ROWS: u16 = placement::MIN_ROWS;

/// An event that the main queue delivers to the shell.
enum ShellEvent {
    /// An event of the cctop app: the watcher, a job, or the tick.
    App(AppEvent),
    /// The person pressed the global hotkey.
    Hotkey,
}

/// What decides the image of the status item. The item draws again only when
/// this value changes.
type PaintKey = (Minimap, Appearance, u32, u32);

/// What changes while the program runs.
struct State {
    /// The board and the rules that keep it in step with the system.
    controller: Controller,
    /// The text of the last failed focus or notification.
    last_error: Option<String>,
    /// What the image of the status item shows now.
    painted: Option<PaintKey>,
    /// When the panel shows and hides.
    panel: Panel,
}

/// The values that a `Bar` holds.
struct Ivars {
    /// The status item in the menu bar.
    status_item: Retained<NSStatusItem>,
    /// The jobs that can block: a focus and a notification.
    jobs: Jobs,
    /// The values that change.
    state: RefCell<State>,
    /// The targets of the session items of the open menu. The tag of an item
    /// is the index in this list.
    targets: RefCell<Vec<FocusTarget>>,
    /// The floating panel. It is hidden until the person shows it.
    window: Retained<GridPanel>,
    /// The view of the panel.
    view: Retained<GridView>,
    /// The cells that the cctop screen draws into.
    terminal: SharedTerminal,
    /// The chord that is registered, or `None` when no hotkey works.
    registered: Option<Chord>,
    /// The text of the failed registration of the hotkey.
    hotkey_error: Option<String>,
}

define_class!(
    /// The object that owns the timer target, the notification target, the
    /// menu actions, and the delegate of the menu.
    // SAFETY:
    // - `NSObject` has no subclassing requirement.
    // - `Bar` does not implement `Drop`.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct Bar;

    impl Bar {
        /// Handles the tick of the timer.
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            autoreleasepool(|_| self.handle(AppEvent::Tick));
        }

        /// Draws again after a change of the appearance or of the displays.
        #[unsafe(method(redraw:))]
        fn redraw_on_notification(&self, _notification: &NSNotification) {
            autoreleasepool(|_| self.refresh());
        }

        /// Focuses the session of the item.
        #[unsafe(method(focusSession:))]
        fn focus_session(&self, item: &NSMenuItem) {
            let target = usize::try_from(item.tag())
                .ok()
                .and_then(|index| self.ivars().targets.borrow().get(index).cloned());
            if let Some(target) = target {
                self.ivars().jobs.spawn_focus(target);
            }
        }

        /// Shows the panel, from the menu item.
        #[unsafe(method(showPanel:))]
        fn show_panel(&self, _item: &NSMenuItem) {
            autoreleasepool(|_| self.show_request(false));
        }

        /// Turns the notifications on or off.
        #[unsafe(method(toggleNotifications:))]
        fn toggle_notifications(&self, _item: &NSMenuItem) {
            let mut state = self.ivars().state.borrow_mut();
            let on = !state.controller.notify();
            state.controller.set_notify(on);
        }
    }

    unsafe impl NSObjectProtocol for Bar {}

    unsafe impl NSWindowDelegate for Bar {
        /// Hides the panel when the person clicks outside it.
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            autoreleasepool(|_| self.input(PanelInput::ResignKey));
        }
    }

    unsafe impl NSMenuDelegate for Bar {
        /// Builds the items of the menu when it opens.
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            autoreleasepool(|_| self.rebuild_menu(menu));
        }
    }
);

impl Bar {
    /// Makes the object that holds `ivars`.
    fn new(mtm: MainThreadMarker, ivars: Ivars) -> Retained<Bar> {
        let this = Bar::alloc(mtm).set_ivars(ivars);
        // SAFETY: The signature of the `init` method of `NSObject` is correct.
        unsafe { msg_send![super(this), init] }
    }

    /// Handles one event of the shell on the main thread.
    fn handle_shell(&self, event: ShellEvent) {
        match event {
            ShellEvent::App(event) => self.handle(event),
            ShellEvent::Hotkey => self.show_request(true),
        }
    }

    /// Handles one event on the main thread: updates the last error, gives
    /// the event to the controller, runs its effects, and draws again.
    fn handle(&self, event: AppEvent) {
        let effects = {
            let mut state = self.ivars().state.borrow_mut();
            match &event {
                AppEvent::FocusDone(report) => {
                    state.last_error = (!report.succeeded()).then(|| report.status_line());
                }
                AppEvent::NotifyFailed(text) => {
                    state.last_error = Some(format!("The notification failed: {text}"));
                }
                _ => {}
            }
            state.controller.handle(event, EpochMillis::now())
        };
        for effect in effects {
            match effect {
                Effect::Focus(target) => self.ivars().jobs.spawn_focus(target),
                Effect::Notify(message) => self.ivars().jobs.spawn_notify(message),
                // Only the keys of the panel make this effect, and the panel
                // turns it into a hide. Only the menu item quits the program.
                Effect::Quit => {}
            }
        }
        self.redraw_panel();
        self.refresh();
    }

    /// Gives `input` to the panel and runs the jobs that it returns.
    fn input(&self, input: PanelInput) {
        let jobs = {
            let mut state = self.ivars().state.borrow_mut();
            let State {
                controller, panel, ..
            } = &mut *state;
            panel.handle(input, controller, EpochMillis::now())
        };
        self.run_jobs(jobs, None);
        self.redraw_panel();
    }

    /// Shows the panel, or hides it when `hotkey` is true and it is shown.
    ///
    /// The placement comes first: the front window and the screens give the
    /// size of the panel in rows. When no screen fits, nothing happens.
    fn show_request(&self, hotkey: bool) {
        let mtm = self.mtm();
        let screens: Vec<Screen> = NSScreen::screens(mtm)
            .iter()
            .map(|screen| Screen {
                frame: rect_of(screen.frame()),
                visible: rect_of(screen.visibleFrame()),
            })
            .collect();
        let mouse = NSEvent::mouseLocation();
        let Some(placed) = placement::place(&PlacementInput {
            front_window: front_window(mtm),
            screens: &screens,
            mouse: Point {
                x: mouse.x,
                y: mouse.y,
            },
            cell: self.ivars().view.cell(),
        }) else {
            return;
        };
        let rows = placed.rows;
        let input = if hotkey {
            PanelInput::Hotkey { rows }
        } else {
            PanelInput::ShowItem { rows }
        };
        let jobs = {
            let mut state = self.ivars().state.borrow_mut();
            let State {
                controller, panel, ..
            } = &mut *state;
            panel.handle(input, controller, EpochMillis::now())
        };
        self.run_jobs(jobs, Some(placed));
        self.redraw_panel();
    }

    /// Runs the jobs of the panel. A show needs `placed`.
    fn run_jobs(&self, jobs: Vec<PanelJob>, placed: Option<Placement>) {
        let ivars = self.ivars();
        for job in jobs {
            match job {
                PanelJob::Show => {
                    if let Some(placed) = placed {
                        ivars
                            .terminal
                            .borrow_mut()
                            .backend_mut()
                            .resize(placed.columns, placed.rows);
                        ivars.window.setFrame_display(frame_of(placed.frame), false);
                    }
                    self.draw_screen();
                    ivars.window.makeKeyAndOrderFront(None);
                    ivars.window.makeFirstResponder(Some(&ivars.view));
                }
                PanelJob::Hide => ivars.window.orderOut(None),
                PanelJob::Focus(target) => ivars.jobs.spawn_focus(target),
                PanelJob::Notify(message) => ivars.jobs.spawn_notify(message),
            }
        }
    }

    /// Draws the cctop screen again, when the panel is shown.
    fn redraw_panel(&self) {
        if self.ivars().state.borrow().panel.is_visible() {
            self.draw_screen();
        }
    }

    /// Draws the cctop screen into the cells and asks the view to paint them.
    fn draw_screen(&self) {
        let ivars = self.ivars();
        let now = EpochMillis::now();
        {
            let state = ivars.state.borrow();
            let app = state.controller.app();
            // The backend of the grid does not fail.
            let _ = ivars
                .terminal
                .borrow_mut()
                .draw(|frame| cctop::ui::draw(frame, app, now));
        }
        ivars.view.setNeedsDisplay(true);
    }

    /// Draws the mini-map into the status item, when its image changed.
    /// The old image stays when `AppKit` refuses a step.
    fn refresh(&self) {
        let mtm = self.mtm();
        let Some(button) = self.ivars().status_item.button(mtm) else {
            return;
        };
        let appearance = appearance_of(&button.effectiveAppearance());
        let scale = button
            .window()
            .map(|window| window.backingScaleFactor())
            .or_else(|| NSScreen::mainScreen(mtm).map(|screen| screen.backingScaleFactor()))
            .map_or(FALLBACK_SCALE, scale_of);
        let thickness = thickness_of(NSStatusBar::systemStatusBar().thickness());
        let pixmap = {
            let mut state = self.ivars().state.borrow_mut();
            let minimap = Minimap::of(state.controller.app().board(), TILE_LIMIT);
            let key = (minimap, appearance, scale, thickness);
            if state.painted.as_ref() == Some(&key) {
                return;
            }
            let pixmap = painter::paint(&key.0, thickness, scale, appearance);
            state.painted = Some(key);
            pixmap
        };
        if let Some(image) = image_of(&pixmap) {
            button.setImage(Some(&image));
        }
    }

    /// Replaces the items of `menu` with the entries of the menu module.
    fn rebuild_menu(&self, menu: &NSMenu) {
        let mtm = self.mtm();
        let (entries, targets) = {
            let state = self.ivars().state.borrow();
            let entries = menu::entries(&MenuInput {
                board: state.controller.app().board(),
                now: EpochMillis::now(),
                notify: state.controller.notify(),
                hotkey: self.ivars().registered,
                last_error: state
                    .last_error
                    .as_deref()
                    .or(self.ivars().hotkey_error.as_deref()),
            });
            let targets: Vec<FocusTarget> = entries
                .iter()
                .filter_map(|entry| match entry {
                    MenuEntry::Session { target, .. } => Some(target.clone()),
                    _ => None,
                })
                .collect();
            (entries, targets)
        };
        *self.ivars().targets.borrow_mut() = targets;
        menu.removeAllItems();
        let mut session_index: isize = 0;
        for entry in &entries {
            let item = match entry {
                MenuEntry::Separator => NSMenuItem::separatorItem(mtm),
                MenuEntry::Counts(text) | MenuEntry::Error(text) => disabled_item(mtm, text),
                MenuEntry::Show(text) => {
                    let item = action_item(mtm, text, sel!(showPanel:), "");
                    set_target(&item, self);
                    item
                }
                MenuEntry::Session { title, .. } => {
                    let item = action_item(mtm, title, sel!(focusSession:), "");
                    item.setTag(session_index);
                    session_index += 1;
                    set_target(&item, self);
                    item
                }
                MenuEntry::Notifications(on) => {
                    let item = action_item(
                        mtm,
                        menu::NOTIFICATIONS_TITLE,
                        sel!(toggleNotifications:),
                        "",
                    );
                    item.setState(if *on {
                        NSControlStateValueOn
                    } else {
                        NSControlStateValueOff
                    });
                    set_target(&item, self);
                    item
                }
                // SAFETY of the action: `terminate:` is a method of
                // `NSApplication`. The item has no target, so the action goes
                // up the responder chain to the application.
                MenuEntry::Quit => action_item(mtm, menu::QUIT_TITLE, sel!(terminate:), QUIT_KEY),
            };
            menu.addItem(&item);
        }
    }
}

/// The rectangle of `rect` as the placement module reads it.
fn rect_of(rect: objc2_foundation::NSRect) -> Rect {
    Rect {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    }
}

/// The rectangle of `rect` as `AppKit` reads it.
fn frame_of(rect: Rect) -> objc2_foundation::NSRect {
    objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(rect.x, rect.y),
        NSSize::new(rect.width, rect.height),
    )
}

/// Hands a key of the panel to the shell. It runs on the main thread.
fn panel_input(input: PanelInput) {
    let bar = BAR.with(|cell| cell.borrow().clone());
    if let Some(bar) = bar {
        autoreleasepool(|_| bar.input(input));
    }
}

/// Registers the hotkey of `chord`. Returns the manager, which must stay
/// alive for the life of the program, or the text of the failure.
///
/// The handler posts a [`ShellEvent::Hotkey`] for each press of this hotkey.
fn register_hotkey(chord: Chord) -> Result<GlobalHotKeyManager, String> {
    let hotkey = hotkey_of(chord).ok_or_else(|| "the key has no counterpart".to_string())?;
    let manager = GlobalHotKeyManager::new().map_err(|error| error.to_string())?;
    let id = hotkey.id();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == id && event.state == HotKeyState::Pressed {
            post(ShellEvent::Hotkey);
        }
    }));
    manager
        .register(hotkey)
        .map_err(|error| error.to_string())?;
    Ok(manager)
}

/// Makes `target` answer the action of `item`. The menu keeps no strong
/// reference, and the thread-local of the shell keeps the target alive.
fn set_target(item: &NSMenuItem, target: &Bar) {
    // SAFETY: The target lives for the whole run, and the actions of the
    // items that call this function are methods of `Bar`.
    unsafe { item.setTarget(Some(target)) };
}

/// Makes a disabled item with `title`.
fn disabled_item(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    // SAFETY: The selector is absent, which AppKit allows for an item that
    // does nothing.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            None,
            &NSString::from_str(""),
        )
    };
    item.setEnabled(false);
    item
}

/// Makes an enabled item with `title` that sends `action`.
fn action_item(mtm: MainThreadMarker, title: &str, action: Sel, key: &str) -> Retained<NSMenuItem> {
    // SAFETY: The caller names an action that its target answers with one
    // item argument, or a method of `NSApplication` for an item with no target.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    };
    item.setEnabled(true);
    item
}

/// The appearance of the menu bar, from the appearance of its button.
fn appearance_of(appearance: &objc2_app_kit::NSAppearance) -> Appearance {
    let names = NSArray::from_slice(&[
        // SAFETY: These are constants of AppKit that exist at run time.
        unsafe { NSAppearanceNameAqua },
        // SAFETY: As above.
        unsafe { NSAppearanceNameDarkAqua },
    ]);
    let best = appearance.bestMatchFromAppearancesWithNames(&names);
    // SAFETY: The constant of AppKit exists at run time.
    if best.is_some_and(|name| &*name == unsafe { NSAppearanceNameDarkAqua }) {
        Appearance::Dark
    } else {
        Appearance::Light
    }
}

/// The backing scale as a whole number of at least 1.
fn scale_of(factor: f64) -> u32 {
    // The value is rounded and clamped, so the cast cannot lose a sign or wrap.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is rounded and at least 1"
    )]
    let scale = factor.round().max(1.0) as u32;
    scale
}

/// The thickness of the menu bar as a whole number of points.
fn thickness_of(points: f64) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is rounded and not below 0"
    )]
    let thickness = points.round().max(0.0) as u32;
    thickness
}

/// Makes the image of `pixmap`, with a size in points of its pixels divided
/// by its scale. Returns `None` when `AppKit` refuses to make the image.
fn image_of(pixmap: &Pixmap) -> Option<Retained<NSImage>> {
    let scale = f64::from(pixmap.scale().max(1));
    let points = NSSize::new(
        f64::from(pixmap.width()) / scale,
        f64::from(pixmap.height()) / scale,
    );
    let representation = bitmap_representation(pixmap, points)?;
    let image = NSImage::initWithSize(NSImage::alloc(), points);
    image.addRepresentation(&representation);
    image.setTemplate(false);
    Some(image)
}

/// Makes a bitmap representation of `pixmap`, tagged as sRGB, with `points` as its size.
///
/// The bytes of the pixmap are RGBA with a straight alpha, row by row from the
/// top left, with no padding. Returns `None` when `AppKit` refuses to make the
/// representation. When it refuses to tag it as sRGB, the untagged
/// representation stands.
fn bitmap_representation(pixmap: &Pixmap, points: NSSize) -> Option<Retained<NSBitmapImageRep>> {
    let width = isize::try_from(pixmap.width()).ok()?;
    let height = isize::try_from(pixmap.height()).ok()?;
    let source_stride = usize::try_from(pixmap.width()).ok()? * BYTES_PER_PIXEL;
    let rows = usize::try_from(pixmap.height()).ok()?;
    // SAFETY: The planes pointer is null, so AppKit allocates the buffer. The
    // color space name is a constant of AppKit. The sizes describe one
    // non-planar buffer of 8-bit RGBA pixels with a straight alpha.
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
    let tagged = representation
        .bitmapImageRepByRetaggingWithColorSpace(&NSColorSpace::sRGBColorSpace())
        .unwrap_or(representation);
    tagged.setSize(points);
    Some(tagged)
}

/// Starts a repeating timer on a run loop in the common modes.
///
/// An open menu runs the main run loop in the event tracking mode, which
/// `AppKit` lists among the common modes. A timer in the default mode only
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

/// Asks for a redraw when the appearance or the displays change.
fn register_observers(bar: &Bar) {
    let observer: &AnyObject = bar;
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

thread_local! {
    /// The object of the shell, on the main thread. [`run`] sets it once.
    static BAR: RefCell<Option<Retained<Bar>>> = const { RefCell::new(None) };
}

/// Hands `event` to the shell. It runs on the main thread, from the main queue.
fn deliver(event: ShellEvent) {
    let bar = BAR.with(|cell| cell.borrow().clone());
    if let Some(bar) = bar {
        autoreleasepool(|_| bar.handle_shell(event));
    }
}

/// Sends `event` to the main thread. Any thread can call it.
fn post(event: ShellEvent) {
    DispatchQueue::main().exec_async(move || deliver(event));
}

/// Starts the status item and runs until the user quits.
///
/// The function creates the state directory, starts the watcher first so no
/// change falls between the read and the watch, loads the sessions, and starts
/// the status item. It calls `on_ready` once, when the status item is up and
/// just before the run loop starts.
#[must_use]
pub fn run(settings: &Settings, dirs: &ClaudeDirs, on_ready: Box<dyn FnOnce()>) -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("{NOT_MAIN_THREAD}");
        return ExitCode::from(EXIT_NOT_MAIN_THREAD);
    };
    let state_dir = dirs.state_dir();
    if let Err(error) = std::fs::create_dir_all(&state_dir) {
        eprintln!("cctop-bar: cannot create {}: {error}", state_dir.display());
        return ExitCode::from(EXIT_START_FAILED);
    }
    // The watcher stays alive until `run` returns, which is the end of the program.
    let _watcher = match cctop::store::watch(&state_dir, |event| {
        post(ShellEvent::App(AppEvent::Dir(event)));
    }) {
        Ok(watcher) => watcher,
        Err(error) => {
            eprintln!("cctop-bar: cannot watch {}: {error}", state_dir.display());
            return ExitCode::from(EXIT_START_FAILED);
        }
    };
    let mut controller = Controller::new(
        Sync::new(dirs),
        Box::new(SystemClock::new()),
        settings.notify,
    );
    if let Err(error) = controller.start(EpochMillis::now()) {
        eprintln!("cctop-bar: cannot read {}: {error}", state_dir.display());
        return ExitCode::from(EXIT_START_FAILED);
    }
    let tools = FocusTools {
        runner: Arc::new(SystemRunner),
        writer: Arc::new(DeviceWriter),
    };
    // This process holds the lock of the bar. A `holder` call in the holding
    // process closes a descriptor of the lock file and releases its own lock,
    // so the gate must send each notification and never ask the lock.
    let sink: EventSink = Arc::new(|event| post(ShellEvent::App(event)));
    let jobs = Jobs::new(tools, NotifyGate::Always, sink);

    // The manager stays alive until `run` returns, which is the end of the program.
    let (_hotkey_manager, registered, hotkey_error) = match settings.hotkey {
        None => (None, None, None),
        Some(chord) => match register_hotkey(chord) {
            Ok(manager) => (Some(manager), Some(chord), None),
            Err(error) => (
                None,
                None,
                Some(format!(
                    "The hotkey {} is not registered: {error}",
                    chord.display()
                )),
            ),
        },
    };
    let terminal: SharedTerminal = match Terminal::new(CellGrid::new(INITIAL_COLUMNS, INITIAL_ROWS))
    {
        Ok(terminal) => Rc::new(RefCell::new(terminal)),
        Err(error) => {
            eprintln!("cctop-bar: cannot make the screen of the panel: {error}");
            return ExitCode::from(EXIT_START_FAILED);
        }
    };
    let view = GridView::new(mtm, Rc::clone(&terminal), panel_input);
    let window = make_panel(mtm, &view);

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let status_item =
        NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    let bar = Bar::new(
        mtm,
        Ivars {
            status_item,
            jobs,
            state: RefCell::new(State {
                controller,
                last_error: None,
                painted: None,
                panel: Panel::new(),
            }),
            targets: RefCell::new(Vec::new()),
            window,
            view,
            terminal,
            registered,
            hotkey_error,
        },
    );
    bar.ivars()
        .window
        .setDelegate(Some(ProtocolObject::from_ref(&*bar)));
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    menu.setDelegate(Some(ProtocolObject::from_ref(&*bar)));
    bar.ivars().status_item.setMenu(Some(&menu));
    BAR.with(|cell| *cell.borrow_mut() = Some(bar.clone()));
    bar.refresh();
    // SAFETY: The target is the bar, which answers `tick:` with one timer
    // argument. The timer keeps its target alive.
    unsafe { start_repeating_timer(&NSRunLoop::mainRunLoop(), TICK_SECONDS, &bar, sel!(tick:)) };
    register_observers(&bar);
    on_ready();
    app.run();
    ExitCode::SUCCESS
}
