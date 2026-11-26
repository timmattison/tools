//! The comparison of two pictures, pixel by pixel, for `--diff`.
//!
//! [`compare`] takes two decoded pictures and gives one picture of the
//! differences, the regions that differ, and a count of the pixels that
//! differ. The module reads no file and writes no terminal.
#![deny(clippy::indexing_slicing)]

use anyhow::{bail, Result};
use image::{DynamicImage, Rgba, RgbaImage};

/// Red, the color of a pixel that differs and of each outline.
pub const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);

/// A pixel that no picture covers. It has no color and no alpha.
pub const BLANK: Rgba<u8> = Rgba([0, 0, 0, 0]);

/// Opaque black, the ground of the `mask` mode.
const BLACK: Rgba<u8> = Rgba([0, 0, 0, 255]);

/// The largest canvas, in pixels: 2^27 pixels, which is 512 MiB of RGBA.
pub const MAX_CANVAS_PIXELS: u64 = 1 << 27;

/// What the picture of the differences shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Mode {
    /// The second image, with a red outline around each region that differs
    Boxes,
    /// The first image, with each pixel that differs in red
    #[default]
    Highlight,
    /// Opaque black, with each pixel that differs in red
    Mask,
}

/// A rectangle of the canvas, in pixels. All four edges are inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    /// The first column of the rectangle.
    pub left: u32,
    /// The first row of the rectangle.
    pub top: u32,
    /// The last column of the rectangle.
    pub right: u32,
    /// The last row of the rectangle.
    pub bottom: u32,
}

/// The result of one comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct Diff {
    /// The picture of the differences, the size of the canvas.
    pub picture: RgbaImage,
    /// The padded and merged boxes, sorted by (top, left).
    pub regions: Vec<Region>,
    /// The count of the pixels that differ.
    pub differing_pixels: u64,
}

impl Diff {
    /// The second line of the header that `ic` prints above the picture.
    ///
    /// # Returns
    ///
    /// `the images are the same` when no pixel differs. Otherwise a line such
    /// as `3 regions differ (1204 pixels)`.
    pub fn summary(&self) -> String {
        if self.differing_pixels == 0 {
            return "the images are the same".to_string();
        }
        let regions = self.regions.len();
        let (noun, verb) = if regions == 1 {
            ("region", "differs")
        } else {
            ("regions", "differ")
        };
        let pixels = if self.differing_pixels == 1 {
            "pixel"
        } else {
            "pixels"
        };
        format!(
            "{regions} {noun} {verb} ({} {pixels})",
            self.differing_pixels
        )
    }
}

/// Compare two images and give the picture of the differences in `mode`.
///
/// # Arguments
///
/// * `first` - The first image. The function takes it by value, so that it
///   does not copy it: an RGBA8 image moves into the comparison.
/// * `second` - The second image, taken by value for the same reason.
/// * `mode` - What the picture shows.
///
/// # Errors
///
/// Returns an error when the canvas holds more than [`MAX_CANVAS_PIXELS`]
/// pixels.
pub fn compare(first: DynamicImage, second: DynamicImage, mode: Mode) -> Result<Diff> {
    let first = first.into_rgba8();
    let second = second.into_rgba8();
    let width = first.width().max(second.width());
    let height = first.height().max(second.height());
    let canvas_pixels = u64::from(width).saturating_mul(u64::from(height));
    if canvas_pixels > MAX_CANVAS_PIXELS {
        bail!(
            "the canvas of the diff is {width}x{height} pixels, and the limit is \
             {MAX_CANVAS_PIXELS} pixels"
        );
    }
    let covered = |x: u32, y: u32| covers(&first, x, y) || covers(&second, x, y);
    let mut picture = RgbaImage::new(width, height);
    let mut differing = Grid::new(width, height);
    let mut differing_pixels = 0_u64;
    for (x, y, out) in picture.enumerate_pixels_mut() {
        let one = pixel_at(&first, x, y);
        let two = pixel_at(&second, x, y);
        let differs = one != two;
        if differs {
            differing_pixels = differing_pixels.saturating_add(1);
            differing.set(x, y);
        }
        *out = match mode {
            Mode::Boxes => two,
            Mode::Highlight if differs => RED,
            Mode::Mask if differs => RED,
            Mode::Mask if covered(x, y) => BLACK,
            _ => one,
        };
    }
    let thickness = line_thickness(width, height);
    let regions = find_regions(&differing, thickness);
    if mode == Mode::Boxes {
        for region in &regions {
            draw_outline(&mut picture, *region, thickness, covered);
        }
    }
    Ok(Diff {
        picture,
        regions,
        differing_pixels,
    })
}

