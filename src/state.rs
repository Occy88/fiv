//! State management - pure data structures for application state.
//!
//! This module contains the state types that drive the viewer. The key insight
//! is separating input state (what keys are held) from view state (what to render).
//! This allows frame-based navigation during key hold.

use crate::config::InputConfig;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// Input state tracking with click vs hold distinction.
///
/// Behavior:
/// - Quick press-release (< hold_threshold): Single navigation on release
/// - Long press (>= hold_threshold): Repeat navigation while held
#[derive(Debug)]
pub struct InputState {
    /// Right/forward navigation key held
    right_held: bool,
    /// Left/backward navigation key held
    left_held: bool,
    /// Home key pressed (single shot)
    pub home_pressed: bool,
    /// End key pressed (single shot)
    pub end_pressed: bool,
    /// When the current key was pressed
    press_start: Option<Instant>,
    /// Direction of current press (1 = right, -1 = left)
    press_direction: i32,
    /// Whether we're in repeat mode (held past threshold)
    in_repeat_mode: bool,
    /// When last repeat navigation occurred
    last_repeat: Instant,
    /// Pending click to emit on release (direction)
    pending_click: Option<i32>,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            right_held: false,
            left_held: false,
            home_pressed: false,
            end_pressed: false,
            press_start: None,
            press_direction: 0,
            in_repeat_mode: false,
            last_repeat: Instant::now(),
            pending_click: None,
        }
    }

    /// Called when right key state changes
    pub fn set_right(&mut self, pressed: bool) {
        if pressed && !self.right_held {
            // Key just pressed
            self.start_press(1);
        } else if !pressed && self.right_held {
            // Key just released
            self.end_press(1);
        }
        self.right_held = pressed;
    }

    /// Called when left key state changes
    pub fn set_left(&mut self, pressed: bool) {
        if pressed && !self.left_held {
            // Key just pressed
            self.start_press(-1);
        } else if !pressed && self.left_held {
            // Key just released
            self.end_press(-1);
        }
        self.left_held = pressed;
    }

    /// Start tracking a key press
    fn start_press(&mut self, direction: i32) {
        self.press_start = Some(Instant::now());
        self.press_direction = direction;
        self.in_repeat_mode = false;
        self.pending_click = None;
    }

    /// Handle key release
    fn end_press(&mut self, direction: i32) {
        // Only handle if this was the active press
        if self.press_direction == direction {
            if !self.in_repeat_mode {
                // Was a quick click - queue single navigation
                self.pending_click = Some(direction);
            }
            // Reset press tracking
            self.press_start = None;
            self.press_direction = 0;
            self.in_repeat_mode = false;
        }
    }

    /// Process input and return navigation direction.
    /// Returns: Some(1) for forward, Some(-1) for backward, None for no navigation.
    pub fn process(&mut self, config: &InputConfig) -> Option<i32> {
        let now = Instant::now();

        // Handle single-shot keys first
        if self.home_pressed {
            self.home_pressed = false;
            return Some(i32::MIN); // Special: go to start
        }
        if self.end_pressed {
            self.end_pressed = false;
            return Some(i32::MAX); // Special: go to end
        }

        // Handle pending click from release
        if let Some(dir) = self.pending_click.take() {
            return Some(dir);
        }

        // Check if a key is being held
        let start = self.press_start?;

        let held_duration = now.duration_since(start);

        // Check if we should enter repeat mode
        if !self.in_repeat_mode {
            if held_duration >= config.hold_threshold {
                // Enter repeat mode - first navigation
                self.in_repeat_mode = true;
                self.last_repeat = now;
                return Some(self.press_direction);
            }
            // Still in click detection phase - no navigation yet
            return None;
        }

        // In repeat mode - check interval
        let since_last = now.duration_since(self.last_repeat);
        if since_last >= config.repeat_interval {
            self.last_repeat = now;
            return Some(self.press_direction);
        }

        None
    }

    /// Check if any navigation is active (for control flow)
    pub fn is_navigating(&self) -> bool {
        self.right_held || self.left_held || self.home_pressed || self.end_pressed || self.pending_click.is_some()
    }
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

