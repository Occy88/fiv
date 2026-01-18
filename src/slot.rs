//! Lock-free image slot for zero-contention access.
//!
//! The ImageSlot is the core primitive for achieving immediate feedback.
//! It uses atomic operations to allow the main thread to read image data
//! without ever blocking, while background threads can upgrade the data
//! at any time.
//!
//! Key invariant: reads never block, writes are atomic swaps.

use crate::config::QualityTier;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::Arc;

/// Decoded image data ready for display.
/// This is the "raw data" that the viewer renders from.
#[derive(Debug)]
pub struct ImageData {
    /// RGBA pixel data
    pub pixels: Vec<u8>,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Quality tier this was decoded at
    pub quality: QualityTier,
}

impl ImageData {
    pub fn new(pixels: Vec<u8>, width: u32, height: u32, quality: QualityTier) -> Self {
        Self {
            pixels,
            width,
            height,
            quality,
        }
    }

    /// Memory size in bytes
    #[inline]
    pub fn memory_size(&self) -> usize {
        self.pixels.len()
    }
}

/// Immutable metadata about an image (derived from file/headers).
#[derive(Debug, Clone)]
pub struct ImageMeta {
    /// Path to the image file
    pub path: PathBuf,
    /// Original width (from headers, before any scaling)
    pub original_width: u32,
    /// Original height (from headers, before any scaling)
    pub original_height: u32,
}

impl ImageMeta {
    pub fn new(path: PathBuf, width: u32, height: u32) -> Self {
        Self {
            path,
            original_width: width,
            original_height: height,
        }
    }

    /// Estimate memory for full resolution RGBA
    #[inline]
    pub fn full_memory_estimate(&self) -> usize {
        (self.original_width as usize) * (self.original_height as usize) * 4
    }

    /// Estimate memory for a specific tier
    #[inline]
    pub fn memory_for_tier(&self, tier: QualityTier) -> usize {
        tier.estimate_memory(self.original_width, self.original_height)
    }
}

/// A lock-free slot holding image data.
///
/// The slot can be in one of three states:
/// - Empty: no data loaded yet
/// - Loading: data is being decoded (optional intermediate state)
/// - Ready: data is available for rendering
///
/// The main thread reads via `read()` which never blocks.
/// Background threads write via `upgrade()` which atomically swaps in new data.
pub struct ImageSlot {
    /// Pointer to current image data (null if empty)
    /// Uses raw pointer for lock-free atomic operations
    data_ptr: AtomicPtr<ImageData>,

    /// Metadata about this image (immutable after creation)
    pub meta: ImageMeta,

    /// Generation counter - incremented on each update
    /// Used by preloader to detect stale work
    generation: AtomicU64,
}

impl ImageSlot {
    /// Create a new empty slot with metadata
    pub fn new(meta: ImageMeta) -> Self {
        Self {
            data_ptr: AtomicPtr::new(ptr::null_mut()),
            meta,
            generation: AtomicU64::new(0),
        }
    }

    /// Read current image data (lock-free).
    ///
    /// Returns None if no data is loaded yet.
    /// The returned Arc keeps the data alive even if the slot is upgraded.
    #[inline]
    pub fn read(&self) -> Option<Arc<ImageData>> {
        let ptr = self.data_ptr.load(Ordering::Acquire);
        if ptr.is_null() {
            return None;
        }

        // SAFETY: If ptr is non-null, it points to a valid Arc allocation.
        // We increment the refcount by cloning, so the data stays alive.
        // The original Arc in the slot also keeps it alive.
        unsafe {
            // Reconstruct Arc without taking ownership (just increment refcount)
            Arc::increment_strong_count(ptr);
            Some(Arc::from_raw(ptr))
        }
    }

    /// Check current quality tier without cloning the data
    #[inline]
    pub fn current_quality(&self) -> Option<QualityTier> {
        let ptr = self.data_ptr.load(Ordering::Acquire);
        if ptr.is_null() {
            return None;
        }
        // SAFETY: ptr is valid if non-null
        unsafe { Some((*ptr).quality) }
    }

    /// Check if this slot has data at or above the given quality
    #[inline]
    pub fn has_quality(&self, min_quality: QualityTier) -> bool {
        self.current_quality()
            .map(|q| q >= min_quality)
            .unwrap_or(false)
    }