/// The pixel of `image` at (`x`, `y`), or [`BLANK`] when the image does not
/// cover that position.
fn pixel_at(image: &RgbaImage, x: u32, y: u32) -> Rgba<u8> {
    image.get_pixel_checked(x, y).copied().unwrap_or(BLANK)
}

/// Whether `image` covers the position (`x`, `y`).
fn covers(image: &RgbaImage, x: u32, y: u32) -> bool {
    x < image.width() && y < image.height()
}

/// A flag for each pixel of the canvas.
struct Grid {
    /// The width of the canvas, in pixels.
    width: u32,
    /// The height of the canvas, in pixels.
    height: u32,
    /// The flags, row by row from the top left.
    cells: Vec<bool>,
}

impl Grid {
    /// A grid of `width` by `height` flags that are all off.
    fn new(width: u32, height: u32) -> Self {
        let count = u64::from(width).saturating_mul(u64::from(height));
        Self {
            width,
            height,
            cells: vec![false; usize::try_from(count).unwrap_or(0)],
        }
    }

    /// The position of (`x`, `y`) in `cells`, or `None` outside the grid.
    fn index(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let row = usize::try_from(y).ok()?;
        let column = usize::try_from(x).ok()?;
        row.checked_mul(usize::try_from(self.width).ok()?)?
            .checked_add(column)
    }

    /// Whether the flag at (`x`, `y`) is on. A position outside the grid is off.
    fn get(&self, x: u32, y: u32) -> bool {
        self.index(x, y)
            .and_then(|index| self.cells.get(index))
            .copied()
            .unwrap_or(false)
    }

    /// Turn the flag at (`x`, `y`) on. A position outside the grid does nothing.
    fn set(&mut self, x: u32, y: u32) {
        if let Some(cell) = self.index(x, y).and_then(|index| self.cells.get_mut(index)) {
            *cell = true;
        }
    }
}

/// A maximal horizontal run of pixels that differ, in one row.
#[derive(Debug, Clone, Copy)]
struct Run {
    /// The first column of the run.
    left: u32,
    /// The last column of the run. The edge is inclusive.
    right: u32,
    /// The index of the [`Group`] that holds the run.
    label: usize,
}

/// A node of a union-find over the runs that touch.
#[derive(Debug, Clone, Copy)]
struct Group {
    /// The node that this node joined. A root is its own parent.
    parent: usize,
    /// The box of the pixels of the group, with no padding. Valid on a root.
    bounds: Region,
}

/// What the compaction of a row has done with one group.
#[derive(Debug, Clone, Copy)]
enum Slot {
    /// No run of the row has reached this group.
    Unseen,
    /// The group lives on, at this index of the new arena.
    Active(usize),
    /// The group has ended, and its box is in the finished list.
    Finished,
}

/// The root of the group `start`. The walk halves the path as it climbs.
fn find_root(groups: &mut [Group], start: usize) -> usize {
    let mut node = start;
    loop {
        let Some(parent) = groups.get(node).map(|group| group.parent) else {
            return node;
        };
        if parent == node {
            return node;
        }
        let grandparent = groups.get(parent).map_or(parent, |group| group.parent);
        if let Some(group) = groups.get_mut(node) {
            group.parent = grandparent;
        }
        node = grandparent;
    }
}

/// Grow the box `target` so that it holds the box `other` too.
fn widen(target: &mut Region, other: Region) {
    *target = union(*target, other);
}

