//! Pixel sampling over a captured frame.
//!
//! The render tests assert on regions of the real frame against the committed
//! theme tokens, so a layout regression fails instead of slipping past a
//! colour-count check. Everything here takes *logical* rects and scales them to
//! device pixels.

use std::collections::HashSet;

/// One captured off-screen frame plus its colour count.
pub(crate) struct Frame {
    /// The raw RGBA image.
    pub(crate) image: image::RgbaImage,
    /// Distinct colours, reported in the log line.
    pub(crate) colours: usize,
    /// Device pixels per logical pixel.
    pub(crate) scale: u32,
}

impl Frame {
    /// Device-pixel size.
    pub(crate) fn dims(&self) -> (u32, u32) {
        self.image.dimensions()
    }

    /// One pixel, clamped to the frame.
    pub(crate) fn px(&self, x: u32, y: u32) -> [u8; 4] {
        let (w, h) = self.dims();
        self.image.get_pixel(x.min(w - 1), y.min(h - 1)).0
    }

    /// Scale a logical rect to device pixels, clamped to the frame.
    pub(crate) fn rect(&self, rect: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
        let (w, h) = self.dims();
        let s = self.scale;
        (
            (rect.0 * s).min(w),
            (rect.1 * s).min(h),
            (rect.2 * s).min(w),
            (rect.3 * s).min(h),
        )
    }

    /// Pixels within `tol` of `rgb` inside the logical rect.
    pub(crate) fn count_near(&self, rect: (u32, u32, u32, u32), rgb: [u8; 3], tol: u8) -> usize {
        let (x0, y0, x1, y1) = self.rect(rect);
        let mut hits = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = self.px(x, y);
                if p[0].abs_diff(rgb[0]) <= tol
                    && p[1].abs_diff(rgb[1]) <= tol
                    && p[2].abs_diff(rgb[2]) <= tol
                {
                    hits += 1;
                }
            }
        }
        hits
    }

    /// Pixels further than `tol` from `rgb` inside the logical rect.
    pub(crate) fn count_not_near(
        &self,
        rect: (u32, u32, u32, u32),
        rgb: [u8; 3],
        tol: u8,
    ) -> usize {
        let (x0, y0, x1, y1) = self.rect(rect);
        let mut hits = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = self.px(x, y);
                if p[0].abs_diff(rgb[0]) > tol
                    || p[1].abs_diff(rgb[1]) > tol
                    || p[2].abs_diff(rgb[2]) > tol
                {
                    hits += 1;
                }
            }
        }
        hits
    }

    /// Pixels that differ from `base` and trend toward `tint`.
    pub(crate) fn count_tint(
        &self,
        rect: (u32, u32, u32, u32),
        base: [u8; 3],
        tint: [u8; 3],
    ) -> usize {
        let (x0, y0, x1, y1) = self.rect(rect);
        let mut hits = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = self.px(x, y);
                let near_base = p[0].abs_diff(base[0]) <= 6
                    && p[1].abs_diff(base[1]) <= 6
                    && p[2].abs_diff(base[2]) <= 6;
                let toward_tint = (p[0] as i16 - base[0] as i16).signum()
                    == (tint[0] as i16 - base[0] as i16).signum()
                    && (p[1] as i16 - base[1] as i16).signum()
                        == (tint[1] as i16 - base[1] as i16).signum();
                if !near_base && toward_tint {
                    hits += 1;
                }
            }
        }
        hits
    }

    /// Distinct colours in the frame.
    pub(crate) fn count_colours(image: &image::RgbaImage) -> usize {
        let mut colours = HashSet::new();
        for pixel in image.pixels() {
            colours.insert(pixel.0);
        }
        colours.len()
    }

    /// Perceptual luminance averaged over the whole frame (sRGB weights).
    ///
    /// A light theme renders to a higher mean than its dark counterpart; the
    /// both-theme matrix test uses this to prove the two are actually different
    /// screens, not the same frame drawn twice.
    pub(crate) fn mean_luminance(&self) -> f64 {
        let (w, h) = self.dims();
        let mut sum = 0.0;
        for y in 0..h {
            for x in 0..w {
                let p = self.px(x, y);
                sum +=
                    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]);
            }
        }
        sum / (f64::from(w) * f64::from(h))
    }
}