/// View state - what the viewer is currently showing.
///
/// This is the "model" in a model-view separation. It contains everything
/// needed to render a frame, with no references to external resources.
#[derive(Debug, Clone)]
pub struct ViewState {
    /// Current image index
    pub current_index: usize,
    /// Total number of images
    pub total_images: usize,
    /// Window dimensions
    pub window_width: u32,
    pub window_height: u32,
    /// Whether a render is needed
    pub needs_render: bool,
    /// Last rendered quality (for upgrade detection)
    pub last_render_quality: Option<crate::config::QualityTier>,
}

impl ViewState {
    pub fn new(total_images: usize, window_width: u32, window_height: u32) -> Self {
        Self {
            current_index: 0,
            total_images,
            window_width,
            window_height,
            needs_render: true,
            last_render_quality: None,
        }
    }

    /// Navigate by delta (positive = forward, negative = backward)
    pub fn navigate(&mut self, delta: i32) {
        if self.total_images == 0 {
            return;
        }

        // Handle special values
        if delta == i32::MIN {
            self.current_index = 0;
        } else if delta == i32::MAX {
            self.current_index = self.total_images - 1;
        } else {
            // Normal navigation with wrap-around
            let new_index = if delta >= 0 {
                (self.current_index + delta as usize) % self.total_images
            } else {
                let back = (-delta) as usize;
                if back > self.current_index {
                    self.total_images - (back - self.current_index) % self.total_images
                } else {
                    self.current_index - back
                }
            };
            // Handle edge case where modulo gives total_images
            self.current_index = new_index % self.total_images;
        }

        self.needs_render = true;
        self.last_render_quality = None;
    }

    /// Update window size
    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.window_width = width;
            self.window_height = height;
            self.needs_render = true;
        }
    }

    /// Mark that a quality upgrade is available
    pub fn signal_quality_upgrade(&mut self) {
        self.needs_render = true;
    }

    /// Mark render complete with given quality
    pub fn render_complete(&mut self, quality: crate::config::QualityTier) {
        self.needs_render = false;
        self.last_render_quality = Some(quality);
    }

    /// Check if we need to re-render for quality upgrade
    pub fn needs_quality_upgrade(&self) -> bool {
        match self.last_render_quality {
            Some(q) => q != crate::config::QualityTier::Full,
            None => false,
        }
    }

    /// Get formatted title string
    pub fn title(&self, filename: &str) -> String {
        let quality_indicator = match self.last_render_quality {
            Some(crate::config::QualityTier::Thumbnail) => " [loading...]",
            Some(crate::config::QualityTier::Preview) => " [preview]",
            _ => "",
        };

        if self.total_images == 0 {
            "Fiv - No images found".to_string()
        } else {
            format!(
                "Fiv - {} [{}/{}]{}",
                filename,
                self.current_index + 1,
                self.total_images,
                quality_indicator
            )
        }
    }
}

/// Navigation direction for predictive loading
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
    Unknown,
}

/// Shared state for communication between main thread and preloader.
/// Uses atomics for lock-free access.
pub struct SharedState {
    /// Current image index (set by main thread, read by preloader)
    current_index: AtomicUsize,
    /// Previous index (for direction detection)
    previous_index: AtomicUsize,
    /// Generation counter (incremented on each navigation)
    generation: AtomicUsize,
    /// Navigation direction: 0=unknown, 1=forward, 2=backward
    direction: AtomicUsize,
    /// Shutdown flag
    shutdown: AtomicUsize,
    /// Total number of images (for wrap-around detection)
    total: AtomicUsize,
}

impl SharedState {
    pub fn new() -> Self {
        Self {
            current_index: AtomicUsize::new(0),
            previous_index: AtomicUsize::new(0),
            generation: AtomicUsize::new(0),
            direction: AtomicUsize::new(0),
            shutdown: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
        }
    }

    /// Set total number of images
    pub fn set_total(&self, total: usize) {
        self.total.store(total, Ordering::SeqCst);
    }

