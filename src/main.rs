//! Picto - A high-performance image viewer.
//!
//! Architecture overview:
//! - Lock-free image slots for zero-contention access
//! - Input state tracking for immediate press-and-hold response
//! - Pure render functions (no side effects)
//! - Background preloader that never blocks the main thread
//!
//! The key insight is treating the viewer as a "window over raw data":
//! - Main thread reads from slots atomically, never waits
//! - Background threads upgrade slot data atomically
//! - Rendering is always immediate with best available data

mod config;
mod decode;
mod preload;
mod render;
mod slot;
mod state;
mod store;

use clap::Parser;
use config::{Config, QualityTier};
use decode::{scan_directory, Decoder};
use pixels::{Pixels, SurfaceTexture};
use preload::{create_store_fast, spawn_preloader};
use render::render_image;
use state::{InputState, SharedState, ViewState};
use store::{ImageStore, MemoryBudget};
use std::path::PathBuf;
use std::sync::Arc;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, Event, VirtualKeyCode, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowBuilder};

#[derive(Parser, Debug)]
#[command(name = "picto")]
#[command(about = "A high-performance image viewer", long_about = None)]
struct Args {
    /// Directory containing images
    #[arg(default_value = ".")]
    directory: PathBuf,
}

fn main() {
    let args = Args::parse();

    // Validate directory
    let dir = args.directory.canonicalize().unwrap_or_else(|_| {
        eprintln!(
            "Error: Cannot access directory '{}'",
            args.directory.display()
        );
        std::process::exit(1);
    });

    if !dir.is_dir() {
        eprintln!("Error: '{}' is not a directory", dir.display());
        std::process::exit(1);
    }

    // Load configuration
    let config = Config::default();

    // Create window and event loop FIRST for fast startup
    let event_loop = EventLoop::new();

    let window = WindowBuilder::new()
        .with_title("Picto - Loading...")
        .with_inner_size(LogicalSize::new(
            config.render.default_width,
            config.render.default_height,
        ))
        .build(&event_loop)
        .expect("Failed to create window");

    let size = window.inner_size();
    let surface_texture = SurfaceTexture::new(size.width, size.height, &window);
    let mut pixels = Pixels::new(size.width, size.height, surface_texture)
        .expect("Failed to create pixel buffer");

    // Initialize components
    let decoder = Arc::new(Decoder::new());
    let budget = Arc::new(MemoryBudget::from_config(&config));

    // Scan directory (fast - just lists files)
    let paths = scan_directory(&dir, &decoder);

    if paths.is_empty() {
        eprintln!(
            "No supported images found in '{}'\nSupported formats: {:?}",
            dir.display(),
            decoder.extensions()
        );
        std::process::exit(1);
    }

    // Create store WITHOUT reading metadata (fast)
    let store = Arc::new(create_store_fast(paths, Arc::clone(&budget)));

    // Create shared state for main/preloader communication
    let shared_state = Arc::new(SharedState::new());
    shared_state.set_total(store.len());

    // Load first image on main thread for immediate display
    if let Some(slot) = store.get(0) {
        if let Some(data) = decoder.decode(&slot.meta.path, QualityTier::Full) {
            store.insert(0, data);
        }
    }

    // Spawn preloader thread AFTER first image is loaded
    let _preloader_handle = spawn_preloader(
        Arc::clone(&store),
        Arc::clone(&shared_state),
        Arc::clone(&decoder),
        config.clone(),
    );

    // Initialize state
    let mut view_state = ViewState::new(store.len(), size.width, size.height);
    let mut input_state = InputState::new();

    // Initial render
    do_render(&store, &mut view_state, &mut pixels, &config);
    update_title(&window, &store, &view_state);

    // Run event loop
    event_loop.run(move |event, _, control_flow| {
        // Determine control flow based on state
        *control_flow = if input_state.is_navigating() || view_state.needs_render || view_state.needs_quality_upgrade() {
            ControlFlow::Poll // Active mode
        } else {
            ControlFlow::Wait // Power-efficient mode
        };

        match event {
            Event::WindowEvent { event, .. } => {
                handle_window_event(
                    event,
                    &mut input_state,
                    &mut view_state,
                    &shared_state,
                    &window,
                    &mut pixels,
                    control_flow,
                );
            }

            Event::MainEventsCleared => {
                // Process input state and navigate if needed
                if let Some(delta) = input_state.process(&config.input) {
                    view_state.navigate(delta);
                    shared_state.set_current(view_state.current_index);
                    update_title(&window, &store, &view_state);
                }

                // Check for quality upgrades from preloader
                if !view_state.needs_render && view_state.needs_quality_upgrade() {
                    if let Some(slot) = store.get(view_state.current_index) {
                        if let Some(current_quality) = slot.current_quality() {
                            if Some(current_quality) > view_state.last_render_quality {
                                view_state.signal_quality_upgrade();
                            }
                        }
                    }
                }

                // Render if needed
                if view_state.needs_render {
                    do_render(&store, &mut view_state, &mut pixels, &config);
                    update_title(&window, &store, &view_state);
                }
            }

            Event::RedrawRequested(_) => {
                // Fallback render on explicit redraw request
                do_render(&store, &mut view_state, &mut pixels, &config);
            }

            _ => {}
        }
    });
}

