//! Fiv - A high-performance image viewer (Fast Image Viewer).
//!
//! Architecture overview:
//! - Lock-free image slots for zero-contention access
//! - Input state tracking for immediate press-and-hold response
//! - Pure render functions (no side effects)
//! - Background preloader that never blocks the main thread

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
use std::path::PathBuf;
use std::sync::Arc;
use store::{ImageStore, MemoryBudget};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

#[derive(Parser, Debug)]
#[command(name = "fiv")]
#[command(about = "A high-performance image viewer", long_about = None)]
struct Args {
    #[arg(default_value = ".")]
    directory: PathBuf,
}

/// Key actions for data-driven input handling
#[derive(Clone, Copy)]
enum KeyAction {
    NavigateRight,
    NavigateLeft,
    JumpHome,
    JumpEnd,
    Quit,
}

/// Key binding table - maps physical keys to actions
const KEY_BINDINGS: &[(KeyCode, KeyAction)] = &[
    (KeyCode::ArrowRight, KeyAction::NavigateRight),
    (KeyCode::KeyD, KeyAction::NavigateRight),
    (KeyCode::Space, KeyAction::NavigateRight),
    (KeyCode::ArrowLeft, KeyAction::NavigateLeft),
    (KeyCode::KeyA, KeyAction::NavigateLeft),
    (KeyCode::Home, KeyAction::JumpHome),
    (KeyCode::End, KeyAction::JumpEnd),
    (KeyCode::Escape, KeyAction::Quit),
    (KeyCode::KeyQ, KeyAction::Quit),
];

fn lookup_key_action(key: KeyCode) -> Option<KeyAction> {
    KEY_BINDINGS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, action)| *action)
}

/// Initialized window state - created once window is ready
struct WindowState {
    window: Arc<Window>,
    pixels: Pixels<'static>,
    view_state: ViewState,
    _preloader_handle: std::thread::JoinHandle<()>,
}

impl WindowState {
    fn create(
        event_loop: &ActiveEventLoop,
        config: &Config,
        store: &Arc<ImageStore>,
        shared_state: &Arc<SharedState>,
        decoder: &Arc<Decoder>,
    ) -> Self {
        let window_attributes = Window::default_attributes()
            .with_title("Fiv - Loading...")
            .with_inner_size(LogicalSize::new(
                config.render.default_width,
                config.render.default_height,
            ));

        let window = Arc::new(
            event_loop
                .create_window(window_attributes)
                .expect("Failed to create window"),
        );

        let size = window.inner_size();
        let surface_texture = SurfaceTexture::new(size.width, size.height, Arc::clone(&window));
        let pixels = Pixels::new(size.width, size.height, surface_texture)
            .expect("Failed to create pixel buffer");

        let view_state = ViewState::new(store.len(), size.width, size.height);

        // Load first image synchronously for immediate display
        if let Some(slot) = store.get(0) {
            if let Some(data) = decoder.decode(&slot.meta.path, QualityTier::Full) {
                store.insert(0, data);
            }
        }

        // Spawn preloader after first image
        let preloader_handle = spawn_preloader(
            Arc::clone(store),
            Arc::clone(shared_state),
            Arc::clone(decoder),
            config.clone(),
        );

        Self {
            window,
            pixels,
            view_state,
            _preloader_handle: preloader_handle,
        }
    }

    fn render(&mut self, store: &ImageStore, config: &Config) {
        let frame = self.pixels.frame_mut();
        let image_data = store.read(self.view_state.current_index);

        let result = render_image(
            image_data.as_ref(),
            frame,
            self.view_state.window_width,
            self.view_state.window_height,
            config.render.background_color,
        );

        match result.quality {
            Some(quality) => self.view_state.render_complete(quality),
            None => self.view_state.needs_render = true,
        }

        let _ = self.pixels.render();
    }

