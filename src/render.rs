//! Pure rendering functions.
//!
//! Rendering is a pure function: given state and data, produce pixels.
//! No side effects, no locks, no mutations to shared state.
//! This is the "view" in model-view separation.

use crate::config::QualityTier;
use crate::slot::ImageData;
use std::sync::Arc;

/// Result of a render operation
pub struct RenderResult {
    /// Quality tier of rendered image (None if no image available)
    pub quality: Option<QualityTier>,
}

/// Render an image to a pixel buffer.
///
/// This is a pure function with no side effects. It reads from `image_data`
/// and writes to `frame`, using `window_size` to determine scaling.
///
/// # Arguments
/// * `image_data` - The image to render (None shows black screen)
/// * `frame` - Output pixel buffer (RGBA, row-major)
/// * `window_width` - Window width in pixels
/// * `window_height` - Window height in pixels
/// * `background` - Background color (RGBA)
///
/// # Returns
/// RenderResult indicating success and quality
pub fn render_image(
    image_data: Option<&Arc<ImageData>>,
    frame: &mut [u8],
    window_width: u32,
    window_height: u32,
    background: [u8; 4],
) -> RenderResult {
    // Clear to background
    clear_frame(frame, background);

    let img = match image_data {
        Some(data) => data,
        None => {
            return RenderResult { quality: None };
        }
    };

    let win_w = window_width as usize;
    let win_h = window_height as usize;
    let img_w = img.width as usize;
    let img_h = img.height as usize;

    if win_w == 0 || win_h == 0 || img_w == 0 || img_h == 0 {
        return RenderResult { quality: Some(img.quality) };
    }

    // Calculate scaling to fit window while maintaining aspect ratio (letterbox)
    let scale_x = win_w as f64 / img_w as f64;
    let scale_y = win_h as f64 / img_h as f64;
    let scale = scale_x.min(scale_y);

    let display_w = (img_w as f64 * scale) as usize;
    let display_h = (img_h as f64 * scale) as usize;

    // Center in window
    let offset_x = (win_w - display_w) / 2;
    let offset_y = (win_h - display_h) / 2;

    // Blit with nearest-neighbor scaling
    blit_scaled(
        &img.pixels,
        img_w,
        img_h,
        frame,
        win_w,
        offset_x,
        offset_y,
        display_w,
        display_h,
    );

    RenderResult { quality: Some(img.quality) }
}

/// Clear frame buffer to a solid color
#[inline]
pub fn clear_frame(frame: &mut [u8], color: [u8; 4]) {
    // Fast path for black (most common)
    if color == [0, 0, 0, 255] {
        frame.fill(0);
        // Set alpha to 255 for every 4th byte
        for chunk in frame.chunks_exact_mut(4) {
            chunk[3] = 255;
        }
    } else {
        for chunk in frame.chunks_exact_mut(4) {
            chunk.copy_from_slice(&color);
        }
    }
}

/// Blit source image to destination with nearest-neighbor scaling.
#[inline]
#[allow(clippy::too_many_arguments)]
fn blit_scaled(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_x: usize,
    dst_y: usize,
    dst_w: usize,
    dst_h: usize,
) {
    if dst_w == 0 || dst_h == 0 {
        return;
    }

    // Precompute source X coordinates for each destination X
    let x_scale = src_w as f64 / dst_w as f64;
    let y_scale = src_h as f64 / dst_h as f64;

    // Process row by row
    for dy in 0..dst_h {
        let src_y = ((dy as f64 * y_scale) as usize).min(src_h - 1);
        let src_row_offset = src_y * src_w * 4;
        let dst_row_offset = ((dst_y + dy) * dst_stride + dst_x) * 4;

        for dx in 0..dst_w {
            let src_x = ((dx as f64 * x_scale) as usize).min(src_w - 1);
            let src_idx = src_row_offset + src_x * 4;
            let dst_idx = dst_row_offset + dx * 4;

            if dst_idx + 3 < dst.len() && src_idx + 3 < src.len() {
                dst[dst_idx] = src[src_idx];
                dst[dst_idx + 1] = src[src_idx + 1];
                dst[dst_idx + 2] = src[src_idx + 2];
                dst[dst_idx + 3] = 255; // Force opaque
            }
        }
    }
}

/// Blit with bilinear interpolation (higher quality, slower)
#[allow(dead_code, clippy::too_many_arguments)]
pub fn blit_bilinear(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_x: usize,
    dst_y: usize,
    dst_w: usize,
    dst_h: usize,
) {
    if dst_w == 0 || dst_h == 0 || src_w < 2 || src_h < 2 {
        blit_scaled(src, src_w, src_h, dst, dst_stride, dst_x, dst_y, dst_w, dst_h);
        return;
    }

    let x_ratio = (src_w as f64 - 1.0) / dst_w.max(1) as f64;
    let y_ratio = (src_h as f64 - 1.0) / dst_h.max(1) as f64;

    for dy in 0..dst_h {
        let src_y = dy as f64 * y_ratio;
        let y0 = src_y.floor() as usize;
        let y1 = (y0 + 1).min(src_h - 1);
        let y_frac = src_y - y0 as f64;

        let dst_row_offset = ((dst_y + dy) * dst_stride + dst_x) * 4;

        for dx in 0..dst_w {
            let src_x = dx as f64 * x_ratio;
            let x0 = src_x.floor() as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let x_frac = src_x - x0 as f64;

            let idx00 = (y0 * src_w + x0) * 4;
            let idx01 = (y0 * src_w + x1) * 4;
            let idx10 = (y1 * src_w + x0) * 4;
            let idx11 = (y1 * src_w + x1) * 4;

            let dst_idx = dst_row_offset + dx * 4;

            if dst_idx + 3 < dst.len() {
                for c in 0..3 {
                    let v00 = src[idx00 + c] as f64;
                    let v01 = src[idx01 + c] as f64;
                    let v10 = src[idx10 + c] as f64;
                    let v11 = src[idx11 + c] as f64;

                    let v0 = v00 * (1.0 - x_frac) + v01 * x_frac;
                    let v1 = v10 * (1.0 - x_frac) + v11 * x_frac;
                    let v = v0 * (1.0 - y_frac) + v1 * y_frac;

                    dst[dst_idx + c] = v.round() as u8;
                }
                dst[dst_idx + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_image(w: u32, h: u32) -> Arc<ImageData> {
        let pixels = vec![128u8; (w * h * 4) as usize];
        Arc::new(ImageData::new(pixels, w, h, QualityTier::Full))
    }

    #[test]
    fn test_render_empty() {
        let mut frame = vec![0u8; 100 * 100 * 4];
        let result = render_image(None, &mut frame, 100, 100, [0, 0, 0, 255]);

        assert!(result.quality.is_none());
    }

    #[test]
    fn test_render_image() {
        let img = make_test_image(50, 50);
        let mut frame = vec![0u8; 100 * 100 * 4];

        let result = render_image(Some(&img), &mut frame, 100, 100, [0, 0, 0, 255]);

        assert_eq!(result.quality, Some(QualityTier::Full));
    }

    #[test]
    fn test_clear_frame() {
        let mut frame = vec![0u8; 16];
        clear_frame(&mut frame, [255, 0, 0, 255]);

        assert_eq!(&frame[0..4], &[255, 0, 0, 255]);
        assert_eq!(&frame[4..8], &[255, 0, 0, 255]);
    }
}
