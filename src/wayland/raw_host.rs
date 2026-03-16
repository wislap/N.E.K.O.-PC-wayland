use std::convert::TryInto;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop::channel::{
    Event as ChannelEvent, Sender as ChannelSender, channel,
};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState, SimpleGlobal};
use smithay_client_toolkit::seat::{
    Capability, SeatHandler, SeatState,
    keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
    pointer::{BTN_LEFT, PointerData, PointerEvent, PointerEventKind, PointerHandler},
};
use smithay_client_toolkit::shell::{
    WaylandSurface,
    xdg::{
        XdgShell,
        window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
    },
};
use smithay_client_toolkit::shm::{
    Shm, ShmHandler,
    slot::{Buffer, SlotPool},
};
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, delegate_simple, delegate_xdg_shell, delegate_xdg_window,
    registry_handlers,
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::{self, WpViewport},
    wp_viewporter::WpViewporter,
};

use crate::wayland::input_region::{InputRegion, InteractiveRect};

#[derive(Debug, Clone)]
pub struct RawHostConfig {
    pub title: String,
    pub app_id: String,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub input_region_source_size: Option<(u32, u32)>,
    pub input_region: InputRegion,
    pub visible_region: Option<InputRegion>,
    pub transparent_outside_input_region: bool,
    pub show_debug_regions_when_empty: bool,
    pub move_on_left_press: bool,
    pub log_pointer_events: bool,
    pub install_ctrlc_handler: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawHostPointerButton {
    Left,
    Middle,
    Right,
    Other(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawHostPointerEvent {
    Enter {
        x: f64,
        y: f64,
    },
    Leave {
        x: f64,
        y: f64,
    },
    Motion {
        x: f64,
        y: f64,
    },
    Button {
        x: f64,
        y: f64,
        button: RawHostPointerButton,
        pressed: bool,
    },
    Wheel {
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawHostModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub caps_lock: bool,
    pub logo: bool,
    pub num_lock: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RawHostKeyboardEvent {
    Press {
        raw_code: u32,
        keysym: u32,
        utf8: Option<String>,
        modifiers: RawHostModifiers,
    },
    Repeat {
        raw_code: u32,
        keysym: u32,
        utf8: Option<String>,
        modifiers: RawHostModifiers,
    },
    Release {
        raw_code: u32,
        keysym: u32,
        modifiers: RawHostModifiers,
    },
}

#[derive(Debug, Clone)]
pub struct RawHostFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl RawHostFrame {
    pub fn from_bgra(width: u32, height: u32, bgra: Vec<u8>) -> Result<Self> {
        let expected_len = width as usize * height as usize * 4;
        if bgra.len() != expected_len {
            return Err(anyhow!(
                "invalid BGRA frame length {}, expected {} for {}x{}",
                bgra.len(),
                expected_len,
                width,
                height
            ));
        }

        Ok(Self {
            width,
            height,
            bgra,
        })
    }

    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let expected_len = width as usize * height as usize * 4;
        if rgba.len() != expected_len {
            return Err(anyhow!(
                "invalid RGBA frame length {}, expected {} for {}x{}",
                rgba.len(),
                expected_len,
                width,
                height
            ));
        }

        let mut bgra = vec![0_u8; expected_len];
        for (src, dst) in rgba.chunks_exact(4).zip(bgra.chunks_exact_mut(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }

        Self::from_bgra(width, height, bgra)
    }
}

impl RawHostConfig {
    pub fn probe() -> Self {
        Self {
            title: "raw-wayland-region-probe".to_string(),
            app_id: "moe.neko.raw-wayland-region-probe".to_string(),
            width: 800,
            height: 600,
            fullscreen: false,
            input_region_source_size: None,
            input_region: InputRegion::from_rects(vec![InteractiveRect {
                x: 0,
                y: 0,
                width: 220,
                height: 320,
            }]),
            visible_region: None,
            transparent_outside_input_region: true,
            show_debug_regions_when_empty: true,
            move_on_left_press: true,
            log_pointer_events: true,
            install_ctrlc_handler: true,
        }
    }
}

pub fn run(config: RawHostConfig) -> Result<()> {
    run_inner(config, None)
}

pub fn spawn(config: RawHostConfig) -> Result<RawHostThread> {
    let (sender, receiver) = channel::<RawHostCommand>();
    let running = Arc::new(AtomicBool::new(true));
    let latest_frame = Arc::new(Mutex::new(None));
    let frame_update_pending = Arc::new(AtomicBool::new(false));
    let pointer_subscribers = Arc::new(Mutex::new(Vec::new()));
    let keyboard_subscribers = Arc::new(Mutex::new(Vec::new()));
    let thread_running = Arc::clone(&running);
    let thread_latest_frame = Arc::clone(&latest_frame);
    let thread_frame_update_pending = Arc::clone(&frame_update_pending);
    let thread_pointer_subscribers = Arc::clone(&pointer_subscribers);
    let thread_keyboard_subscribers = Arc::clone(&keyboard_subscribers);
    let thread = thread::Builder::new()
        .name("neko-raw-wayland-host".to_string())
        .spawn(move || {
            run_inner_with_running(
                config,
                Some(receiver),
                thread_running,
                thread_latest_frame,
                thread_frame_update_pending,
                thread_pointer_subscribers,
                thread_keyboard_subscribers,
            )
        })
        .context("failed to spawn raw Wayland host thread")?;

    Ok(RawHostThread {
        handle: RawHostHandle {
            sender,
            running,
            latest_frame,
            frame_update_pending,
            pointer_subscribers,
            keyboard_subscribers,
        },
        thread,
    })
}

fn run_inner(
    config: RawHostConfig,
    commands: Option<smithay_client_toolkit::reexports::calloop::channel::Channel<RawHostCommand>>,
) -> Result<()> {
    let running = Arc::new(AtomicBool::new(true));
    let latest_frame = Arc::new(Mutex::new(None));
    let frame_update_pending = Arc::new(AtomicBool::new(false));
    run_inner_with_running(
        config,
        commands,
        running,
        latest_frame,
        frame_update_pending,
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
}

fn run_inner_with_running(
    config: RawHostConfig,
    commands: Option<smithay_client_toolkit::reexports::calloop::channel::Channel<RawHostCommand>>,
    running: Arc<AtomicBool>,
    latest_frame: Arc<Mutex<Option<RawHostFrame>>>,
    frame_update_pending: Arc<AtomicBool>,
    pointer_subscribers: Arc<Mutex<Vec<Sender<RawHostPointerEvent>>>>,
    keyboard_subscribers: Arc<Mutex<Vec<Sender<RawHostKeyboardEvent>>>>,
) -> Result<()> {
    let _ = env_logger::try_init();

    let conn = Connection::connect_to_env().context("failed to connect to Wayland compositor")?;
    let (globals, event_queue) =
        registry_queue_init(&conn).context("failed to initialize Wayland registry")?;
    let qh = event_queue.handle();

    let mut event_loop: EventLoop<RawHostApp> =
        EventLoop::try_new().context("failed to create calloop event loop")?;
    WaylandSource::new(conn.clone(), event_queue)
        .insert(event_loop.handle())
        .context("failed to insert Wayland source into event loop")?;

    let compositor =
        CompositorState::bind(&globals, &qh).context("wl_compositor is not available")?;
    let xdg_shell = XdgShell::bind(&globals, &qh).context("xdg_shell is not available")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm is not available")?;
    let viewporter = SimpleGlobal::<WpViewporter, 1>::bind(&globals, &qh).ok();

    let surface = compositor.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::None, &qh);
    window.set_title(config.title.clone());
    window.set_app_id(config.app_id.clone());
    if config.fullscreen {
        window.set_min_size(None);
        window.set_max_size(None);
        window.set_fullscreen(None);
    } else {
        window.set_min_size(Some((config.width, config.height)));
        window.set_max_size(Some((config.width, config.height)));
    }
    window.commit();
    let viewport = viewporter
        .as_ref()
        .and_then(|global| global.get().ok())
        .map(|viewporter| viewporter.get_viewport(window.wl_surface(), &qh, ()));

    let pool = SlotPool::new((config.width * config.height * 4) as usize, &shm)
        .context("failed to create shm pool")?;

    if config.install_ctrlc_handler {
        let ctrlc_flag = Arc::clone(&running);
        ctrlc::set_handler(move || {
            ctrlc_flag.store(false, Ordering::SeqCst);
        })
        .context("failed to install Ctrl-C handler")?;
    }

    let mut app = RawHostApp {
        registry_state: RegistryState::new(&globals),
        compositor_state: compositor,
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        shm,
        pool,
        window,
        viewporter,
        viewport,
        runtime: RawHostRuntime {
            width: config.width,
            height: config.height,
            fullscreen: config.fullscreen,
            input_region_source_size: config.input_region_source_size,
            source_input_region: config.input_region.clone(),
            input_region: scale_input_region_to_window(
                &config.input_region,
                config.input_region_source_size,
                config.width,
                config.height,
            ),
            visible_region: config.visible_region.clone(),
            frame: None,
            input_region_applied: false,
            transparent_outside_input_region: config.transparent_outside_input_region,
            show_debug_regions_when_empty: config.show_debug_regions_when_empty,
            move_on_left_press: config.move_on_left_press,
            log_pointer_events: config.log_pointer_events,
        },
        buffer: None,
        buffer_dimensions: None,
        first_configure: true,
        exit: false,
        pointer: None,
        keyboard: None,
        pointer_inside: false,
        running,
        latest_frame,
        frame_update_pending,
        pointer_subscribers,
        keyboard_subscribers,
        modifiers: RawHostModifiers::default(),
    };

    if let Some(command_channel) = commands {
        event_loop
            .handle()
            .insert_source(command_channel, |event, _, app| match event {
                ChannelEvent::Msg(command) => {
                    if let Err(err) = app.handle_command(command) {
                        eprintln!("raw host command failed: {err:#}");
                        app.exit = true;
                    }
                }
                ChannelEvent::Closed => {
                    eprintln!("raw host command channel closed");
                    app.exit = true;
                    app.running.store(false, Ordering::SeqCst);
                }
            })
            .map_err(|err| anyhow!("failed to install raw host command source: {err}"))?;
    }

    eprintln!(
        "raw Wayland host started: title={:?} size={}x{} fullscreen={} input_rects={}",
        config.title,
        config.width,
        config.height,
        config.fullscreen,
        config.input_region.rects().len()
    );

    while app.running.load(Ordering::SeqCst) && !app.exit {
        event_loop
            .dispatch(Duration::from_millis(16), &mut app)
            .context("Wayland event loop dispatch failed")?;
    }

    app.running.store(false, Ordering::SeqCst);

    Ok(())
}

#[derive(Debug)]
pub struct RawHostThread {
    pub handle: RawHostHandle,
    thread: JoinHandle<Result<()>>,
}

impl RawHostThread {
    pub fn join(self) -> thread::Result<Result<()>> {
        self.thread.join()
    }

    pub fn detach(self) -> RawHostHandle {
        let handle = self.handle.clone();
        std::mem::forget(self);
        handle
    }
}

#[derive(Debug, Clone)]
pub struct RawHostHandle {
    sender: ChannelSender<RawHostCommand>,
    running: Arc<AtomicBool>,
    latest_frame: Arc<Mutex<Option<RawHostFrame>>>,
    frame_update_pending: Arc<AtomicBool>,
    pointer_subscribers: Arc<Mutex<Vec<Sender<RawHostPointerEvent>>>>,
    keyboard_subscribers: Arc<Mutex<Vec<Sender<RawHostKeyboardEvent>>>>,
}

impl RawHostHandle {
    pub fn set_input_region(&self, region: InputRegion) -> Result<()> {
        self.sender
            .send(RawHostCommand::SetInputRegion(region))
            .context("failed to send input-region update to raw host")
    }

    pub fn set_visible_region(&self, region: Option<InputRegion>) -> Result<()> {
        self.sender
            .send(RawHostCommand::SetVisibleRegion(region))
            .context("failed to send visible-region update to raw host")
    }

    pub fn shutdown(&self) -> Result<()> {
        self.sender
            .send(RawHostCommand::Shutdown)
            .context("failed to send shutdown command to raw host")
    }

    pub fn set_rgba_frame(&self, frame: RawHostFrame) -> Result<()> {
        {
            let mut latest_frame = self
                .latest_frame
                .lock()
                .expect("latest frame mutex poisoned");
            *latest_frame = Some(frame);
        }
        if !self.frame_update_pending.swap(true, Ordering::SeqCst) {
            self.sender
                .send(RawHostCommand::FrameReady)
                .context("failed to notify raw host about latest frame")?;
        }
        Ok(())
    }

    pub fn clear_frame(&self) -> Result<()> {
        self.sender
            .send(RawHostCommand::ClearFrame)
            .context("failed to clear RGBA frame in raw host")
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn subscribe_pointer_events(&self) -> Receiver<RawHostPointerEvent> {
        let (sender, receiver) = mpsc::channel();
        self.pointer_subscribers
            .lock()
            .expect("pointer subscribers mutex poisoned")
            .push(sender);
        receiver
    }

    pub fn subscribe_keyboard_events(&self) -> Receiver<RawHostKeyboardEvent> {
        let (sender, receiver) = mpsc::channel();
        self.keyboard_subscribers
            .lock()
            .expect("keyboard subscribers mutex poisoned")
            .push(sender);
        receiver
    }
}

#[derive(Debug)]
enum RawHostCommand {
    SetInputRegion(InputRegion),
    SetVisibleRegion(Option<InputRegion>),
    FrameReady,
    ClearFrame,
    Shutdown,
}

#[derive(Debug)]
struct RawHostRuntime {
    width: u32,
    height: u32,
    fullscreen: bool,
    input_region_source_size: Option<(u32, u32)>,
    source_input_region: InputRegion,
    input_region: InputRegion,
    visible_region: Option<InputRegion>,
    frame: Option<RawHostFrame>,
    input_region_applied: bool,
    transparent_outside_input_region: bool,
    show_debug_regions_when_empty: bool,
    move_on_left_press: bool,
    log_pointer_events: bool,
}

struct RawHostApp {
    registry_state: RegistryState,
    compositor_state: CompositorState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    pool: SlotPool,
    window: Window,
    viewporter: Option<SimpleGlobal<WpViewporter, 1>>,
    viewport: Option<WpViewport>,
    runtime: RawHostRuntime,
    buffer: Option<Buffer>,
    buffer_dimensions: Option<(u32, u32)>,
    first_configure: bool,
    exit: bool,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer_inside: bool,
    running: Arc<AtomicBool>,
    latest_frame: Arc<Mutex<Option<RawHostFrame>>>,
    frame_update_pending: Arc<AtomicBool>,
    pointer_subscribers: Arc<Mutex<Vec<Sender<RawHostPointerEvent>>>>,
    keyboard_subscribers: Arc<Mutex<Vec<Sender<RawHostKeyboardEvent>>>>,
    modifiers: RawHostModifiers,
}

impl RawHostApp {
    fn broadcast_pointer_event(&self, event: RawHostPointerEvent) {
        let mut subscribers = self
            .pointer_subscribers
            .lock()
            .expect("pointer subscribers mutex poisoned");
        subscribers.retain(|sender| sender.send(event).is_ok());
    }

    fn broadcast_keyboard_event(&self, event: RawHostKeyboardEvent) {
        let mut subscribers = self
            .keyboard_subscribers
            .lock()
            .expect("keyboard subscribers mutex poisoned");
        subscribers.retain(|sender| sender.send(event.clone()).is_ok());
    }

    fn handle_command(&mut self, command: RawHostCommand) -> Result<()> {
        match command {
            RawHostCommand::SetInputRegion(region) => {
                self.runtime.source_input_region = region;
                self.runtime.input_region = scale_input_region_to_window(
                    &self.runtime.source_input_region,
                    self.runtime.input_region_source_size,
                    self.runtime.width,
                    self.runtime.height,
                );
                self.runtime.input_region_applied = false;
                self.apply_input_region()?;
            }
            RawHostCommand::SetVisibleRegion(region) => {
                self.runtime.visible_region = region;
                self.draw()?;
            }
            RawHostCommand::FrameReady => {
                self.consume_latest_frame()?;
            }
            RawHostCommand::ClearFrame => {
                self.frame_update_pending.store(false, Ordering::SeqCst);
                if let Ok(mut latest_frame) = self.latest_frame.lock() {
                    latest_frame.take();
                }
                self.runtime.frame = None;
                self.draw()?;
            }
            RawHostCommand::Shutdown => {
                self.exit = true;
                self.running.store(false, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    fn consume_latest_frame(&mut self) -> Result<()> {
        loop {
            let next_frame = {
                let mut latest_frame = self
                    .latest_frame
                    .lock()
                    .expect("latest frame mutex poisoned");
                latest_frame.take()
            };

            if let Some(frame) = next_frame {
                self.runtime.frame = Some(frame);
                self.draw()?;
            }

            self.frame_update_pending.store(false, Ordering::SeqCst);
            let has_newer_frame = self
                .latest_frame
                .lock()
                .expect("latest frame mutex poisoned")
                .is_some();
            if !has_newer_frame || self.frame_update_pending.swap(true, Ordering::SeqCst) {
                break;
            }
        }

        Ok(())
    }

    fn apply_input_region(&mut self) -> Result<()> {
        let region = Region::new(&self.compositor_state).context("failed to create wl_region")?;
        for rect in self.runtime.input_region.rects() {
            region.add(
                rect.x,
                rect.y,
                i32::try_from(rect.width).context("input region width overflow")?,
                i32::try_from(rect.height).context("input region height overflow")?,
            );
        }
        self.window.set_input_region(Some(region.wl_region()));
        self.window.commit();
        self.runtime.input_region_applied = true;
        eprintln!(
            "applied raw wl_surface input-region with {} rect(s): {:?}",
            self.runtime.input_region.rects().len(),
            self.runtime.input_region.rects()
        );
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        let (draw_width, draw_height) = self
            .preferred_buffer_size()
            .unwrap_or((self.runtime.width, self.runtime.height));
        let stride = draw_width as i32 * 4;
        let width = self.runtime.width;
        let height = self.runtime.height;

        if !self.runtime.input_region_applied {
            self.apply_input_region()?;
        }

        if self.buffer_dimensions != Some((draw_width, draw_height)) {
            self.buffer = None;
            self.buffer_dimensions = Some((draw_width, draw_height));
        }

        let buffer = self.buffer.get_or_insert_with(|| {
            self.pool
                .create_buffer(
                    draw_width as i32,
                    draw_height as i32,
                    stride,
                    wl_shm::Format::Argb8888,
                )
                .expect("failed to allocate shm buffer")
                .0
        });

        let canvas = match self.pool.canvas(buffer) {
            Some(canvas) => canvas,
            None => {
                let (next_buffer, canvas) = self
                    .pool
                    .create_buffer(
                        draw_width as i32,
                        draw_height as i32,
                        stride,
                        wl_shm::Format::Argb8888,
                    )
                    .context("failed to allocate secondary shm buffer")?;
                *buffer = next_buffer;
                canvas
            }
        };

        let visible_region = self
            .runtime
            .visible_region
            .as_ref()
            .unwrap_or(&self.runtime.input_region);
        let frame_ref = self.runtime.frame.as_ref();

        if let Some(frame) = frame_ref {
            if frame.width == draw_width
                && frame.height == draw_height
                && region_covers_full_window(visible_region, width, height)
            {
                blit_fullscreen_rgba_to_argb(canvas, frame);
            } else if region_covers_full_window(visible_region, width, height)
                && draw_width == width
                && draw_height == height
            {
                blit_scaled_fullscreen_rgba_to_argb(canvas, draw_width, draw_height, frame);
            } else {
                draw_frame_with_regions(
                    canvas,
                    draw_width,
                    draw_height,
                    frame,
                    visible_region,
                    self.runtime.transparent_outside_input_region,
                );
            }
        } else {
            draw_debug_background(
                canvas,
                draw_width,
                draw_height,
                &self.runtime.input_region,
                visible_region,
                self.runtime.show_debug_regions_when_empty,
                self.runtime.transparent_outside_input_region,
            );
        }

        if let Some(viewport) = self.viewport.as_ref() {
            viewport.set_source(0.0, 0.0, draw_width as f64, draw_height as f64);
            viewport.set_destination(width as i32, height as i32);
        }

        self.window
            .wl_surface()
            .damage_buffer(0, 0, draw_width as i32, draw_height as i32);
        buffer
            .attach_to(self.window.wl_surface())
            .context("failed to attach buffer to wl_surface")?;
        self.window.commit();
        Ok(())
    }

    fn preferred_buffer_size(&self) -> Option<(u32, u32)> {
        if self.viewport.is_none() {
            return None;
        }
        let visible_region = self
            .runtime
            .visible_region
            .as_ref()
            .unwrap_or(&self.runtime.input_region);
        if !region_covers_full_window(visible_region, self.runtime.width, self.runtime.height) {
            return None;
        }
        if let Some(frame) = self.runtime.frame.as_ref() {
            return Some((frame.width, frame.height));
        }
        self.runtime.input_region_source_size
    }
}

fn point_in_rect(x: i32, y: i32, rect: &crate::wayland::input_region::InteractiveRect) -> bool {
    let right = rect.x.saturating_add(rect.width as i32);
    let bottom = rect.y.saturating_add(rect.height as i32);
    x >= rect.x && y >= rect.y && x < right && y < bottom
}

fn scale_input_region_to_window(
    region: &InputRegion,
    source_size: Option<(u32, u32)>,
    dst_width: u32,
    dst_height: u32,
) -> InputRegion {
    let Some((src_width, src_height)) = source_size else {
        return region.clone();
    };
    if src_width == 0
        || src_height == 0
        || dst_width == 0
        || dst_height == 0
        || (src_width == dst_width && src_height == dst_height)
    {
        return region.clone();
    }

    InputRegion::from_rects(
        region
            .rects()
            .iter()
            .map(|rect| InteractiveRect {
                x: ((i64::from(rect.x) * i64::from(dst_width)) / i64::from(src_width)) as i32,
                y: ((i64::from(rect.y) * i64::from(dst_height)) / i64::from(src_height)) as i32,
                width: ((u64::from(rect.width) * u64::from(dst_width)) / u64::from(src_width))
                    .max(1) as u32,
                height: ((u64::from(rect.height) * u64::from(dst_height))
                    / u64::from(src_height))
                .max(1) as u32,
            })
            .collect(),
    )
}

fn region_covers_full_window(region: &InputRegion, width: u32, height: u32) -> bool {
    matches!(
        region.rects(),
        [InteractiveRect {
            x: 0,
            y: 0,
            width: rect_width,
            height: rect_height,
        }] if *rect_width == width && *rect_height == height
    )
}

fn blit_fullscreen_rgba_to_argb(canvas: &mut [u8], frame: &RawHostFrame) {
    canvas.copy_from_slice(&frame.bgra);
}

fn blit_scaled_fullscreen_rgba_to_argb(
    canvas: &mut [u8],
    dst_width: u32,
    dst_height: u32,
    frame: &RawHostFrame,
) {
    if dst_width == 0 || dst_height == 0 || frame.width == 0 || frame.height == 0 {
        canvas.fill(0);
        return;
    }

    let dst_width_usize = dst_width as usize;
    let src_width_usize = frame.width as usize;
    let mut src_x_map = Vec::with_capacity(dst_width_usize);
    for dst_x in 0..dst_width {
        let src_x = ((dst_x as u64 * frame.width as u64) / dst_width as u64)
            .min(frame.width.saturating_sub(1) as u64) as usize;
        src_x_map.push(src_x);
    }

    for dst_y in 0..dst_height as usize {
        let src_y = ((dst_y as u64 * frame.height as u64) / dst_height as u64)
            .min(frame.height.saturating_sub(1) as u64) as usize;
        let src_row = &frame.bgra[src_y * src_width_usize * 4..(src_y + 1) * src_width_usize * 4];
        let dst_row =
            &mut canvas[dst_y * dst_width_usize * 4..(dst_y + 1) * dst_width_usize * 4];
        for (dst_x, src_x) in src_x_map.iter().copied().enumerate() {
            let src_base = src_x * 4;
            let dst_base = dst_x * 4;
            dst_row[dst_base] = src_row[src_base];
            dst_row[dst_base + 1] = src_row[src_base + 1];
            dst_row[dst_base + 2] = src_row[src_base + 2];
            dst_row[dst_base + 3] = src_row[src_base + 3];
        }
    }
}

fn draw_frame_with_regions(
    canvas: &mut [u8],
    width: u32,
    _height: u32,
    frame: &RawHostFrame,
    visible_region: &InputRegion,
    transparent_outside_input_region: bool,
) {
    for (index, chunk) in canvas.chunks_exact_mut(4).enumerate() {
        let x = (index % width as usize) as i32;
        let y = (index / width as usize) as i32;
        let visible = visible_region
            .rects()
            .iter()
            .any(|rect| point_in_rect(x, y, rect));
        let color = color_from_scaled_frame_or_background(
            frame,
            x,
            y,
            width,
            _height,
            visible,
            transparent_outside_input_region,
        );
        let array: &mut [u8; 4] = chunk.try_into().expect("chunk size must be 4");
        *array = color.to_le_bytes();
    }
}

fn draw_debug_background(
    canvas: &mut [u8],
    width: u32,
    _height: u32,
    input_region: &InputRegion,
    visible_region: &InputRegion,
    show_debug_regions_when_empty: bool,
    transparent_outside_input_region: bool,
) {
    for (index, chunk) in canvas.chunks_exact_mut(4).enumerate() {
        let x = (index % width as usize) as i32;
        let y = (index / width as usize) as i32;
        let interactive = input_region.rects().iter().any(|rect| point_in_rect(x, y, rect));
        let visible = visible_region
            .rects()
            .iter()
            .any(|rect| point_in_rect(x, y, rect));
        let color = if show_debug_regions_when_empty {
            if interactive {
                argb(0xFF, 0xFF, 0xD0, 0x3B)
            } else if visible {
                argb(0xFF, 0xC9, 0x4D, 0x4D)
            } else if transparent_outside_input_region {
                argb(0x00, 0x00, 0x00, 0x00)
            } else {
                argb(0xFF, 0x72, 0x12, 0x12)
            }
        } else {
            argb(0x00, 0x00, 0x00, 0x00)
        };
        let array: &mut [u8; 4] = chunk.try_into().expect("chunk size must be 4");
        *array = color.to_le_bytes();
    }
}

fn map_pointer_button(button: u32) -> RawHostPointerButton {
    match button {
        BTN_LEFT => RawHostPointerButton::Left,
        0x111 => RawHostPointerButton::Right,
        0x112 => RawHostPointerButton::Middle,
        other => RawHostPointerButton::Other(other),
    }
}

fn argb(a: u32, r: u32, g: u32, b: u32) -> u32 {
    (a << 24) | (r << 16) | (g << 8) | b
}

fn color_from_scaled_frame_or_background(
    frame: &RawHostFrame,
    x: i32,
    y: i32,
    dst_width: u32,
    dst_height: u32,
    visible: bool,
    transparent_outside_input_region: bool,
) -> u32 {
    if !visible {
        return if transparent_outside_input_region {
            argb(0x00, 0x00, 0x00, 0x00)
        } else {
            argb(0xFF, 0x72, 0x12, 0x12)
        };
    }

    if x < 0 || y < 0 || dst_width == 0 || dst_height == 0 {
        return argb(0x00, 0x00, 0x00, 0x00);
    }

    let src_x = ((x as u32).saturating_mul(frame.width) / dst_width).min(frame.width.saturating_sub(1));
    let src_y = ((y as u32).saturating_mul(frame.height) / dst_height).min(frame.height.saturating_sub(1));
    let base = ((src_y * frame.width + src_x) * 4) as usize;
    let b = frame.bgra[base] as u32;
    let g = frame.bgra[base + 1] as u32;
    let r = frame.bgra[base + 2] as u32;
    let a = frame.bgra[base + 3] as u32;
    argb(a, r, g, b)
}

impl CompositorHandler for RawHostApp {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for RawHostApp {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl WindowHandler for RawHostApp {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.exit = true;
        self.running.store(false, Ordering::SeqCst);
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let was_first_configure = self.first_configure;
        let new_width = configure
            .new_size
            .0
            .map(|value| value.get())
            .unwrap_or(self.runtime.width);
        let new_height = configure
            .new_size
            .1
            .map(|value| value.get())
            .unwrap_or(self.runtime.height);
        let size_changed = new_width != self.runtime.width || new_height != self.runtime.height;

        if size_changed {
            self.buffer = None;
            self.runtime.width = new_width;
            self.runtime.height = new_height;
        }
        self.runtime.input_region = scale_input_region_to_window(
            &self.runtime.source_input_region,
            self.runtime.input_region_source_size,
            self.runtime.width,
            self.runtime.height,
        );
        if size_changed {
            self.runtime.input_region_applied = false;
            eprintln!(
                "raw host configure: {}x{}",
                self.runtime.width, self.runtime.height
            );
        }

        if self.runtime.fullscreen {
            self.runtime.visible_region = Some(InputRegion::from_rects(vec![InteractiveRect {
                x: 0,
                y: 0,
                width: self.runtime.width,
                height: self.runtime.height,
            }]));
        }

        if self.first_configure {
            self.first_configure = false;
        }

        if (!size_changed && !was_first_configure) || self.exit {
            return;
        }

        if let Err(err) = self.draw() {
            eprintln!("raw host draw failed during configure: {err:#}");
            self.exit = true;
        }
    }
}

impl SeatHandler for RawHostApp {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => {
                    eprintln!("pointer capability acquired");
                    self.pointer = Some(pointer);
                }
                Err(err) => eprintln!("failed to create pointer: {err}"),
            }
        }

        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seat_state.get_keyboard(qh, &seat, None) {
                Ok(keyboard) => {
                    eprintln!("keyboard capability acquired");
                    self.keyboard = Some(keyboard);
                }
                Err(err) => eprintln!("failed to create keyboard: {err}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }

        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for RawHostApp {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        keysyms: &[Keysym],
    ) {
        eprintln!("keyboard enter: pressed={keysyms:?}");
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        eprintln!("keyboard leave");
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.broadcast_keyboard_event(RawHostKeyboardEvent::Press {
            raw_code: event.raw_code,
            keysym: event.keysym.raw(),
            utf8: event.utf8.clone(),
            modifiers: self.modifiers,
        });
        eprintln!("key press: {event:?}");
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.broadcast_keyboard_event(RawHostKeyboardEvent::Repeat {
            raw_code: event.raw_code,
            keysym: event.keysym.raw(),
            utf8: event.utf8.clone(),
            modifiers: self.modifiers,
        });
        eprintln!("key repeat: {event:?}");
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.broadcast_keyboard_event(RawHostKeyboardEvent::Release {
            raw_code: event.raw_code,
            keysym: event.keysym.raw(),
            modifiers: self.modifiers,
        });
        eprintln!("key release: {event:?}");
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _raw_modifiers: RawModifiers,
        _layout: u32,
    ) {
        self.modifiers = RawHostModifiers {
            ctrl: modifiers.ctrl,
            alt: modifiers.alt,
            shift: modifiers.shift,
            caps_lock: modifiers.caps_lock,
            logo: modifiers.logo,
            num_lock: modifiers.num_lock,
        };
        eprintln!("modifiers changed: {modifiers:?}");
    }
}

impl PointerHandler for RawHostApp {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if &event.surface != self.window.wl_surface() {
                continue;
            }

            match event.kind {
                PointerEventKind::Enter { serial } => {
                    self.pointer_inside = true;
                    self.broadcast_pointer_event(RawHostPointerEvent::Enter {
                        x: event.position.0,
                        y: event.position.1,
                    });
                    if self.runtime.log_pointer_events {
                        eprintln!("pointer enter serial={serial} pos={:?}", event.position);
                    }
                }
                PointerEventKind::Leave { serial } => {
                    self.pointer_inside = false;
                    self.broadcast_pointer_event(RawHostPointerEvent::Leave {
                        x: event.position.0,
                        y: event.position.1,
                    });
                    if self.runtime.log_pointer_events {
                        eprintln!("pointer leave serial={serial}");
                    }
                }
                PointerEventKind::Motion { .. } => {
                    self.broadcast_pointer_event(RawHostPointerEvent::Motion {
                        x: event.position.0,
                        y: event.position.1,
                    });
                    if self.runtime.log_pointer_events && self.pointer_inside {
                        eprintln!("pointer motion pos={:?}", event.position);
                    }
                }
                PointerEventKind::Press { button, serial, .. } => {
                    if self.runtime.move_on_left_press && button == BTN_LEFT {
                        if let Some(pointer_data) = pointer.data::<PointerData>() {
                            let seat = pointer_data.seat();
                            self.window.move_(seat, serial);
                            eprintln!(
                                "started xdg_toplevel move via input region: serial={} pos={:?}",
                                serial, event.position
                            );
                        }
                    }
                    self.broadcast_pointer_event(RawHostPointerEvent::Button {
                        x: event.position.0,
                        y: event.position.1,
                        button: map_pointer_button(button),
                        pressed: true,
                    });
                    if self.runtime.log_pointer_events {
                        eprintln!(
                            "pointer press button={} serial={} pos={:?}",
                            button, serial, event.position
                        );
                    }
                }
                PointerEventKind::Release { button, serial, .. } => {
                    self.broadcast_pointer_event(RawHostPointerEvent::Button {
                        x: event.position.0,
                        y: event.position.1,
                        button: map_pointer_button(button),
                        pressed: false,
                    });
                    if self.runtime.log_pointer_events {
                        eprintln!(
                            "pointer release button={} serial={} pos={:?}",
                            button, serial, event.position
                        );
                    }
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } => {
                    self.broadcast_pointer_event(RawHostPointerEvent::Wheel {
                        x: event.position.0,
                        y: event.position.1,
                        delta_x: horizontal.absolute,
                        delta_y: vertical.absolute,
                    });
                    if self.runtime.log_pointer_events {
                        eprintln!(
                            "pointer axis horizontal={horizontal:?} vertical={vertical:?} pos={:?}",
                            event.position
                        );
                    }
                }
            }
        }
    }
}

impl ShmHandler for RawHostApp {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for RawHostApp {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState,];
}

impl AsMut<SimpleGlobal<WpViewporter, 1>> for RawHostApp {
    fn as_mut(&mut self) -> &mut SimpleGlobal<WpViewporter, 1> {
        self.viewporter
            .as_mut()
            .expect("wp_viewporter global requested but not bound")
    }
}

impl Dispatch<WpViewport, ()> for RawHostApp {
    fn event(
        _: &mut RawHostApp,
        _: &WpViewport,
        _: wp_viewport::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<RawHostApp>,
    ) {
        unreachable!("wp_viewport::Event is empty in version 1")
    }
}

delegate_compositor!(RawHostApp);
delegate_output!(RawHostApp);
delegate_shm!(RawHostApp);
delegate_seat!(RawHostApp);
delegate_keyboard!(RawHostApp);
delegate_pointer!(RawHostApp);
delegate_xdg_shell!(RawHostApp);
delegate_xdg_window!(RawHostApp);
delegate_simple!(RawHostApp, WpViewporter, 1);
delegate_registry!(RawHostApp);
