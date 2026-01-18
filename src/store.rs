//! Image store - manages collection of image slots with memory budget.
//!
//! The ImageStore is the "window over raw data" - it holds all image slots
//! and manages memory allocation. It provides a consistent view of all images
//! that can be accessed without locking.

use crate::config::Config;
use crate::slot::{ImageData, ImageMeta, ImageSlot};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Memory budget tracker using atomic operations.
pub struct MemoryBudget {
    /// Total budget in bytes
    total: usize,
    /// Currently used bytes (atomic for lock-free tracking)
    used: AtomicUsize,
}

impl MemoryBudget {
    pub fn new(total: usize) -> Self {
        Self {
            total,
            used: AtomicUsize::new(0),
        }
    }

    pub fn from_config(config: &Config) -> Self {
        Self::new(config.memory.calculate_budget())
    }

    #[inline]
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn available(&self) -> usize {
        self.total.saturating_sub(self.used())
    }

    /// Try to allocate memory. Returns true if successful.
    pub fn try_allocate(&self, bytes: usize) -> bool {
        let mut current = self.used.load(Ordering::Relaxed);
        loop {
            if current + bytes > self.total {
                return false;
            }
            match self.used.compare_exchange_weak(
                current,
                current + bytes,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(x) => current = x,
            }
        }
    }

    /// Release previously allocated memory
    pub fn release(&self, bytes: usize) {
        self.used.fetch_sub(bytes, Ordering::SeqCst);
    }
}

/// The image store - holds all slots and manages memory.
pub struct ImageStore {
    /// All image slots (indexed by position in directory)
    slots: Vec<ImageSlot>,
    /// Memory budget
    budget: Arc<MemoryBudget>,
}

impl ImageStore {
    /// Create store with pre-populated metadata
    pub fn with_metadata(metas: Vec<ImageMeta>, budget: Arc<MemoryBudget>) -> Self {
        let slots = metas.into_iter().map(ImageSlot::new).collect();
        Self { slots, budget }
    }

    /// Number of images
    #[inline]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Get a slot by index (wraps around)
    #[inline]
    pub fn get(&self, index: usize) -> Option<&ImageSlot> {
        if self.slots.is_empty() {
            None
        } else {
            Some(&self.slots[index % self.slots.len()])
        }
    }

    /// Get slot unchecked (caller ensures valid index)
    #[inline]
    pub fn slot(&self, index: usize) -> &ImageSlot {
        &self.slots[index]
    }

    /// Read image data at index (lock-free)
    #[inline]
    pub fn read(&self, index: usize) -> Option<Arc<ImageData>> {
        self.get(index)?.read()
    }

    /// Insert/upgrade image data at index.
    /// Manages memory budget automatically.
    pub fn insert(&self, index: usize, data: Arc<ImageData>) -> bool {
        let slot = match self.get(index) {
            Some(s) => s,
            None => return false,
        };

        let new_size = data.memory_size();
        let old_size = slot.memory_used();

        // Calculate net memory change
        let net_increase = new_size.saturating_sub(old_size);

        // Try to allocate the additional memory needed
        if net_increase > 0 && !self.budget.try_allocate(net_increase) {
            return false; // Not enough memory
        }

        // Perform the upgrade
        if slot.upgrade(data) {
            // Release old memory if we had some
            if old_size > 0 && new_size > old_size {
                // We already accounted for net increase, nothing more needed
            } else if old_size > new_size {
                // Somehow got smaller (shouldn't happen with upgrade)
                self.budget.release(old_size - new_size);
            }
            true
        } else {
            // Upgrade rejected (not higher quality) - release allocated memory
            if net_increase > 0 {
                self.budget.release(net_increase);
            }
            false
        }
    }

    /// Evict images far from current position.
    /// Returns amount of memory freed.
    pub fn evict_far(&self, current: usize, keep_range: usize) -> usize {
        let total = self.len();
        if total == 0 {
            return 0;
        }

        let mut freed = 0;

        for (idx, slot) in self.slots.iter().enumerate() {
            let dist = circular_distance(idx, current, total);
            if dist > keep_range && !slot.is_empty() {
                let mem = slot.memory_used();
                slot.clear();
                self.budget.release(mem);
                freed += mem;
            }
        }

        freed
    }

    /// Evict lowest priority images until we have enough space.
    /// Returns amount of memory freed.
    pub fn make_room(&self, needed: usize, current: usize) -> usize {
        if self.budget.available() >= needed {
            return 0;
        }

        let total = self.len();
        if total == 0 {
            return 0;
        }

        // Collect (index, distance, memory) for non-empty slots
        let mut candidates: Vec<(usize, usize, usize)> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| !slot.is_empty())
            .map(|(idx, slot)| {
                let dist = circular_distance(idx, current, total);
                let mem = slot.memory_used();
                (idx, dist, mem)
            })
            .collect();

        // Sort by distance descending (furthest first)
        candidates.sort_by(|a, b| b.1.cmp(&a.1));

        let mut freed = 0;

        for (idx, _, mem) in candidates {
            if self.budget.available() >= needed {
                break;
            }
            self.slots[idx].clear();
            self.budget.release(mem);
            freed += mem;
        }

        freed
    }
}

/// Calculate shortest distance in circular list
#[inline]
pub fn circular_distance(a: usize, b: usize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    let forward = if a >= b { a - b } else { total - b + a };
    let backward = if b >= a { b - a } else { total - a + b };
    forward.min(backward)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circular_distance() {
        assert_eq!(circular_distance(0, 0, 10), 0);
        assert_eq!(circular_distance(0, 1, 10), 1);
        assert_eq!(circular_distance(0, 5, 10), 5);
        assert_eq!(circular_distance(0, 9, 10), 1); // Wrap around
        assert_eq!(circular_distance(9, 0, 10), 1);
        assert_eq!(circular_distance(3, 7, 10), 4);
    }

    #[test]
    fn test_budget() {
        let budget = MemoryBudget::new(1000);

        assert!(budget.try_allocate(500));
        assert_eq!(budget.used(), 500);

        assert!(budget.try_allocate(400));
        assert_eq!(budget.used(), 900);

        assert!(!budget.try_allocate(200)); // Would exceed
        assert_eq!(budget.used(), 900);

        budget.release(300);
        assert_eq!(budget.used(), 600);

        assert!(budget.try_allocate(200)); // Now fits
        assert_eq!(budget.used(), 800);
    }
}