    /// Update current index and track direction (main thread)
    pub fn set_current(&self, index: usize) {
        let prev = self.current_index.load(Ordering::SeqCst);
        let total = self.total.load(Ordering::SeqCst);

        // Detect direction (handling wrap-around)
        let dir = if total == 0 || prev == index {
            0 // Unknown
        } else if index == (prev + 1) % total {
            1 // Forward
        } else if index == (prev + total - 1) % total {
            2 // Backward
        } else if index > prev {
            1 // Forward (jump)
        } else {
            2 // Backward (jump)
        };

        self.previous_index.store(prev, Ordering::SeqCst);
        self.current_index.store(index, Ordering::SeqCst);
        self.direction.store(dir, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Get current index (preloader)
    pub fn current(&self) -> usize {
        self.current_index.load(Ordering::SeqCst)
    }

    /// Get navigation direction
    pub fn direction(&self) -> Direction {
        match self.direction.load(Ordering::SeqCst) {
            1 => Direction::Forward,
            2 => Direction::Backward,
            _ => Direction::Unknown,
        }
    }

    /// Signal shutdown (main thread)
    pub fn shutdown(&self) {
        self.shutdown.store(1, Ordering::SeqCst);
    }

    /// Check if shutdown was requested (preloader)
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst) != 0
    }
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::InputConfig;
    use std::time::Duration;

    #[test]
    fn test_navigation() {
        let mut state = ViewState::new(10, 800, 600);

        // Forward
        state.navigate(1);
        assert_eq!(state.current_index, 1);

        // Backward
        state.navigate(-1);
        assert_eq!(state.current_index, 0);

        // Wrap forward (navigate to end then forward)
        state.navigate(i32::MAX); // Go to last image
        assert_eq!(state.current_index, 9);
        state.navigate(1);
        assert_eq!(state.current_index, 0);

        // Wrap backward
        state.navigate(-1);
        assert_eq!(state.current_index, 9);
    }

    #[test]
    fn test_click_vs_hold() {
        let config = InputConfig {
            hold_threshold: Duration::from_millis(150),
            repeat_interval: Duration::from_millis(60),
        };

        let mut input = InputState::new();

        // Quick press-release should not navigate until release
        input.set_right(true);
        let result = input.process(&config);
        assert_eq!(result, None); // No navigation yet - waiting to see if it's a click or hold

        // Release quickly - should queue a click
        input.set_right(false);
        let result = input.process(&config);
        assert_eq!(result, Some(1)); // Click navigation

        // Should not navigate again
        let result = input.process(&config);
        assert_eq!(result, None);
    }

    #[test]
    fn test_hold_repeat() {
        let config = InputConfig {
            hold_threshold: Duration::from_millis(10), // Short for testing
            repeat_interval: Duration::from_millis(5),
        };

        let mut input = InputState::new();

        // Press and hold
        input.set_right(true);

        // Wait past threshold
        std::thread::sleep(Duration::from_millis(15));

        // Should enter repeat mode
        let result = input.process(&config);
        assert_eq!(result, Some(1));

        // Wait for repeat interval
        std::thread::sleep(Duration::from_millis(10));
        let result = input.process(&config);
        assert_eq!(result, Some(1));
    }

    #[test]
    fn test_shared_state() {
        let state = SharedState::new();

        assert_eq!(state.current(), 0);

        state.set_current(5);
        assert_eq!(state.current(), 5);

        assert!(!state.is_shutdown());
        state.shutdown();
        assert!(state.is_shutdown());
    }

    #[test]
    fn test_direction_tracking() {
        let state = SharedState::new();
        state.set_total(10);

        // Initial direction unknown
        assert_eq!(state.direction(), Direction::Unknown);

        // Move forward: 0 -> 1
        state.set_current(1);
        assert_eq!(state.direction(), Direction::Forward);

        // Move forward: 1 -> 2
        state.set_current(2);
        assert_eq!(state.direction(), Direction::Forward);

        // Move backward: 2 -> 1
        state.set_current(1);
        assert_eq!(state.direction(), Direction::Backward);

        // Wrap around forward: 9 -> 0
        state.set_current(9);
        state.set_current(0);
        assert_eq!(state.direction(), Direction::Forward);

        // Wrap around backward: 0 -> 9
        state.set_current(9);
        assert_eq!(state.direction(), Direction::Backward);
    }
}