    /// Check if slot is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data_ptr.load(Ordering::Acquire).is_null()
    }

    /// Upgrade the slot with new image data (lock-free).
    ///
    /// This atomically swaps in the new data. If there was previous data,
    /// it will be dropped when all references to it are gone.
    ///
    /// Returns true if the upgrade was performed (new quality > old quality).
    pub fn upgrade(&self, new_data: Arc<ImageData>) -> bool {
        // Check if this is actually an upgrade
        if let Some(current_quality) = self.current_quality() {
            if new_data.quality <= current_quality {
                // Not an upgrade, skip
                return false;
            }
        }

        // Convert Arc to raw pointer (transfers ownership to the pointer)
        let new_ptr = Arc::into_raw(new_data) as *mut ImageData;

        // Atomically swap in the new pointer
        let old_ptr = self.data_ptr.swap(new_ptr, Ordering::AcqRel);

        // Increment generation to signal change
        self.generation.fetch_add(1, Ordering::Release);

        // Drop old data if it existed
        if !old_ptr.is_null() {
            // SAFETY: old_ptr was a valid Arc that we owned
            unsafe {
                drop(Arc::from_raw(old_ptr));
            }
        }

        true
    }

    /// Force-set new data regardless of quality (used for eviction/replacement)
    pub fn set(&self, new_data: Option<Arc<ImageData>>) {
        let new_ptr = new_data
            .map(|d| Arc::into_raw(d) as *mut ImageData)
            .unwrap_or(ptr::null_mut());

        let old_ptr = self.data_ptr.swap(new_ptr, Ordering::AcqRel);
        self.generation.fetch_add(1, Ordering::Release);

        if !old_ptr.is_null() {
            unsafe {
                drop(Arc::from_raw(old_ptr));
            }
        }
    }

    /// Clear the slot (release data)
    pub fn clear(&self) {
        self.set(None);
    }

    /// Get current generation (for change detection)
    #[inline]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Estimate memory currently used by this slot
    pub fn memory_used(&self) -> usize {
        let ptr = self.data_ptr.load(Ordering::Acquire);
        if ptr.is_null() {
            0
        } else {
            // SAFETY: ptr is valid if non-null
            unsafe { (*ptr).memory_size() }
        }
    }
}

impl Drop for ImageSlot {
    fn drop(&mut self) {
        // Clean up any remaining data
        let ptr = self.data_ptr.load(Ordering::Acquire);
        if !ptr.is_null() {
            unsafe {
                drop(Arc::from_raw(ptr));
            }
        }
    }
}

// SAFETY: ImageSlot uses atomic operations for all mutable state.
// The Arc<ImageData> is safely shared between threads.
unsafe impl Send for ImageSlot {}
unsafe impl Sync for ImageSlot {}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_data(quality: QualityTier) -> Arc<ImageData> {
        Arc::new(ImageData::new(vec![0u8; 100], 10, 10, quality))
    }

    #[test]
    fn test_empty_slot() {
        let meta = ImageMeta::new(PathBuf::from("test.jpg"), 100, 100);
        let slot = ImageSlot::new(meta);

        assert!(slot.is_empty());
        assert!(slot.read().is_none());
        assert!(slot.current_quality().is_none());
    }

    #[test]
    fn test_upgrade() {
        let meta = ImageMeta::new(PathBuf::from("test.jpg"), 100, 100);
        let slot = ImageSlot::new(meta);

        // First data
        let thumb = make_test_data(QualityTier::Thumbnail);
        assert!(slot.upgrade(thumb));
        assert_eq!(slot.current_quality(), Some(QualityTier::Thumbnail));

        // Upgrade to higher quality
        let full = make_test_data(QualityTier::Full);
        assert!(slot.upgrade(full));
        assert_eq!(slot.current_quality(), Some(QualityTier::Full));

        // Can't downgrade
        let thumb2 = make_test_data(QualityTier::Thumbnail);
        assert!(!slot.upgrade(thumb2)); // Returns false
        assert_eq!(slot.current_quality(), Some(QualityTier::Full)); // Still full
    }

    #[test]
    fn test_read_returns_clone() {
        let meta = ImageMeta::new(PathBuf::from("test.jpg"), 100, 100);
        let slot = ImageSlot::new(meta);

        let data = make_test_data(QualityTier::Full);
        slot.upgrade(data);

        // Read twice - should get independent Arcs
        let read1 = slot.read().unwrap();
        let read2 = slot.read().unwrap();

        assert_eq!(Arc::strong_count(&read1), 3); // slot + read1 + read2
        drop(read2);
        assert_eq!(Arc::strong_count(&read1), 2); // slot + read1
    }

    #[test]
    fn test_generation_increments() {
        let meta = ImageMeta::new(PathBuf::from("test.jpg"), 100, 100);
        let slot = ImageSlot::new(meta);

        let gen0 = slot.generation();

        slot.upgrade(make_test_data(QualityTier::Thumbnail));
        let gen1 = slot.generation();
        assert!(gen1 > gen0);

        slot.upgrade(make_test_data(QualityTier::Full));
        let gen2 = slot.generation();
        assert!(gen2 > gen1);
    }
}
