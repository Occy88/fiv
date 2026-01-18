//! Preloader - parallel background loading with predictive prefetching.
//!
//! The preloader uses all CPU cores to decode images in parallel.
//! It tracks navigation direction to predict which images to load next,
//! biasing heavily in the direction of travel.
//!
//! Key design principles:
//! - Never block the main thread
//! - Always have something to show (even thumbnail)
//! - Predict user's next images based on direction
//! - Use all available cores for decoding

use crate::config::{PreloadConfig, QualityTier};
use crate::decode::Decoder;
use crate::slot::ImageMeta;
use crate::state::{Direction, SharedState};
use crate::store::{circular_distance, ImageStore, MemoryBudget};
use rayon::prelude::*;
use std::sync::Arc;
use std::thread;

/// Spawn the preloader thread.
pub fn spawn_preloader(
    store: Arc<ImageStore>,
    shared_state: Arc<SharedState>,
    decoder: Arc<Decoder>,
    config: crate::config::Config,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        preloader_loop(store, shared_state, decoder, config.preload);
    })
}

/// Main preloader loop - runs continuously until shutdown
fn preloader_loop(
    store: Arc<ImageStore>,
    state: Arc<SharedState>,
    decoder: Arc<Decoder>,
    config: PreloadConfig,
) {
    // Configure rayon thread pool if max_parallel_tasks is set
    if config.max_parallel_tasks > 0 {
        rayon::ThreadPoolBuilder::new()
            .num_threads(config.max_parallel_tasks)
            .build_global()
            .ok(); // Ignore if already initialized
    }

    loop {
        if state.is_shutdown() {
            return;
        }

        let total = store.len();
        if total == 0 {
            thread::sleep(config.idle_poll_interval);
            continue;
        }

        // Get current state
        let current = state.current();
        let direction = state.direction();

        // Build load tasks based on direction
        let tasks = build_prioritized_tasks(&store, current, total, direction, &config);

        if tasks.is_empty() {
            // Nothing to load - evict far images and wait
            evict_far_images(&store, current, &config);
            thread::sleep(config.idle_poll_interval);
            continue;
        }

        // Decode ALL tasks in parallel - don't limit batch size
        // Rayon will efficiently distribute across cores
        let results: Vec<_> = tasks
            .par_iter()
            .filter_map(|task| {
                // Don't check generation during decode - we want to finish work
                // even if user navigated (the images are still useful)
                let slot = store.slot(task.index);
                let path = &slot.meta.path;
                decoder.decode(path, task.quality).map(|data| (task.index, data))
            })
            .collect();

        // Insert all results - even if user navigated, these are still useful
        // They'll be evicted later if too far away
        let current_now = state.current();
        for (idx, data) in results {
            let dist = circular_distance(idx, current_now, total);
            // Make room for nearby images
            if dist <= config.full_quality_count {
                store.make_room(data.memory_size(), current_now);
            }
            store.insert(idx, data);
        }

        // Evict images that are too far from current position
        evict_far_images(&store, state.current(), &config);
    }
}

/// A task describing what to load
#[derive(Debug, Clone, Copy)]
struct LoadTask {
    index: usize,
    quality: QualityTier,
    distance: usize,
    in_direction: bool, // Is this in the predicted direction of travel?
}

/// Build prioritized list of images to load based on direction
fn build_prioritized_tasks(
    store: &ImageStore,
    current: usize,
    total: usize,
    direction: Direction,
    config: &PreloadConfig,
) -> Vec<LoadTask> {
    let mut tasks = Vec::new();
    let (ahead_range, behind_range) = config.range_for_direction(direction);

    // Current image: ALWAYS load at full quality first
    if !store.slot(current).has_quality(QualityTier::Full) {
        tasks.push(LoadTask {
            index: current,
            quality: QualityTier::Full,
            distance: 0,
            in_direction: true,
        });
    }

    // Build tasks for ahead direction
    for offset in 1..=ahead_range {
        let idx = (current + offset) % total;
        let desired_quality = config.quality_for_distance(offset);
        let slot = store.slot(idx);

        if !slot.has_quality(desired_quality) {
            tasks.push(LoadTask {
                index: idx,
                quality: desired_quality,
                distance: offset,
                in_direction: direction != Direction::Backward,
            });
        }
    }

    // Build tasks for behind direction
    for offset in 1..=behind_range {
        let idx = (current + total - offset) % total;
        let desired_quality = config.quality_for_distance(offset);
        let slot = store.slot(idx);

        if !slot.has_quality(desired_quality) {
            tasks.push(LoadTask {
                index: idx,
                quality: desired_quality,
                distance: offset,
                in_direction: direction != Direction::Forward,
            });
        }
    }

    // Sort tasks by priority:
    // 1. In-direction tasks first
    // 2. Higher quality first (Full > Preview > Thumbnail)
    // 3. Closer distance first
    tasks.sort_by(|a, b| {
        // In-direction first
        match (a.in_direction, b.in_direction) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            _ => {}
        }
        // Higher quality first
        match b.quality.cmp(&a.quality) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        // Closer first
        a.distance.cmp(&b.distance)
    });

    tasks
}

/// Evict images that are too far from current position
fn evict_far_images(store: &ImageStore, current: usize, config: &PreloadConfig) {
    let keep_range = config.total_range();
    store.evict_far(current, keep_range);
}

/// Create image store with paths only (fast startup, no I/O)
pub fn create_store_fast(
    paths: Vec<std::path::PathBuf>,
    budget: Arc<MemoryBudget>,
) -> ImageStore {
    let metas: Vec<ImageMeta> = paths
        .into_iter()
        .map(|path| ImageMeta::new(path, 0, 0))
        .collect();

    ImageStore::with_metadata(metas, budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_priority() {
        let config = PreloadConfig::default();

        // Full quality should be higher priority
        assert!(config.quality_for_distance(1) == QualityTier::Full);
        assert!(config.quality_for_distance(5) == QualityTier::Full);
        assert!(config.quality_for_distance(10) == QualityTier::Preview);
        assert!(config.quality_for_distance(20) == QualityTier::Thumbnail);
    }

    #[test]
    fn test_direction_ranges() {
        let config = PreloadConfig::default();

        // Forward: more ahead
        let (ahead, behind) = config.range_for_direction(Direction::Forward);
        assert!(ahead > behind);

        // Backward: more behind
        let (ahead, behind) = config.range_for_direction(Direction::Backward);
        assert!(behind > ahead);

        // Unknown: symmetric
        let (ahead, behind) = config.range_for_direction(Direction::Unknown);
        assert_eq!(ahead, behind);
    }
}
