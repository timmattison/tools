//! The floating panel and its grid view. The panel never takes the focus of
//! the application that the person works in. The view draws the cells of the
//! cctop screen and gives each key to the shell.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use cctop::state::Rgb;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSFont, NSFontAttributeName, NSFontWeightBold,
    NSFontWeightRegular, NSForegroundColorAttributeName, NSPanel, NSRectFill, NSStringDrawing,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};
use ratatui::Terminal;

use crate::cellgrid::{CellGrid, PanelColors};
use crate::keymap::{self, PanelKey};
use crate::panel::PanelInput;
use crate::placement::CellSize;

/// The size of the font of the panel, in points.
const FONT_SIZE: f64 = 13.0;

/// The text that gives the width of one cell.
const WIDTH_SAMPLE: &str = "M";

/// The attributes of a glyph: its font and its color.
type Attributes = Retained<NSDictionary<NSString, AnyObject>>;

/// What decides the attributes of a glyph: the channels of its color, and bold.
type AttributeKey = (u8, u8, u8, bool);

/// The terminal of the cctop screen, which the shell and the view share.
pub type SharedTerminal = Rc<RefCell<Terminal<CellGrid>>>;

/// The values that the view holds.
pub struct ViewIvars {
    /// The cells that the view draws.
    terminal: SharedTerminal,
    /// The font of a normal glyph.
    regular: Retained<NSFont>,
    /// The font of a bold glyph.
    bold: Retained<NSFont>,
    /// The size of one cell.
    cell: CellSize,
    /// What a key press does with the shell.
    on_input: fn(PanelInput),
}

define_class!(
    /// The panel. It can become the key window, so it gets the keys, and it
    /// never becomes the main window.
    // SAFETY:
    // - `NSPanel` has no subclassing requirement that this class breaks.
    // - `GridPanel` has no ivars and does not implement `Drop`.
    #[unsafe(super(NSPanel, NSWindow, objc2_app_kit::NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    pub struct GridPanel;

    impl GridPanel {
        /// The panel takes the keys.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        /// The panel is never the main window of the application.
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }
    }
);

define_class!(
    /// The view that draws the cells and takes the keys.
    // SAFETY:
    // - `NSView` has no subclassing requirement that this class breaks.
    // - `GridView` does not implement `Drop`.
    #[unsafe(super(NSView, objc2_app_kit::NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewIvars]
    pub struct GridView;

    impl GridView {
        /// Row 0 is at the top.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// The view takes the keys.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        /// Draws the cells.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw_cells();
        }

        /// Gives the key to the shell. The view never calls the method of its
        /// superclass, so no key makes the alert sound.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let characters = event.charactersIgnoringModifiers().map(|text| text.to_string());
            let flags = u64::try_from(event.modifierFlags().0).unwrap_or(0);
            let key = keymap::map_key(event.keyCode(), characters.as_deref(), flags);
            if key != PanelKey::Ignore {
                (self.ivars().on_input)(PanelInput::Key(key));
            }
        }
    }
);

impl GridView {
    /// Makes the view. `on_input` gets each key of the person.
    pub fn new(
        mtm: MainThreadMarker,
        terminal: SharedTerminal,
        on_input: fn(PanelInput),
    ) -> Retained<GridView> {
        let regular = font(FONT_SIZE, false);
        let bold = font(FONT_SIZE, true);
        let cell = cell_size(&regular);
        let this = GridView::alloc(mtm).set_ivars(ViewIvars {
            terminal,
            regular,
            bold,
            cell,
            on_input,
        });
        // SAFETY: The signature of the `init` method of `NSObject` is correct.
        unsafe { msg_send![super(this), init] }
    }

    /// The size of one cell, in points.
    pub fn cell(&self) -> CellSize {
        self.ivars().cell
    }

