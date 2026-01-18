//! Image decoding service.
//!
//! This module handles all image decoding, separated from the preloading logic.
//! It provides a clean interface for decoding images at various quality tiers.

use crate::config::QualityTier;
use crate::slot::ImageData;
use std::fs;
use std::path::Path;
use std::sync::Arc;

/// Decoder for images - handles format detection and quality tiers.
pub struct Decoder {
    /// Supported extensions (lowercase, no dot)
    supported_extensions: Vec<&'static str>,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            supported_extensions: vec!["jpg", "jpeg", "png", "gif", "bmp", "webp"],
        }
    }

    /// Check if a file is supported
    pub fn is_supported(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                let ext_lower = ext.to_lowercase();
                self.supported_extensions.iter().any(|&e| e == ext_lower)
            })
            .unwrap_or(false)
    }

    /// Get supported extensions
    pub fn extensions(&self) -> &[&'static str] {
        &self.supported_extensions
    }

    /// Decode image at specified quality tier
    pub fn decode(&self, path: &Path, quality: QualityTier) -> Option<Arc<ImageData>> {
        let data = fs::read(path).ok()?;

        // Decode to RGBA
        let (rgba, width, height) = if Self::is_jpeg(path) {
            Self::decode_jpeg(&data)?
        } else {
            Self::decode_generic(&data)?
        };

        // Resize for quality tier if needed
        let (target_w, target_h) = quality.target_dimensions(width, height);

        let final_rgba = if target_w == width && target_h == height {
            rgba
        } else {
            Self::resize_bilinear(&rgba, width, height, target_w, target_h)
        };

        Some(Arc::new(ImageData::new(
            final_rgba, target_w, target_h, quality,
        )))
    }

    /// Check if file is JPEG by extension
    fn is_jpeg(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let lower = e.to_lowercase();
                lower == "jpg" || lower == "jpeg"
            })
            .unwrap_or(false)
    }

    /// Decode JPEG using zune-jpeg (fast)
    fn decode_jpeg(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
        // Try zune-jpeg first
        let mut decoder = zune_jpeg::JpegDecoder::new(data);
        if let Ok(pixels) = decoder.decode() {
            if let Some(info) = decoder.info() {
                let rgba = Self::to_rgba(pixels, info.components);
                return Some((rgba, info.width as u32, info.height as u32));
            }
        }

        // Fallback to image crate
        Self::decode_generic(data)
    }

    /// Decode using image crate (generic fallback)
    fn decode_generic(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
        let img = image::load_from_memory(data).ok()?;
        let rgba = img.to_rgba8();
        Some((rgba.as_raw().to_vec(), rgba.width(), rgba.height()))
    }

    /// Convert raw pixels to RGBA
    fn to_rgba(pixels: Vec<u8>, components: u8) -> Vec<u8> {
        match components {
            4 => pixels, // Already RGBA
            3 => pixels
                .chunks_exact(3)
                .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
                .collect(),
            1 => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            _ => pixels,
        }
    }

    /// Resize using bilinear interpolation
    fn resize_bilinear(data: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
        if src_w == dst_w && src_h == dst_h {
            return data.to_vec();
        }

        let src_w = src_w as usize;
        let src_h = src_h as usize;
        let dst_w = dst_w as usize;
        let dst_h = dst_h as usize;

        let mut result = vec![0u8; dst_w * dst_h * 4];

        let x_ratio = (src_w as f64 - 1.0) / dst_w.max(1) as f64;
        let y_ratio = (src_h as f64 - 1.0) / dst_h.max(1) as f64;

        for y in 0..dst_h {
            let src_y = y as f64 * y_ratio;
            let y0 = src_y.floor() as usize;
            let y1 = (y0 + 1).min(src_h - 1);
            let y_frac = src_y - y0 as f64;

            for x in 0..dst_w {
                let src_x = x as f64 * x_ratio;
                let x0 = src_x.floor() as usize;
                let x1 = (x0 + 1).min(src_w - 1);
                let x_frac = src_x - x0 as f64;

                let idx00 = (y0 * src_w + x0) * 4;
                let idx01 = (y0 * src_w + x1) * 4;
                let idx10 = (y1 * src_w + x0) * 4;
                let idx11 = (y1 * src_w + x1) * 4;
                let dst_idx = (y * dst_w + x) * 4;

                for c in 0..4 {
                    let v00 = data.get(idx00 + c).copied().unwrap_or(0) as f64;
                    let v01 = data.get(idx01 + c).copied().unwrap_or(0) as f64;
                    let v10 = data.get(idx10 + c).copied().unwrap_or(0) as f64;
                    let v11 = data.get(idx11 + c).copied().unwrap_or(0) as f64;

                    let v0 = v00 * (1.0 - x_frac) + v01 * x_frac;
                    let v1 = v10 * (1.0 - x_frac) + v11 * x_frac;
                    let v = v0 * (1.0 - y_frac) + v1 * y_frac;

                    result[dst_idx + c] = v.round() as u8;
                }
            }
        }

        result
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Scan a directory for supported images
pub fn scan_directory(dir: &Path, decoder: &Decoder) -> Vec<std::path::PathBuf> {
    let mut images: Vec<_> = walkdir::WalkDir::new(dir)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| decoder.is_supported(e.path()))
        .map(|e| e.path().to_path_buf())
        .collect();

    images.sort();
    images
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supported_extensions() {
        let decoder = Decoder::new();

        assert!(decoder.is_supported(Path::new("test.jpg")));
        assert!(decoder.is_supported(Path::new("test.JPEG")));
        assert!(decoder.is_supported(Path::new("test.png")));
        assert!(!decoder.is_supported(Path::new("test.txt")));
        assert!(!decoder.is_supported(Path::new("test")));
    }

    #[test]
    fn test_resize() {
        // 2x2 image, all red
        let src = vec![
            255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
        ];
        let dst = Decoder::resize_bilinear(&src, 2, 2, 4, 4);

        // Should be 4x4, still mostly red
        assert_eq!(dst.len(), 4 * 4 * 4);
        // First pixel should be red
        assert_eq!(&dst[0..4], &[255, 0, 0, 255]);
    }
}