    fn update_title(&self, store: &ImageStore) {
        let filename = store
            .get(self.view_state.current_index)
            .and_then(|slot| slot.meta.path.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        self.window.set_title(&self.view_state.title(&filename));
    }

    fn handle_resize(&mut self, width: u32, height: u32) {
        self.view_state.resize(width, height);
        let _ = self.pixels.resize_surface(width, height);
        let _ = self.pixels.resize_buffer(width, height);
    }

    fn check_quality_upgrade(&mut self, store: &ImageStore) {
        if self.view_state.needs_render || !self.view_state.needs_quality_upgrade() {
            return;
        }

        let dominated_by_preloader = store
            .get(self.view_state.current_index)
            .and_then(|slot| slot.current_quality())
            .map(|q| Some(q) > self.view_state.last_render_quality)
            .unwrap_or(false);

        if dominated_by_preloader {
            self.view_state.signal_quality_upgrade();
        }
    }

    fn control_flow(&self, input_state: &InputState) -> ControlFlow {
        let active = input_state.is_navigating()
            || self.view_state.needs_render
            || self.view_state.needs_quality_upgrade();

        if active {
            ControlFlow::Poll
        } else {
            ControlFlow::Wait
        }
    }
}

/// Application with two-phase initialization
struct App {
    config: Config,
    decoder: Arc<Decoder>,
    store: Arc<ImageStore>,
    shared_state: Arc<SharedState>,
    input_state: InputState,
    window_state: Option<WindowState>,
}

impl App {
    fn new(
        config: Config,
        decoder: Arc<Decoder>,
        store: Arc<ImageStore>,
        shared_state: Arc<SharedState>,
    ) -> Self {
        Self {
            config,
            decoder,
            store,
            shared_state,
            input_state: InputState::new(),
            window_state: None,
        }
    }

    fn handle_key_action(
        &mut self,
        action: KeyAction,
        pressed: bool,
        event_loop: &ActiveEventLoop,
    ) {
        match action {
            KeyAction::NavigateRight => self.input_state.set_right(pressed),
            KeyAction::NavigateLeft => self.input_state.set_left(pressed),
            KeyAction::JumpHome if pressed => self.input_state.home_pressed = true,
            KeyAction::JumpEnd if pressed => self.input_state.end_pressed = true,
            KeyAction::Quit if pressed => {
                self.shared_state.shutdown();
                event_loop.exit();
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window_state.is_some() {
            return;
        }

        let mut ws = WindowState::create(
            event_loop,
            &self.config,
            &self.store,
            &self.shared_state,
            &self.decoder,
        );

        ws.render(&self.store, &self.config);
        ws.update_title(&self.store);
        self.window_state = Some(ws);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let ws = match self.window_state.as_mut() {
            Some(ws) => ws,
            None => return,
        };

        match event {
            WindowEvent::CloseRequested => {
                self.shared_state.shutdown();
                event_loop.exit();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    if let Some(action) = lookup_key_action(key) {
                        self.handle_key_action(
                            action,
                            event.state == ElementState::Pressed,
                            event_loop,
                        );
                    }
                }
            }

            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                ws.handle_resize(size.width, size.height);
            }

            WindowEvent::RedrawRequested => {
                ws.render(&self.store, &self.config);
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let ws = match self.window_state.as_mut() {
            Some(ws) => ws,
            None => return,
        };

        event_loop.set_control_flow(ws.control_flow(&self.input_state));

        // Process navigation
        if let Some(delta) = self.input_state.process(&self.config.input) {
            ws.view_state.navigate(delta);
            self.shared_state.set_current(ws.view_state.current_index);
            ws.update_title(&self.store);
        }

        ws.check_quality_upgrade(&self.store);

        if ws.view_state.needs_render {
            ws.render(&self.store, &self.config);
            ws.update_title(&self.store);
            ws.window.request_redraw();
        }
    }
}

fn main() {
    let args = Args::parse();

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

    let config = Config::default();
    let decoder = Arc::new(Decoder::new());
    let budget = Arc::new(MemoryBudget::from_config(&config));
    let paths = scan_directory(&dir, &decoder);

    if paths.is_empty() {
        eprintln!(
            "No supported images found in '{}'\nSupported formats: {:?}",
            dir.display(),
            decoder.extensions()
        );
        std::process::exit(1);
    }

    let store = Arc::new(create_store_fast(paths, Arc::clone(&budget)));
    let shared_state = Arc::new(SharedState::new());
    shared_state.set_total(store.len());

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    let mut app = App::new(config, decoder, store, shared_state);

    event_loop.run_app(&mut app).expect("Event loop error");
}