/// Put the runs of the row `y` into `runs`, left to right.
fn collect_runs(differing: &Grid, y: u32, runs: &mut Vec<Run>) {
    runs.clear();
    let mut start: Option<u32> = None;
    for x in 0..differing.width {
        match (differing.get(x, y), start) {
            (true, None) => start = Some(x),
            (false, Some(left)) => {
                runs.push(Run {
                    left,
                    right: x.saturating_sub(1),
                    label: 0,
                });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(left) = start {
        runs.push(Run {
            left,
            right: differing.width.saturating_sub(1),
            label: 0,
        });
    }
}

/// Give each run of `current` a group: the group of each run of `previous`
/// that touches it, also at a corner, or a new group. Groups that two runs of
/// `previous` held apart are joined.
fn label_runs(groups: &mut Vec<Group>, previous: &[Run], current: &mut [Run], y: u32) {
    let mut first = 0_usize;
    for run in current.iter_mut() {
        while previous
            .get(first)
            .is_some_and(|p| p.right.saturating_add(1) < run.left)
        {
            first = first.saturating_add(1);
        }
        let mut label: Option<usize> = None;
        let mut index = first;
        while let Some(p) = previous.get(index) {
            if p.left > run.right.saturating_add(1) {
                break;
            }
            index = index.saturating_add(1);
            let root = find_root(groups, p.label);
            match label {
                None => label = Some(root),
                Some(kept) if kept != root => {
                    let other = groups.get(root).map(|group| group.bounds);
                    if let Some(group) = groups.get_mut(root) {
                        group.parent = kept;
                    }
                    if let (Some(other), Some(group)) = (other, groups.get_mut(kept)) {
                        widen(&mut group.bounds, other);
                    }
                }
                Some(_) => {}
            }
        }
        let label = label.unwrap_or_else(|| {
            let at = groups.len();
            groups.push(Group {
                parent: at,
                bounds: Region {
                    left: run.left,
                    top: y,
                    right: run.right,
                    bottom: y,
                },
            });
            at
        });
        run.label = label;
        if let Some(group) = groups.get_mut(label) {
            widen(
                &mut group.bounds,
                Region {
                    left: run.left,
                    top: y,
                    right: run.right,
                    bottom: y,
                },
            );
        }
    }
}

/// Keep only the groups that `current` holds, give each run of `current` the
/// index of its group in the new arena, and push the box of each group that
/// only `previous` held onto `finished`.
fn finish_and_compact(
    groups: &mut Vec<Group>,
    previous: &[Run],
    current: &mut [Run],
    finished: &mut Vec<Region>,
) {
    let mut slots = vec![Slot::Unseen; groups.len()];
    let mut arena: Vec<Group> = Vec::with_capacity(current.len());
    for run in current.iter_mut() {
        let root = find_root(groups, run.label);
        match slots.get(root).copied().unwrap_or(Slot::Finished) {
            Slot::Active(at) => run.label = at,
            _ => {
                let at = arena.len();
                if let Some(group) = groups.get(root) {
                    arena.push(Group {
                        parent: at,
                        bounds: group.bounds,
                    });
                }
                if let Some(slot) = slots.get_mut(root) {
                    *slot = Slot::Active(at);
                }
                run.label = at;
            }
        }
    }
    finish_groups(groups, previous, &mut slots, finished);
    *groups = arena;
}

/// Push the box of each group of `runs` that is still `Unseen`, once.
fn finish_groups(
    groups: &mut [Group],
    runs: &[Run],
    slots: &mut [Slot],
    finished: &mut Vec<Region>,
) {
    for run in runs {
        let root = find_root(groups, run.label);
        if let (Some(slot @ Slot::Unseen), Some(group)) = (slots.get_mut(root), groups.get(root)) {
            finished.push(group.bounds);
            *slot = Slot::Finished;
        }
    }
}

/// The padded boxes of the pixels that differ, sorted by (top, left).
///
/// Two pixels that differ are in one group when they touch, also at a corner.
/// The box of a group has `thickness + 2` pixels of padding on each side, and
/// the canvas clips it.
///
/// The function makes one pass over the rows. It cuts each row into runs of
/// pixels that differ, and it joins each run to the runs of the row above that
/// touch it, also at a corner. A group that no run of the row continues is
/// finished, and only its box stays. The work memory thus grows with the width
/// of the canvas, plus one box for each finished group, and not with the area.
///
/// # Arguments
///
/// * `differing` - The flag of each pixel that differs.
/// * `thickness` - The thickness of an outline, in pixels.
fn find_regions(differing: &Grid, thickness: u32) -> Vec<Region> {
    let padding = thickness.saturating_add(2);
    let last_column = differing.width.saturating_sub(1);
    let last_row = differing.height.saturating_sub(1);
    let mut groups: Vec<Group> = Vec::new();
    let mut previous: Vec<Run> = Vec::new();
    let mut current: Vec<Run> = Vec::new();
    let mut finished: Vec<Region> = Vec::new();
    for y in 0..differing.height {
        collect_runs(differing, y, &mut current);
        label_runs(&mut groups, &previous, &mut current, y);
        finish_and_compact(&mut groups, &previous, &mut current, &mut finished);
        std::mem::swap(&mut previous, &mut current);
    }
    let mut slots = vec![Slot::Unseen; groups.len()];
    finish_groups(&mut groups, &previous, &mut slots, &mut finished);
    let regions = finished
        .into_iter()
        .map(|bounds| Region {
            left: bounds.left.saturating_sub(padding),
            top: bounds.top.saturating_sub(padding),
            right: bounds.right.saturating_add(padding).min(last_column),
            bottom: bounds.bottom.saturating_add(padding).min(last_row),
        })
        .collect();
    let mut regions = merge_touching(regions);
    regions.sort_by_key(|region| (region.top, region.left));
    regions
}

/// Draw a red outline inside the edge of `region`.
///
/// A pixel of the region is red when it is less than `thickness` pixels from
/// an edge. A pixel that `covered` rejects stays as it is.
///
/// # Arguments
///
/// * `picture` - The picture to draw on.
/// * `region` - The box to outline.
/// * `thickness` - The thickness of the outline, in pixels.
/// * `covered` - Whether an image covers a pixel.
fn draw_outline(
    picture: &mut RgbaImage,
    region: Region,
    thickness: u32,
    covered: impl Fn(u32, u32) -> bool,
) {
    for y in region.top..=region.bottom {
        for x in region.left..=region.right {
            let on_edge = x.saturating_sub(region.left) < thickness
                || region.right.saturating_sub(x) < thickness
                || y.saturating_sub(region.top) < thickness
                || region.bottom.saturating_sub(y) < thickness;
            if on_edge && covered(x, y) {
                if let Some(pixel) = picture.get_pixel_mut_checked(x, y) {
                    *pixel = RED;
                }
            }
        }
    }
}

/// Whether two boxes overlap or touch, also at a corner.
fn touches(a: Region, b: Region) -> bool {
    a.left <= b.right.saturating_add(1)
        && b.left <= a.right.saturating_add(1)
        && a.top <= b.bottom.saturating_add(1)
        && b.top <= a.bottom.saturating_add(1)
}

/// The one box that holds both `a` and `b`.
fn union(a: Region, b: Region) -> Region {
    Region {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    }
}

/// Merge the boxes that overlap or touch.
///
/// A merged box can grow into a third box that touched neither part, so the
/// function repeats the sweep until a sweep merges nothing.
///
/// # Arguments
///
/// * `regions` - The boxes, in any order.
///
/// # Returns
///
/// The merged boxes, sorted by `left`.
fn merge_touching(mut regions: Vec<Region>) -> Vec<Region> {
    loop {
        let before = regions.len();
        regions = merge_sweep(regions);
        if regions.len() == before {
            return regions;
        }
    }
}

/// One sweep of [`merge_touching`].
///
/// The sweep sorts the boxes by `left`. It compares each box only with the
/// later boxes whose `left` is at most one pixel past its `right`, so a
/// picture with many far boxes stays fast.
fn merge_sweep(mut regions: Vec<Region>) -> Vec<Region> {
    regions.sort_by_key(|region| region.left);
    let mut taken = vec![false; regions.len()];
    let mut merged = Vec::new();
    for (start, first) in regions.iter().enumerate() {
        if taken.get(start).copied().unwrap_or(true) {
            continue;
        }
        let mut current = *first;
        for (later, candidate) in regions.iter().enumerate().skip(start.saturating_add(1)) {
            if candidate.left > current.right.saturating_add(1) {
                break;
            }
            let free = taken.get(later).is_some_and(|taken| !taken);
            if free && touches(current, *candidate) {
                current = union(current, *candidate);
                if let Some(flag) = taken.get_mut(later) {
                    *flag = true;
                }
            }
        }
        merged.push(current);
    }
    merged
}

/// The thickness of an outline, in pixels: `max(2, ceil(L / 400))`, where `L`
/// is the longer side of the canvas.
///
/// # Arguments
///
/// * `canvas_width` - The width of the canvas, in pixels.
/// * `canvas_height` - The height of the canvas, in pixels.
pub fn line_thickness(canvas_width: u32, canvas_height: u32) -> u32 {
    canvas_width.max(canvas_height).div_ceil(400).max(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A distinct opaque color for each position.
    fn color(x: u32, y: u32) -> Rgba<u8> {
        Rgba([
            u8::try_from(x * 40 + 10).unwrap(),
            u8::try_from(y * 40 + 10).unwrap(),
            100,
            255,
        ])
    }

    /// A picture that holds `color` at each position.
    fn picture(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_fn(width, height, color))
    }

    #[test]
    fn pictures_of_two_sizes_share_the_upper_left_corner() {
        let diff = compare(picture(3, 2), picture(2, 3), Mode::Highlight).unwrap();
        assert_eq!(diff.picture.dimensions(), (3, 3));
        assert_eq!(*diff.picture.get_pixel(1, 1), color(1, 1));
        assert_eq!(diff.differing_pixels, 4);
    }

    #[test]
    fn a_fully_transparent_strip_is_not_a_difference() {
        let first = RgbaImage::from_fn(3, 2, |x, y| if x == 2 { BLANK } else { color(x, y) });
        let diff = compare(
            DynamicImage::ImageRgba8(first),
            picture(2, 2),
            Mode::Highlight,
        )
        .unwrap();
        assert_eq!(diff.differing_pixels, 0);
    }

    /// The color that `changed` writes.
    const OTHER: Rgba<u8> = Rgba([1, 2, 3, 255]);

    /// A picture like `picture`, with `OTHER` at each listed position.
    fn changed(width: u32, height: u32, spots: &[(u32, u32)]) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_fn(width, height, |x, y| {
            if spots.contains(&(x, y)) {
                OTHER
            } else {
                color(x, y)
            }
        }))
    }

    #[test]
    fn highlight_paints_a_changed_pixel_red() {
        let diff = compare(picture(3, 2), changed(2, 3, &[(0, 0)]), Mode::Highlight).unwrap();
        assert_eq!(*diff.picture.get_pixel(1, 1), color(1, 1));
        assert_eq!(*diff.picture.get_pixel(0, 0), RED);
        assert_eq!(*diff.picture.get_pixel(2, 2), BLANK);
    }

    #[test]
    fn mask_paints_the_same_pixels_black_and_the_changed_ones_red() {
        let diff = compare(picture(3, 2), changed(2, 3, &[(0, 0)]), Mode::Mask).unwrap();
        assert_eq!(*diff.picture.get_pixel(1, 1), BLACK);
        assert_eq!(*diff.picture.get_pixel(0, 0), RED);
        assert_eq!(*diff.picture.get_pixel(2, 2), BLANK);
    }

    #[test]
    fn the_line_thickness_grows_with_the_longer_side() {
        assert_eq!(line_thickness(1, 1), 2);
        assert_eq!(line_thickness(800, 10), 2);
        assert_eq!(line_thickness(10, 801), 3);
        assert_eq!(line_thickness(4000, 3000), 10);
    }

    /// A color that depends on the position, and that is never `OTHER`.
    fn shade(x: u32, y: u32) -> Rgba<u8> {
        Rgba([
            u8::try_from(x * 8 % 256).unwrap(),
            u8::try_from(y * 8 % 256).unwrap(),
            50,
            255,
        ])
    }

    /// A picture of the given size that holds `shade`, with `OTHER` at each
    /// listed position.
    fn field(width: u32, height: u32, spots: &[(u32, u32)]) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_fn(width, height, |x, y| {
            if spots.contains(&(x, y)) {
                OTHER
            } else {
                shade(x, y)
            }
        }))
    }

    /// The region with these four edges.
    fn region(left: u32, top: u32, right: u32, bottom: u32) -> Region {
        Region {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn boxes_of_two_equal_pictures_show_the_second_picture() {
        let second = field(20, 20, &[]);
        let diff = compare(field(20, 20, &[]), second.clone(), Mode::Boxes).unwrap();
        assert!(diff.regions.is_empty());
        assert_eq!(diff.differing_pixels, 0);
        assert_eq!(diff.picture, second.to_rgba8());
    }

    #[test]
    fn boxes_outline_one_changed_pixel_inside_its_padded_box() {
        let second = field(30, 30, &[(10, 10)]);
        let diff = compare(field(30, 30, &[]), second.clone(), Mode::Boxes).unwrap();
        assert_eq!(diff.regions, vec![region(6, 6, 14, 14)]);
        assert_eq!(diff.differing_pixels, 1);
        for (x, y) in [(6, 10), (7, 10), (13, 10), (14, 10), (10, 6), (10, 14)] {
            assert_eq!(*diff.picture.get_pixel(x, y), RED, "({x}, {y})");
        }
        let second = second.to_rgba8();
        for (x, y) in [(8, 10), (10, 10), (12, 10), (5, 10)] {
            assert_eq!(
                diff.picture.get_pixel(x, y),
                second.get_pixel(x, y),
                "({x}, {y})"
            );
        }
    }

    /// The regions of two pictures that differ at the listed positions.
    fn regions_of(width: u32, height: u32, spots: &[(u32, u32)]) -> Vec<Region> {
        compare(
            field(width, height, &[]),
            field(width, height, spots),
            Mode::Boxes,
        )
        .unwrap()
        .regions
    }

    #[test]
    fn far_changes_make_two_regions() {
        assert_eq!(
            regions_of(60, 20, &[(10, 10), (50, 10)]),
            vec![region(6, 6, 14, 14), region(46, 6, 54, 14)]
        );
    }

    #[test]
    fn padded_boxes_that_touch_make_one_region() {
        assert_eq!(
            regions_of(40, 20, &[(10, 10), (19, 10)]),
            vec![region(6, 6, 23, 14)]
        );
    }

    #[test]
    fn padded_boxes_with_a_gap_of_one_pixel_stay_apart() {
        assert_eq!(
            regions_of(40, 20, &[(10, 10), (20, 10)]),
            vec![region(6, 6, 14, 14), region(16, 6, 24, 14)]
        );
    }

    #[test]
    fn a_box_that_touches_only_the_union_is_merged_too() {
        let a = region(0, 0, 10, 10);
        let b = region(11, 5, 20, 15);
        let c = region(0, 12, 5, 20);
        assert!(!touches(a, c) && !touches(b, c));
        assert_eq!(merge_touching(vec![a, b, c]), vec![region(0, 0, 20, 20)]);
    }

    #[test]
    fn boxes_that_meet_at_a_corner_are_merged() {
        assert_eq!(
            merge_touching(vec![region(0, 0, 5, 5), region(6, 6, 9, 9)]),
            vec![region(0, 0, 9, 9)]
        );
    }

    /// A diff with `regions` regions and `pixels` differing pixels.
    fn diff_of(regions: usize, pixels: u64) -> Diff {
        Diff {
            picture: RgbaImage::new(1, 1),
            regions: vec![region(0, 0, 0, 0); regions],
            differing_pixels: pixels,
        }
    }

    #[test]
    fn the_summary_says_when_the_images_are_the_same() {
        assert_eq!(diff_of(0, 0).summary(), "the images are the same");
    }

    #[test]
    fn the_summary_counts_regions_and_pixels_in_the_right_number() {
        assert_eq!(diff_of(1, 1).summary(), "1 region differs (1 pixel)");
        assert_eq!(diff_of(1, 5).summary(), "1 region differs (5 pixels)");
        assert_eq!(diff_of(3, 1204).summary(), "3 regions differ (1204 pixels)");
        assert_eq!(diff_of(2, 1).summary(), "2 regions differ (1 pixel)");
    }

    #[test]
    fn a_canvas_over_the_limit_is_an_error_before_any_allocation() {
        let wide = DynamicImage::ImageRgba8(RgbaImage::new(16384, 1));
        let tall = DynamicImage::ImageRgba8(RgbaImage::new(1, 16384));
        // `unwrap_err` would print the whole picture of a missing error.
        let Err(error) = compare(wide, tall, Mode::Boxes) else {
            panic!("the canvas of 268 million pixels gave a picture");
        };
        let error = error.to_string();
        assert!(error.contains("16384x16384"), "{error}");
        assert!(error.contains("134217728"), "{error}");
    }

    #[test]
    fn boxes_at_the_edge_of_the_canvas_are_clipped() {
        assert_eq!(
            regions_of(20, 20, &[(0, 0), (19, 19)]),
            vec![region(0, 0, 4, 4), region(15, 15, 19, 19)]
        );
    }

    #[test]
    fn a_canvas_of_one_pixel_has_one_region() {
        let diff = compare(field(1, 1, &[]), field(1, 1, &[(0, 0)]), Mode::Boxes).unwrap();
        assert_eq!(diff.regions, vec![region(0, 0, 0, 0)]);
        assert_eq!(*diff.picture.get_pixel(0, 0), RED);
    }

    #[test]
    fn pixels_that_touch_at_a_corner_are_one_group() {
        let diff = compare(
            field(30, 30, &[]),
            field(30, 30, &[(10, 10), (11, 11)]),
            Mode::Boxes,
        )
        .unwrap();
        assert_eq!(diff.regions, vec![region(6, 6, 15, 15)]);
        assert_eq!(diff.differing_pixels, 2);
    }

    #[test]
    fn an_outline_leaves_a_pixel_that_no_image_covers_blank() {
        let diff = compare(field(12, 4, &[]), field(4, 12, &[]), Mode::Boxes).unwrap();
        assert_eq!(diff.regions.len(), 1);
        assert_eq!(*diff.picture.get_pixel(11, 11), BLANK);
        assert_eq!(*diff.picture.get_pixel(11, 3), RED);
    }

    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        /// The bytes that this thread allocated and has not freed.
        static LIVE_BYTES: Cell<usize> = const { Cell::new(0) };
        /// The largest value that `LIVE_BYTES` had since the last reset.
        static PEAK_BYTES: Cell<usize> = const { Cell::new(0) };
    }

    /// An allocator that counts the live bytes of each thread and the peak.
    ///
    /// The counters are thread-local, so the tests that run on other threads
    /// do not change what one test measures.
    struct CountingAllocator;

    /// Add `size` bytes to the live count of this thread and raise the peak.
    fn count_up(size: usize) {
        let _ = LIVE_BYTES.try_with(|live| {
            let now = live.get().saturating_add(size);
            live.set(now);
            let _ = PEAK_BYTES.try_with(|peak| peak.set(peak.get().max(now)));
        });
    }

    /// Take `size` bytes from the live count of this thread.
    fn count_down(size: usize) {
        let _ = LIVE_BYTES.try_with(|live| live.set(live.get().saturating_sub(size)));
    }

    // SAFETY: each method hands the call to `System` with the same arguments, so
    // the contract of `GlobalAlloc` holds as it holds for `System`. The counters
    // are thread-local cells and have no effect on the memory.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: the caller meets the contract of `alloc`, and the layout
            // goes on unchanged.
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                count_up(layout.size());
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            count_down(layout.size());
            // SAFETY: the caller gives a pointer and a layout from `alloc`, and
            // both go on unchanged.
            unsafe { System.dealloc(pointer, layout) };
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            // SAFETY: the caller meets the contract of `realloc`, and the
            // arguments go on unchanged.
            let moved = unsafe { System.realloc(pointer, layout, new_size) };
            if !moved.is_null() {
                count_down(layout.size());
                count_up(new_size);
            }
            moved
        }
    }

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    /// Run `work` and give its result with the bytes that it held at its
    /// largest, above what this thread held when it began.
    fn peak_bytes_of<T>(work: impl FnOnce() -> T) -> (T, usize) {
        let live_at_start = LIVE_BYTES.with(Cell::get);
        PEAK_BYTES.with(|peak| peak.set(live_at_start));
        let result = work();
        let peak = PEAK_BYTES.with(Cell::get);
        (result, peak.saturating_sub(live_at_start))
    }

    /// The side of the square pictures of the memory tests, in pixels.
    const MEMORY_SIDE: u32 = 512;
    /// The bytes of a picture for each pixel: four channels of one byte.
    const PICTURE_BYTES_PER_PIXEL: usize = 4;
    /// The bytes of a `Grid` for each pixel.
    const GRID_BYTES_PER_PIXEL: usize = 1;
    /// The work memory that one pixel of width may cost, in bytes. The runs of
    /// two rows and their groups stay far below this.
    const WORK_BYTES_PER_COLUMN: usize = 1024;

    /// A square picture of `MEMORY_SIDE` pixels that is `value` where `differs`
    /// is true for the position, and black elsewhere.
    fn memory_picture(value: u8, differs: impl Fn(u32, u32) -> bool) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_fn(MEMORY_SIDE, MEMORY_SIDE, |x, y| {
            if differs(x, y) {
                Rgba([value, value, value, 255])
            } else {
                Rgba([0, 0, 0, 255])
            }
        }))
    }

    /// The `Grid` of `MEMORY_SIDE` columns and `height` rows with a flag on
    /// where `on` is true.
    fn memory_grid(height: u32, on: impl Fn(u32, u32) -> bool) -> Grid {
        let mut grid = Grid::new(MEMORY_SIDE, height);
        for y in 0..height {
            for x in 0..MEMORY_SIDE {
                if on(x, y) {
                    grid.set(x, y);
                }
            }
        }
        grid
    }

    /// A rule that says whether the pixel at (x, y) differs.
    type DifferenceRule = fn(u32, u32) -> bool;

    /// The two ways that the tests make every pixel of a region differ.
    fn memory_cases() -> [(&'static str, DifferenceRule); 2] {
        [
            ("every pixel", |_, _| true),
            ("checkerboard", |x, y| (x + y) % 2 == 0),
        ]
    }

    /// The peak of `compare` is the two pictures that it holds, the `Grid`, and
    /// a work area that grows with the width. It is not a copy of each picture
    /// and not a stack that grows with the count of pixels.
    ///
    /// The bound is 4 bytes of picture and 1 byte of `Grid` for each pixel,
    /// plus `WORK_BYTES_PER_COLUMN` for each column. The output picture takes
    /// the 4 bytes, and the images move into the call.
    #[test]
    fn compare_holds_no_copy_of_the_two_images() {
        let pixels = usize::try_from(MEMORY_SIDE * MEMORY_SIDE).unwrap();
        let width = usize::try_from(MEMORY_SIDE).unwrap();
        let bound = (PICTURE_BYTES_PER_PIXEL + GRID_BYTES_PER_PIXEL) * pixels
            + WORK_BYTES_PER_COLUMN * width;
        for (name, differs) in memory_cases() {
            let first = memory_picture(0, |_, _| false);
            let second = memory_picture(255, differs);
            let (diff, peak) = peak_bytes_of(|| compare(first, second, Mode::Boxes).unwrap());
            eprintln!("compare, {name}: peak {peak} bytes, bound {bound}");
            assert_eq!(diff.regions, vec![region(0, 0, 511, 511)], "{name}");
            assert!(
                peak <= bound,
                "{name}: peak {peak} is over the bound {bound}"
            );
        }
    }

    /// How many times taller the tall grid of the width test is than the
    /// square one. A structure of the area grows by this factor.
    const TALL_FACTOR: u32 = 8;

    /// The fill of the regions takes memory that grows with the width, not with
    /// the area.
    ///
    /// The square grid holds its peak below `WORK_BYTES_PER_COLUMN` for each
    /// column. A grid with the same width and `TALL_FACTOR` times the height
    /// must not raise the peak, so a structure of the area fails, also at one
    /// bit for each pixel.
    #[test]
    fn the_regions_take_memory_that_grows_with_the_width() {
        let bound = WORK_BYTES_PER_COLUMN * usize::try_from(MEMORY_SIDE).unwrap();
        for (name, on) in memory_cases() {
            let peak_at = |height: u32| {
                let grid = memory_grid(height, on);
                let (regions, peak) = peak_bytes_of(|| find_regions(&grid, 2));
                eprintln!("find_regions, {name}, {height} rows: peak {peak} bytes");
                let all = region(0, 0, MEMORY_SIDE - 1, height - 1);
                assert_eq!(regions, vec![all], "{name}, {height} rows");
                peak
            };
            let square = peak_at(MEMORY_SIDE);
            let tall = peak_at(MEMORY_SIDE * TALL_FACTOR);
            assert!(
                square <= bound,
                "{name}: peak {square} is over the bound {bound}"
            );
            assert!(
                tall <= square,
                "{name}: the peak grew with the height, from {square} to {tall}"
            );
        }
    }

    /// Two bars joined at the bottom are one group, though the fill meets them
    /// as two groups until the last row.
    #[test]
    fn a_u_shape_is_one_region() {
        let mut spots = Vec::new();
        for y in 5..=30 {
            spots.push((10, y));
            spots.push((50, y));
        }
        spots.extend((10..=50).map(|x| (x, 30)));
        assert_eq!(regions_of(70, 50, &spots), vec![region(6, 1, 54, 34)]);
    }

    /// Two legs that start from one bar are one group, though the row below the
    /// bar holds two runs.
    #[test]
    fn an_inverted_u_shape_is_one_region() {
        let mut spots = Vec::new();
        for y in 5..=30 {
            spots.push((10, y));
            spots.push((50, y));
        }
        spots.extend((10..=50).map(|x| (x, 5)));
        assert_eq!(regions_of(70, 50, &spots), vec![region(6, 1, 54, 34)]);
    }

    /// A group that ends while another continues keeps its own box.
    #[test]
    fn a_group_that_ends_while_another_continues_keeps_its_box() {
        let mut spots = vec![(10, 10)];
        spots.extend((0..50).map(|y| (50, y)));
        assert_eq!(
            regions_of(70, 50, &spots),
            vec![region(46, 0, 54, 49), region(6, 6, 14, 14)]
        );
    }
}