/// Handle window events
fn handle_window_event(
    event: WindowEvent,
    input_state: &mut InputState,
    view_state: &mut ViewState,
    shared_state: &SharedState,
    window: &Window,
    pixels: &mut Pixels,
    control_flow: &mut ControlFlow,
) {
    match event {
        WindowEvent::CloseRequested => {
            shared_state.shutdown();
            *control_flow = ControlFlow::Exit;
        }

        WindowEvent::KeyboardInput { input, .. } => {
            if let Some(key) = input.virtual_keycode {
                let pressed = input.state == ElementState::Pressed;

                match key {
                    // Navigation keys - track state
                    VirtualKeyCode::Right | VirtualKeyCode::D | VirtualKeyCode::Space => {
                        input_state.set_right(pressed);
                    }
                    VirtualKeyCode::Left | VirtualKeyCode::A => {
                        input_state.set_left(pressed);
                    }

                    // Single-shot keys
                    VirtualKeyCode::Home if pressed => {
                        input_state.home_pressed = true;
                    }
                    VirtualKeyCode::End if pressed => {
                        input_state.end_pressed = true;
                    }

                    // Exit
                    VirtualKeyCode::Escape | VirtualKeyCode::Q if pressed => {
                        shared_state.shutdown();
                        *control_flow = ControlFlow::Exit;
                    }

                    _ => {}
                }
            }
        }

        WindowEvent::Resized(new_size) => {
            handle_resize(new_size, view_state, pixels, window);
        }

        _ => {}
    }
}

/// Handle window resize
fn handle_resize(
    new_size: PhysicalSize<u32>,
    view_state: &mut ViewState,
    pixels: &mut Pixels,
    _window: &Window,
) {
    if new_size.width > 0 && new_size.height > 0 {
        view_state.resize(new_size.width, new_size.height);
        let _ = pixels.resize_surface(new_size.width, new_size.height);
        let _ = pixels.resize_buffer(new_size.width, new_size.height);
    }
}

/// Perform rendering
fn do_render(
    store: &ImageStore,
    view_state: &mut ViewState,
    pixels: &mut Pixels,
    config: &Config,
) {
    let frame = pixels.frame_mut();

    // Get current image data (lock-free read)
    let image_data = store.read(view_state.current_index);

    // Render
    let result = render_image(
        image_data.as_ref(),
        frame,
        view_state.window_width,
        view_state.window_height,
        config.render.background_color,
    );

    // Update state
    if let Some(quality) = result.quality {
        view_state.render_complete(quality);
    } else {
        // No image available - request redraw later
        view_state.needs_render = true;
    }

    // Submit to GPU
    let _ = pixels.render();
}

/// Update window title
fn update_title(window: &Window, store: &ImageStore, view_state: &ViewState) {
    let filename = store
        .get(view_state.current_index)
        .map(|slot| {
            slot.meta
                .path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default();

    window.set_title(&view_state.title(&filename));
}
