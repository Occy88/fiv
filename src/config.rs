//! Configuration - all tunable parameters in one place.
//!
//! This module provides a data-driven configuration system where all magic numbers
//! and behavioral parameters are centralized. This makes tuning easy and prevents
//! scattered constants throughout the codebase.

use std::time::Duration;
use sysinfo::System;

/// Master configuration for the viewer.
/// All behavioral parameters are here - no magic numbers elsewhere.
#[derive(Debug, Clone)]
pub struct Config {
    /// Memory management
    pub memory: MemoryConfig,
    /// Input handling
    pub input: InputConfig,
    /// Preloading strategy
    pub preload: PreloadConfig,
    /// Rendering
    pub render: RenderConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            memory: MemoryConfig::default(),
            input: InputConfig::default(),
            preload: PreloadConfig::default(),
            render: RenderConfig::default(),
        }
    }
}

/// Memory budget configuration
#[derive(Debug, Clone)]
pub struct MemoryConfig {
    /// Percentage of system RAM to use (0.0 - 1.0)
    pub budget_ratio: f64,
    /// Minimum budget in bytes
    pub min_budget: usize,
    /// Maximum budget in bytes
    pub max_budget: usize,
}

impl MemoryConfig {
    /// Calculate the actual memory budget in bytes
    pub fn calculate_budget(&self) -> usize {
        let mut sys = System::new_all();
        sys.refresh_memory();

        let total_ram = sys.total_memory() as usize;
        let budget = (total_ram as f64 * self.budget_ratio) as usize;

        budget.clamp(self.min_budget, self.max_budget)
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            budget_ratio: 0.10, // 10% of RAM
            min_budget: 100 * 1024 * 1024,  // 100 MB
            max_budget: 4 * 1024 * 1024 * 1024, // 4 GB
        }
    }
}

/// Input handling configuration
#[derive(Debug, Clone)]
pub struct InputConfig {
    /// How long to hold before entering repeat mode
    /// Below this threshold, release triggers a single click
    pub hold_threshold: Duration,
    /// Interval between repeats while key is held (after hold_threshold)
    pub repeat_interval: Duration,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            // Hold for 150ms before repeat mode kicks in
            hold_threshold: Duration::from_millis(150),
            // ~16 images per second when holding
            repeat_interval: Duration::from_millis(60),
        }
    }
}

/// Preloading strategy configuration
#[derive(Debug, Clone)]
pub struct PreloadConfig {
    /// Number of images to preload ahead when moving forward
    pub ahead_forward: usize,
    /// Number of images to preload behind when moving forward
    pub behind_forward: usize,
    /// Number of images to preload ahead when moving backward
    pub ahead_backward: usize,
    /// Number of images to preload behind when moving backward
    pub behind_backward: usize,
    /// Number of images to preload in each direction when direction unknown
    pub symmetric_range: usize,
    /// How many images to load at full quality (nearest to current)
    pub full_quality_count: usize,
    /// How many images to load at preview quality (after full)
    pub preview_quality_count: usize,
    /// How long to wait when idle before checking for work
    pub idle_poll_interval: Duration,
    /// Maximum parallel decode tasks (0 = use all cores)
    pub max_parallel_tasks: usize,
}

impl Default for PreloadConfig {
    fn default() -> Self {
        Self {
            // When moving forward: heavily bias ahead
            ahead_forward: 30,
            behind_forward: 3,
            // When moving backward: heavily bias behind
            ahead_backward: 3,
            behind_backward: 30,
            // When direction unknown: symmetric
            symmetric_range: 15,
            // Quality tiers by distance
            full_quality_count: 5,     // ±5 images at full quality
            preview_quality_count: 10, // Next ±10 at preview
            // Rest at thumbnail
            idle_poll_interval: Duration::from_millis(1),
            max_parallel_tasks: 0, // Use all cores
        }
    }
}

