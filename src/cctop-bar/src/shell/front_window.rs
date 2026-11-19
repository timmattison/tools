//! The bounds of the front window of the front application. The placement of
//! the panel reads them to pick the screen. The window list gives the owner,
//! the layer and the bounds of a window with no Screen Recording permission.
//! The code never reads the name of a window, which needs that permission.

use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType, CGRect};
use objc2_core_graphics::{
    kCGNullWindowID, kCGWindowBounds, kCGWindowLayer, kCGWindowOwnerPID,
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_foundation::MainThreadMarker;

use crate::placement::{flip_window_bounds, Rect};

/// The layer of a normal application window.
const NORMAL_WINDOW_LAYER: i32 = 0;

/// A window entry of the window list.
type WindowInfo = CFDictionary<CFString, CFType>;

/// The number that `key` names in `window`, as an `i32`.
fn number_of(window: &WindowInfo, key: &CFString) -> Option<i32> {
    window.get(key)?.downcast_ref::<CFNumber>()?.as_i32()
}

/// The bounds of `window` in the coordinates of the window list, where y grows
/// downward from the top of the primary screen.
fn bounds_of(window: &WindowInfo, key: &CFString) -> Option<CGRect> {
    let value = window.get(key)?;
    let dictionary = value.downcast_ref::<CFDictionary>()?;
    let mut rect = CGRect::ZERO;
    // SAFETY: The dictionary is a bounds dictionary of the window list. The
    // pointer is a valid pointer to a rectangle on the stack.
    let read = unsafe { CGRectMakeWithDictionaryRepresentation(Some(dictionary), &raw mut rect) };
    read.then_some(rect)
}

/// The height of the primary screen, which is the first of the list.
fn primary_height(mtm: MainThreadMarker) -> Option<f64> {
    NSScreen::screens(mtm)
        .firstObject()
        .map(|screen| screen.frame().size.height)
}

/// The bounds of the first normal window of the front application, in Cocoa
/// global points. Returns `None` when there is no front application, no
/// window, or the system gives no window list.
pub fn front_window(mtm: MainThreadMarker) -> Option<Rect> {
    let pid = NSWorkspace::sharedWorkspace()
        .frontmostApplication()?
        .processIdentifier();
    let height = primary_height(mtm)?;
    let list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    )?;
    // SAFETY: Each entry of the window list is a dictionary with string keys.
    let list =
        unsafe { CFRetained::cast_unchecked::<objc2_core_foundation::CFArray<WindowInfo>>(list) };
    // SAFETY: The keys are constants of Core Graphics that exist at run time.
    let (owner, layer, bounds) = unsafe { (kCGWindowOwnerPID, kCGWindowLayer, kCGWindowBounds) };
    (0..list.len())
        .filter_map(|index| list.get(index))
        .find(|window| {
            number_of(window, owner) == Some(pid)
                && number_of(window, layer) == Some(NORMAL_WINDOW_LAYER)
        })
        .and_then(|window| bounds_of(&window, bounds))
        .map(|rect| {
            flip_window_bounds(
                Rect {
                    x: rect.origin.x,
                    y: rect.origin.y,
                    width: rect.size.width,
                    height: rect.size.height,
                },
                height,
            )
        })
}