    /// Fills the background and draws each glyph at the origin of its column.
    fn draw_cells(&self) {
        let ivars = self.ivars();
        let colors = PanelColors::default();
        fill(&ns_color(colors.background), self.bounds());
        let paint = ivars.terminal.borrow().backend().paint(&colors);
        let cell = ivars.cell;
        for run in &paint.backgrounds {
            fill(
                &ns_color(run.color),
                NSRect::new(
                    NSPoint::new(
                        f64::from(run.column) * cell.width,
                        f64::from(run.row) * cell.height,
                    ),
                    NSSize::new(f64::from(run.cells) * cell.width, cell.height),
                ),
            );
        }
        let mut attributes: HashMap<AttributeKey, Attributes> = HashMap::new();
        for glyph in &paint.glyphs {
            let dictionary = attributes
                .entry((
                    glyph.color.red,
                    glyph.color.green,
                    glyph.color.blue,
                    glyph.bold,
                ))
                .or_insert_with(|| {
                    let font = if glyph.bold {
                        &ivars.bold
                    } else {
                        &ivars.regular
                    };
                    attributes_of(font, &ns_color(glyph.color))
                });
            let origin = NSPoint::new(
                f64::from(glyph.column) * cell.width,
                f64::from(glyph.row) * cell.height,
            );
            // SAFETY: The dictionary holds a font and a color under the keys
            // of AppKit, which is what the method expects.
            unsafe {
                NSString::from_str(&glyph.glyph)
                    .drawAtPoint_withAttributes(origin, Some(&*dictionary));
            }
        }
    }
}

/// Makes the panel, hidden, with `view` as its content view.
///
/// The style mask is given at the init, because the non-activating flag does
/// not work when it is set later.
pub fn make_panel(mtm: MainThreadMarker, view: &GridView) -> Retained<GridPanel> {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSFloatingWindowLevel,
        NSWindowCollectionBehavior,
    };
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
    // SAFETY: The signature of the init method of `NSWindow` is correct.
    let panel: Retained<GridPanel> = unsafe {
        msg_send![
            super(GridPanel::alloc(mtm).set_ivars(())),
            initWithContentRect: frame,
            styleMask: style,
            backing: NSBackingStoreType::Buffered,
            defer: false
        ]
    };
    panel.setLevel(NSFloatingWindowLevel);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    panel.setHidesOnDeactivate(false);
    // SAFETY: The panel is a retained object that nothing else releases.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setOpaque(true);
    panel.setHasShadow(true);
    panel.setBackgroundColor(Some(&ns_color(PanelColors::default().background)));
    // SAFETY: The name is a constant of AppKit that exists at run time.
    let dark = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua });
    panel.setAppearance(dark.as_deref());
    panel.setContentView(Some(view));
    panel
}

/// The monospaced system font, regular or bold.
fn font(size: f64, bold: bool) -> Retained<NSFont> {
    // SAFETY: The weights are constants of AppKit that exist at run time.
    let weight = unsafe {
        if bold {
            NSFontWeightBold
        } else {
            NSFontWeightRegular
        }
    };
    NSFont::monospacedSystemFontOfSize_weight(size, weight)
}

/// The size of one cell of `font`: the width of an `M` and the line height.
fn cell_size(font: &NSFont) -> CellSize {
    let attributes = attributes_of(font, &NSColor::whiteColor());
    // SAFETY: The dictionary holds a font under the key of AppKit.
    let width =
        unsafe { NSString::from_str(WIDTH_SAMPLE).sizeWithAttributes(Some(&attributes)) }.width;
    CellSize {
        width,
        height: (font.ascender() - font.descender() + font.leading()).ceil(),
    }
}

/// The attributes of a glyph: its font and its color.
fn attributes_of(font: &NSFont, color: &NSColor) -> Attributes {
    // SAFETY: The keys are constants of AppKit that exist at run time.
    let keys: [&NSString; 2] = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
    let font: &AnyObject = font;
    let color: &AnyObject = color;
    NSDictionary::from_slices(&keys, &[font, color])
}

/// The color of `rgb`.
fn ns_color(rgb: Rgb) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(rgb.red) / 255.0,
        f64::from(rgb.green) / 255.0,
        f64::from(rgb.blue) / 255.0,
        1.0,
    )
}

/// Fills `rect` with `color`.
fn fill(color: &NSColor, rect: NSRect) {
    color.setFill();
    NSRectFill(rect);
}
