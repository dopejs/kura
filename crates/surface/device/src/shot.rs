//! Screenshots and the coordinate space the agent works in
//! (docs/harness/remote-computer-use.md §6).
//!
//! The display is captured at native resolution, never-touch windows are
//! blacked out, and the image is scaled so its long edge is at most
//! [`MAX_EDGE`] and encoded as JPEG, stepping quality down until it fits
//! [`BYTE_BUDGET`]. The agent's coordinates are in that scaled space; the
//! device maps them back to display points before it moves the pointer.

use base64::Engine as _;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, RgbImage, Rgba, RgbaImage};
use kura_device_protocol::Screenshot;

pub const MAX_EDGE: u32 = 1280;
/// A screenshot over this is re-encoded at lower quality (§11, slow uplink).
pub const BYTE_BUDGET: usize = 300 * 1024;
const QUALITIES: [u8; 4] = [80, 65, 50, 40];

/// A rectangle in some pixel or point space, origin top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    #[must_use]
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// The size of a screenshot of a `width`×`height` display: the long edge at
/// most [`MAX_EDGE`], aspect kept, never upscaled.
#[must_use]
pub fn fit(width: u32, height: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= MAX_EDGE || long == 0 {
        return (width, height);
    }
    let scale = f64::from(MAX_EDGE) / f64::from(long);
    let scaled = |v: u32| ((f64::from(v) * scale).round() as u32).max(1);
    (scaled(width), scaled(height))
}

/// Maps a point in screenshot space to display points, clamped onto the
/// display so an off-by-one from the model never lands off screen.
#[must_use]
pub fn to_display(x: u32, y: u32, shot: (u32, u32), display: Rect) -> (f64, f64) {
    let map = |v: u32, shot_len: u32, origin: f64, len: f64| {
        let v = f64::from(v) * len / f64::from(shot_len.max(1));
        origin + v.clamp(0.0, (len - 1.0).max(0.0))
    };
    (
        map(x, shot.0, display.x, display.width),
        map(y, shot.1, display.y, display.height),
    )
}

/// Blacks out `hidden` (in the image's own pixels), scales to `size` (the
/// space the agent's coordinates are mapped from, so the two can never
/// disagree), and encodes.
pub fn encode(
    mut native: RgbaImage,
    hidden: &[Rect],
    size: (u32, u32),
) -> Result<Screenshot, String> {
    let (w, h) = (native.width(), native.height());
    for rect in hidden {
        let x0 = rect.x.max(0.0).floor() as u32;
        let y0 = rect.y.max(0.0).floor() as u32;
        let x1 = ((rect.x + rect.width).ceil().max(0.0) as u32).min(w);
        let y1 = ((rect.y + rect.height).ceil().max(0.0) as u32).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                native.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
    }
    let (sw, sh) = size;
    let scaled = if (sw, sh) == (w, h) {
        native
    } else {
        image::imageops::resize(&native, sw, sh, FilterType::Triangle)
    };
    let rgb = DynamicImage::ImageRgba8(scaled).into_rgb8();
    let (bytes, _) = compress(&rgb, BYTE_BUDGET)?;
    Ok(Screenshot {
        media_type: "image/jpeg".into(),
        width: sw,
        height: sh,
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

/// Encodes at the first quality that fits `budget`, or the lowest one.
/// Returns the bytes and the quality used.
fn compress(rgb: &RgbImage, budget: usize) -> Result<(Vec<u8>, u8), String> {
    let mut bytes = Vec::new();
    let mut used = QUALITIES[0];
    for quality in QUALITIES {
        bytes.clear();
        used = quality;
        JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(rgb)
            .map_err(|err| format!("encode: {err}"))?;
        if bytes.len() <= budget {
            break;
        }
    }
    Ok((bytes, used))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retina_fits_in_1280_and_small_screens_are_kept() {
        assert_eq!(fit(2880, 1800), (1280, 800));
        assert_eq!(fit(1800, 2880), (800, 1280));
        assert_eq!(fit(3024, 1964), (1280, 831));
        assert_eq!(fit(1280, 720), (1280, 720));
        assert_eq!(fit(800, 600), (800, 600));
    }

    #[test]
    fn coordinates_map_back_to_points_and_clamp() {
        // 1280×800 screenshot of a 1440×900-point display.
        let display = Rect {
            x: 0.0,
            y: 0.0,
            width: 1440.0,
            height: 900.0,
        };
        assert_eq!(to_display(640, 400, (1280, 800), display), (720.0, 450.0));
        assert_eq!(to_display(0, 0, (1280, 800), display), (0.0, 0.0));
        // Past the edge lands on the last point, not off screen.
        assert_eq!(
            to_display(5000, 5000, (1280, 800), display),
            (1439.0, 899.0)
        );
    }

    #[test]
    fn hidden_windows_are_black_and_the_shot_is_scaled() {
        let native = RgbaImage::from_pixel(2560, 1600, Rgba([255, 255, 255, 255]));
        let hidden = [Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 1600.0,
        }];
        let shot = encode(native, &hidden, fit(2560, 1600)).unwrap();
        assert_eq!((shot.width, shot.height), (1280, 800));
        assert_eq!(shot.media_type, "image/jpeg");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&shot.data)
            .unwrap();
        assert!(bytes.len() <= BYTE_BUDGET);
        let decoded = image::load_from_memory(&bytes).unwrap().into_rgb8();
        // Left half hidden, right half visible (JPEG is lossy: allow slack).
        assert!(decoded.get_pixel(300, 400).0.iter().all(|v| *v < 20));
        assert!(decoded.get_pixel(1000, 400).0.iter().all(|v| *v > 235));
    }

    #[test]
    fn quality_steps_down_until_the_shot_fits() {
        // Noise compresses badly, so quality visibly changes the size.
        let mut seed = 0x2545_f491_u32;
        let rgb = RgbImage::from_fn(320, 200, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let [a, b, c, _] = seed.to_le_bytes();
            image::Rgb([a, b, c])
        });
        let (q80, used) = compress(&rgb, usize::MAX).unwrap();
        assert_eq!(used, 80);
        let (smaller, used) = compress(&rgb, q80.len() - 1).unwrap();
        assert!(used < 80 && smaller.len() < q80.len());
        // Nothing fits: the lowest quality is sent rather than nothing.
        let (_, used) = compress(&rgb, 1).unwrap();
        assert_eq!(used, *QUALITIES.last().unwrap());
    }
}
