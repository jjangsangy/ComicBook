//! Zero-cost geometry and unit newtypes (Phase 4 of the type-safety refactor).
//!
//! These wrap bare `u32`/`u64`/`f64`/`u8` values whose *meaning* — a pixel size, a
//! percentage, a byte count, a JPEG quality — was previously carried only by the
//! parameter name, so a swapped or mis-scaled pair still compiled. Every type here
//! is `Copy` and compiler-erased; none allocates or clones, and none is used inside
//! a per-pixel loop (see `REFACTOR.md` §1.1 and §4).

use anyhow::{bail, Result};

/// A width/height pair in pixels.
///
/// Replaces the bare `(u32, u32)` and adjacent `width`/`height` parameters that a
/// transposition could silently swap (a portrait page resized by its landscape
/// axis). Two fields, so it is a plain `Copy` struct rather than a
/// `#[repr(transparent)]` single-field wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Wrap the `(width, height)` tuple the `image` crate returns.
    pub const fn from_dimensions((width, height): (u32, u32)) -> Self {
        Self { width, height }
    }

    /// Unwrap back to the `(width, height)` tuple for image-crate calls.
    pub const fn to_dimensions(self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// A percentage, normally `0.0`–`100.0` (KCC's autocontrast cutoff reaches `3.0`,
/// and `--preserve-margin` is unvalidated, so no bound is enforced).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Percent(f64);

impl Percent {
    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> f64 {
        self.0
    }

    /// True for "off" (`--preserve-margin 0`, a zero autocontrast cutoff).
    pub fn is_zero(self) -> bool {
        self.0 == 0.0
    }
}

/// A fraction of a whole, normally `0.0`–`1.0`.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fraction(f64);

impl Fraction {
    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> f64 {
        self.0
    }
}

/// A count of pixels.
///
/// Distinct from [`Bytes`] so that a pixel threshold can never be compared against
/// an encoded-size cap (they were both bare `u64`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pixels(u64);

impl Pixels {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// A count of bytes.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bytes(u64);

impl Bytes {
    pub const ZERO: Bytes = Bytes(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl std::ops::Add for Bytes {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl std::iter::Sum for Bytes {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Bytes::ZERO, |total, value| total + value)
    }
}

/// A megabyte count, converted with KCC's binary factor (`1 << 20`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Megabytes(u32);

impl Megabytes {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }

    /// The byte cap KCC's `--target-size` stands for.
    pub const fn to_bytes(self) -> Bytes {
        Bytes(self.0 as u64 * 1024 * 1024)
    }
}

/// A JPEG/WebP quality, validated to KCC's `0..=95` range once in
/// [`crate::ebook::options::Options::resolve`].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality(u8);

impl Quality {
    /// The highest quality KCC accepts.
    pub const MAX: u8 = 95;

    pub fn new(value: u8) -> Result<Self> {
        if value > Self::MAX {
            bail!(
                "JPEG quality must be between 0 and {} (got {value})",
                Self::MAX
            );
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// A half-open pixel rectangle in Pillow's `(left, upper, right, lower)` order
/// (`right`/`lower` exclusive), generic over the coordinate type so the `u32`
/// bounding boxes, the `usize` clip rectangle and the `i64`/`f64` crop rectangles
/// all share one named shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BBox<T> {
    pub left: T,
    pub upper: T,
    pub right: T,
    pub lower: T,
}

impl<T: Copy> BBox<T> {
    pub const fn new(left: T, upper: T, right: T, lower: T) -> Self {
        Self {
            left,
            upper,
            right,
            lower,
        }
    }
}

// `width`/`height` are the span between two edges. They saturate at `0` for the
// integer coordinate types rather than subtracting unchecked: a box whose edges are
// transposed would otherwise panic in a debug build (and wrap in a release one),
// and the crate forbids both (see `REFACTOR.md` §1 rule 6). `f64` has no saturating
// form and cannot panic, so it keeps the plain difference.
macro_rules! bbox_extent {
    ($type:ty) => {
        impl BBox<$type> {
            /// `right - left` (Pillow's width).
            pub fn width(&self) -> $type {
                self.right.saturating_sub(self.left)
            }

            /// `lower - upper` (Pillow's height).
            pub fn height(&self) -> $type {
                self.lower.saturating_sub(self.upper)
            }
        }
    };
}

bbox_extent!(u32);
bbox_extent!(usize);
bbox_extent!(i64);

impl BBox<f64> {
    /// `right - left` (Pillow's width).
    pub fn width(&self) -> f64 {
        self.right - self.left
    }

    /// `lower - upper` (Pillow's height).
    pub fn height(&self) -> f64 {
        self.lower - self.upper
    }
}

impl BBox<u32> {
    /// The number of pixels the box encloses.
    pub fn area(&self) -> u64 {
        u64::from(self.width()) * u64::from(self.height())
    }
}

/// An inclusive index box `(x1, x2, y1, y2)`, grouped by axis.
///
/// Unlike [`BBox`] this mirrors the order KCC's page-number search uses internally
/// (`x2`/`y1` are adjacent), so the two dimensions can be compared without
/// transposing; the fields are inclusive indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexBox {
    pub x1: i64,
    pub x2: i64,
    pub y1: i64,
    pub y2: i64,
}

impl IndexBox {
    pub const fn new(x1: i64, x2: i64, y1: i64, y2: i64) -> Self {
        Self { x1, x2, y1, y2 }
    }

    /// The x span as KCC compares it (`x2 - x1`, deliberately *not* `+ 1`: the
    /// page-number guards use the difference, not the inclusive pixel count).
    pub const fn dx(self) -> i64 {
        self.x2.saturating_sub(self.x1)
    }

    /// The y span (`y2 - y1`), with the same caveat as [`IndexBox::dx`].
    pub const fn dy(self) -> i64 {
        self.y2.saturating_sub(self.y1)
    }

    /// The smallest box containing both.
    pub fn union(self, other: Self) -> Self {
        Self {
            x1: self.x1.min(other.x1),
            x2: self.x2.max(other.x2),
            y1: self.y1.min(other.y1),
            y2: self.y2.max(other.y2),
        }
    }

    /// Whether `other` overlaps `self` once grown by `dx`/`dy` (KCC's
    /// `box_intersect`).
    pub fn intersects(self, other: Self, dx: f64, dy: f64) -> bool {
        !((other.x1 as f64 - dx) > self.x2 as f64
            || (other.x2 as f64 + dx) < self.x1 as f64
            || (other.y1 as f64 - dy) > self.y2 as f64
            || (other.y2 as f64 + dy) < self.y1 as f64)
    }
}

/// A min/max pair of a value domain (Rec. 601 luma or a chroma histogram).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub min: u8,
    pub max: u8,
}

impl Range {
    pub const fn new(min: u8, max: u8) -> Self {
        Self { min, max }
    }

    /// `max - min`; every producer guarantees `min <= max`, so this is exact.
    pub const fn spread(self) -> u8 {
        self.max.saturating_sub(self.min)
    }

    /// Whether the range is flat (or inverted): KCC's `min >= max` guard.
    pub const fn is_degenerate(self) -> bool {
        self.min >= self.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_round_trips_through_dimensions() {
        let size = Size::from_dimensions((40, 30));
        assert_eq!(size, Size::new(40, 30));
        assert_eq!(size.to_dimensions(), (40, 30));
    }

    #[test]
    fn megabytes_use_the_binary_factor() {
        assert_eq!(Megabytes::new(1).to_bytes(), Bytes::new(1_048_576));
        assert_eq!(Megabytes::new(400).to_bytes().raw(), 419_430_400);
    }

    #[test]
    fn bytes_add_and_sum() {
        assert_eq!(Bytes::new(2) + Bytes::new(3), Bytes::new(5));
        let total: Bytes = [Bytes::new(1), Bytes::new(2)].into_iter().sum();
        assert_eq!(total, Bytes::new(3));
    }

    #[test]
    fn quality_rejects_out_of_range_values() {
        assert!(matches!(Quality::new(0), Ok(q) if q.get() == 0));
        assert!(matches!(Quality::new(Quality::MAX), Ok(q) if q.get() == 95));
        assert!(Quality::new(96).is_err());
        assert!(Quality::new(255).is_err());
    }

    #[test]
    fn bbox_reports_width_height_and_area() {
        let bbox = BBox::new(10u32, 5, 20, 15);
        assert_eq!(bbox.width(), 10);
        assert_eq!(bbox.height(), 10);
        assert_eq!(bbox.area(), 100);
    }

    #[test]
    fn index_box_unions_and_intersects() {
        let a = IndexBox::new(0, 4, 0, 0);
        let b = IndexBox::new(0, 4, 1, 1);
        assert!(a.intersects(b, 0.0, 1.0));
        assert_eq!(a.union(b), IndexBox::new(0, 4, 0, 1));
        assert_eq!(a.dx(), 4);
        assert_eq!(a.dy(), 0);
    }

    #[test]
    fn range_spreads_and_detects_degeneracy() {
        assert_eq!(Range::new(1, 200).spread(), 199);
        assert!(Range::new(7, 7).is_degenerate());
        assert!(!Range::new(1, 200).is_degenerate());
    }
}
