//! The layout of the timeline: the map from time to pixels, the zoom, and the
//! scroll. The functions are pure, so a test needs no webview.
//!
//! The timeline shows project time: the times of the clips on their tracks,
//! before the filters change them.

/// The largest zoom, in pixels per second: one pixel for each millisecond.
pub const MAX_PIXELS_PER_SECOND: f64 = 1000.0;
/// The factor of one step of a zoom button or a zoom key. Remotion Studio
/// uses the same factor.
pub const ZOOM_STEP: f64 = 1.2;
/// The smallest width of the window in pixels. A window that has no layout
/// yet counts as this width, so no division is by zero.
const MIN_WIDTH: f64 = 1.0;

/// What the timeline shows: the length of the project in seconds, and the
/// width of the window in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extent {
    duration: f64,
    width: f64,
}

impl Extent {
    /// Make an extent. A duration that is not finite, or that is below zero,
    /// counts as zero. A width that is not finite, or that is below
    /// `MIN_WIDTH`, counts as `MIN_WIDTH`.
    #[must_use]
    pub fn new(duration: f64, width: f64) -> Self {
        let duration = if duration.is_finite() && duration > 0.0 {
            duration
        } else {
            0.0
        };
        let width = if width.is_finite() && width > MIN_WIDTH {
            width
        } else {
            MIN_WIDTH
        };
        Self { duration, width }
    }

    /// The zoom that shows the whole project in the window. It is never above
    /// `MAX_PIXELS_PER_SECOND`, and it is above zero.
    fn fit(self) -> f64 {
        let fit = self.width / self.duration;
        // An empty project gives infinity, and `min` holds it at the largest
        // zoom. A very long project gives a very small zoom.
        if fit > 0.0 {
            fit.min(MAX_PIXELS_PER_SECOND)
        } else {
            f64::MIN_POSITIVE
        }
    }
}

/// The zoom and the scroll of the timeline.
///
/// The default view shows the whole project. A view that shows the whole
/// project stays that way when the window or the project changes its size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct View {
    /// The zoom in pixels per second, or `None` for the zoom that shows the
    /// whole project.
    zoom: Option<f64>,
    /// The time at the left edge of the window, in seconds.
    start: f64,
}

impl View {
    /// The zoom and the scroll of this view in `extent`. The zoom is at least
    /// the zoom that shows the whole project, and at most
    /// `MAX_PIXELS_PER_SECOND`. The start is at least zero, and the window
    /// never goes past the end of the project, unless the whole project is
    /// shorter than the window.
    #[must_use]
    pub fn frame(self, extent: Extent) -> Frame {
        let fit = extent.fit();
        let pixels_per_second = match self.zoom {
            Some(zoom) if zoom.is_finite() => zoom.clamp(fit, MAX_PIXELS_PER_SECOND),
            _ => fit,
        };
        let last_start = (extent.duration - extent.width / pixels_per_second).max(0.0);
        let start = if self.start.is_finite() {
            self.start.clamp(0.0, last_start)
        } else {
            0.0
        };
        Frame {
            pixels_per_second,
            start,
            width: extent.width,
        }
    }

    /// The view after a zoom by `factor` that keeps the time at `anchor`, a
    /// place in the window in pixels, at the same place. A factor above 1
    /// zooms in. A factor that is not finite, or that is not above zero,
    /// changes nothing. An anchor that is not finite is the center of the
    /// window.
    #[must_use]
    pub fn zoom(self, extent: Extent, factor: f64, anchor: f64) -> Self {
        if !factor.is_finite() || factor <= 0.0 {
            return self;
        }
        let frame = self.frame(extent);
        let anchor = if anchor.is_finite() {
            anchor
        } else {
            frame.width / 2.0
        };
        let time = frame.time_at(anchor);
        let fit = extent.fit();
        let pixels_per_second =
            (frame.pixels_per_second * factor).clamp(fit, MAX_PIXELS_PER_SECOND);
        // At the zoom of the whole project, the view follows later changes of
        // the window and of the project.
        let zoom = (pixels_per_second > fit).then_some(pixels_per_second);
        Self {
            zoom,
            start: time - anchor / pixels_per_second,
        }
        .clamped(extent)
    }

    /// The view after a scroll by `pixels`. A positive value shows later
    /// times. A value that is not finite changes nothing.
    #[must_use]
    pub fn pan(self, extent: Extent, pixels: f64) -> Self {
        if !pixels.is_finite() {
            return self;
        }
        let frame = self.frame(extent);
        Self {
            zoom: self.zoom,
            start: frame.start + pixels / frame.pixels_per_second,
        }
        .clamped(extent)
    }

    /// This view with the start that `frame` gives it in `extent`.
    fn clamped(self, extent: Extent) -> Self {
        Self {
            zoom: self.zoom,
            start: self.frame(extent).start,
        }
    }
}

/// The zoom and the scroll that the timeline draws: a map from time to
/// pixels in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pixels_per_second: f64,
    start: f64,
    width: f64,
}

impl Frame {
    /// The zoom in pixels per second. It is above zero.
    #[must_use]
    pub fn pixels_per_second(self) -> f64 {
        self.pixels_per_second
    }

    /// The time at the left edge of the window, in seconds.
    #[must_use]
    pub fn start(self) -> f64 {
        self.start
    }

    /// The time at the right edge of the window, in seconds.
    #[must_use]
    pub fn end(self) -> f64 {
        self.start + self.width / self.pixels_per_second
    }

    /// The width of the window in pixels.
    #[must_use]
    pub fn width(self) -> f64 {
        self.width
    }

    /// The place of `time` in the window, in pixels from the left edge.
    #[must_use]
    pub fn x_of(self, time: f64) -> f64 {
        (time - self.start) * self.pixels_per_second
    }

    /// The time at `x`, a place in the window in pixels from the left edge.
    #[must_use]
    pub fn time_at(self, x: f64) -> f64 {
        self.start + x / self.pixels_per_second
    }
}

#[cfg(test)]
mod tests;