impl PreloadConfig {
    /// Get preload range based on direction
    pub fn range_for_direction(&self, direction: crate::state::Direction) -> (usize, usize) {
        use crate::state::Direction;
        match direction {
            Direction::Forward => (self.ahead_forward, self.behind_forward),
            Direction::Backward => (self.ahead_backward, self.behind_backward),
            Direction::Unknown => (self.symmetric_range, self.symmetric_range),
        }
    }

    /// Get quality tier for distance from current
    pub fn quality_for_distance(&self, distance: usize) -> QualityTier {
        if distance <= self.full_quality_count {
            QualityTier::Full
        } else if distance <= self.full_quality_count + self.preview_quality_count {
            QualityTier::Preview
        } else {
            QualityTier::Thumbnail
        }
    }

    /// Total range (for eviction)
    pub fn total_range(&self) -> usize {
        self.ahead_forward.max(self.behind_backward) + 5
    }
}


/// Rendering configuration
#[derive(Debug, Clone)]
pub struct RenderConfig {
    /// Default window width
    pub default_width: u32,
    /// Default window height
    pub default_height: u32,
    /// Background color (RGBA)
    pub background_color: [u8; 4],
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            default_width: 1280,
            default_height: 720,
            background_color: [0, 0, 0, 255], // Black
        }
    }
}

/// Quality tier for image loading.
/// Ordered from lowest to highest quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QualityTier {
    /// Thumbnail - 256px max dimension
    Thumbnail,
    /// Preview - 1024px max dimension
    Preview,
    /// Full - original resolution
    Full,
}

impl QualityTier {
    /// Maximum dimension for this tier (None = unlimited)
    pub const fn max_dimension(self) -> Option<u32> {
        match self {
            Self::Thumbnail => Some(256),
            Self::Preview => Some(1024),
            Self::Full => None,
        }
    }

    /// Calculate target dimensions maintaining aspect ratio
    pub fn target_dimensions(self, width: u32, height: u32) -> (u32, u32) {
        match self.max_dimension() {
            None => (width, height),
            Some(max_dim) => {
                let max_original = width.max(height);
                if max_original <= max_dim {
                    (width, height)
                } else {
                    let scale = max_dim as f64 / max_original as f64;
                    let new_w = (width as f64 * scale).round() as u32;
                    let new_h = (height as f64 * scale).round() as u32;
                    (new_w.max(1), new_h.max(1))
                }
            }
        }
    }

    /// Estimate memory for RGBA image at this tier
    pub fn estimate_memory(self, width: u32, height: u32) -> usize {
        let (w, h) = self.target_dimensions(width, height);
        (w as usize) * (h as usize) * 4
    }

    /// Iterator from lowest to highest quality
    pub fn all() -> impl Iterator<Item = Self> {
        [Self::Thumbnail, Self::Preview, Self::Full].into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quality_for_distance() {
        let config = PreloadConfig::default();

        // Close images should be full quality
        assert_eq!(config.quality_for_distance(0), QualityTier::Full);
        assert_eq!(config.quality_for_distance(5), QualityTier::Full);

        // Medium distance should be preview
        assert_eq!(config.quality_for_distance(10), QualityTier::Preview);

        // Far images should be thumbnail
        assert_eq!(config.quality_for_distance(20), QualityTier::Thumbnail);
    }

    #[test]
    fn test_direction_ranges() {
        let config = PreloadConfig::default();

        // Forward should bias ahead
        let (ahead, behind) = config.range_for_direction(crate::state::Direction::Forward);
        assert!(ahead > behind);

        // Backward should bias behind
        let (ahead, behind) = config.range_for_direction(crate::state::Direction::Backward);
        assert!(behind > ahead);
    }

    #[test]
    fn test_tier_dimensions() {
        // Thumbnail should scale down large images
        let (w, h) = QualityTier::Thumbnail.target_dimensions(1920, 1080);
        assert!(w <= 256 && h <= 256);

        // Full should preserve dimensions
        let (w, h) = QualityTier::Full.target_dimensions(1920, 1080);
        assert_eq!((w, h), (1920, 1080));

        // Small images should not be upscaled
        let (w, h) = QualityTier::Full.target_dimensions(100, 100);
        assert_eq!((w, h), (100, 100));
    }
}
