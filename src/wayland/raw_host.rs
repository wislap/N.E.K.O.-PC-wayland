use std::convert::TryInto;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::channel::{
    Event as ChannelEvent, Sender as ChannelSender, channel,
};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
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
    delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window, registry_handlers,
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};

use crate::wayland::input_region::{InputRegion, InteractiveRect};

#[derive(Debug, Clone)]
pub struct RawHostConfig {
    pub title: String,
    pub app_id: String,
    pub width: u32,
    pub height: u32,
    pub input_region: InputRegion,
    pub visible_region: Option<InputRegion>,
    pub transparent_outside_input_region: bool,
    pub move_on_left_press: bool,
    pub log_pointer_events: bool,
    pub install_ctrlc_handler: bool,
}

#[derive(Debug, Clone)]
pub struct RawHostFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RawHostFrame {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
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

        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

impl RawHostConfig {
    pub fn probe() -> Self {
        Self {
            title: "raw-wayland-region-probe".to_string(),
            app_id: "moe.neko.raw-wayland-region-probe".to_string(),
            width: 800,
            height: 600,
            input_region: InputRegion::from_rects(vec![InteractiveRect {
                x: 0,
                y: 0,
                width: 220,
                height: 320,
            }]),
            visible_region: None,
            transparent_outside_input_region: true,
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
    let thread_running = Arc::clone(&running);
    let thread = thread::Builder::new()
        .name("neko-raw-wayland-host".to_string())
        .spawn(move || run_inner_with_running(config, Some(receiver), thread_running))
        .context("failed to spawn raw Wayland host thread")?;

    Ok(RawHostThread {
        handle: RawHostHandle { sender, running },
        thread,
    })
}

fn run_inner(config: RawHostConfig, commands: Option<smithay_client_toolkit::reexports::calloop::channel::Channel<RawHostCommand>>) -> Result<()> {
    let running = Arc::new(AtomicBool::new(true));
    run_inner_with_running(config, commands, running)
}

fn run_inner_with_running(
    config: RawHostConfig,
    commands: Option<smithay_client_toolkit::reexports::calloop::channel::Channel<RawHostCommand>>,
    running: Arc<AtomicBool>,
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

    let surface = compositor.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::None, &qh);
    window.set_title(config.title.clone());
    window.set_app_id(config.app_id.clone());
    window.set_min_size(Some((config.width, config.height)));
    window.set_max_size(Some((config.width, config.height)));
    window.commit();

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
        runtime: RawHostRuntime {
            width: config.width,
            height: config.height,
            input_region: config.input_region.clone(),
            visible_region: config.visible_region.clone(),
            frame: None,
            input_region_applied: false,
            transparent_outside_input_region: config.transparent_outside_input_region,
            move_on_left_press: config.move_on_left_press,
            log_pointer_events: config.log_pointer_events,
        },
        buffer: None,
        first_configure: true,
        exit: false,
        pointer: None,
        keyboard: None,
        pointer_inside: false,
        running,
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
        "raw Wayland host started: title={:?} size={}x{} input_rects={}",
        config.title,
        config.width,
        config.height,
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
        self.sender
            .send(RawHostCommand::SetFrame(frame))
            .context("failed to send RGBA frame to raw host")
    }

    pub fn clear_frame(&self) -> Result<()> {
        self.sender
            .send(RawHostCommand::ClearFrame)
            .context("failed to clear RGBA frame in raw host")
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

#[derive(Debug)]
enum RawHostCommand {
    SetInputRegion(InputRegion),
    SetVisibleRegion(Option<InputRegion>),
    SetFrame(RawHostFrame),
    ClearFrame,
    Shutdown,
}

#[derive(Debug)]
struct RawHostRuntime {
    width: u32,
    height: u32,
    input_region: InputRegion,
    visible_region: Option<InputRegion>,
    frame: Option<RawHostFrame>,
    input_region_applied: bool,
    transparent_outside_input_region: bool,
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
    runtime: RawHostRuntime,
    buffer: Option<Buffer>,
    first_configure: bool,
    exit: bool,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer_inside: bool,
    running: Arc<AtomicBool>,
}

impl RawHostApp {
    fn handle_command(&mut self, command: RawHostCommand) -> Result<()> {
        match command {
            RawHostCommand::SetInputRegion(region) => {
                self.runtime.input_region = region;
                self.runtime.input_region_applied = false;
                self.buffer = None;
                self.draw()?;
            }
            RawHostCommand::SetVisibleRegion(region) => {
                self.runtime.visible_region = region;
                self.buffer = None;
                self.draw()?;
            }
            RawHostCommand::SetFrame(frame) => {
                self.runtime.frame = Some(frame);
                self.buffer = None;
                self.draw()?;
            }
            RawHostCommand::ClearFrame => {
                self.runtime.frame = None;
                self.buffer = None;
                self.draw()?;
            }
            RawHostCommand::Shutdown => {
                self.exit = true;
                self.running.store(false, Ordering::SeqCst);
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
        self.runtime.input_region_applied = true;
        eprintln!(
            "applied raw wl_surface input-region with {} rect(s): {:?}",
            self.runtime.input_region.rects().len(),
            self.runtime.input_region.rects()
        );
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        let stride = self.runtime.width as i32 * 4;
        let width = self.runtime.width;
        let height = self.runtime.height;

        if !self.runtime.input_region_applied {
            self.apply_input_region()?;
        }

        let buffer = self.buffer.get_or_insert_with(|| {
            self.pool
                .create_buffer(width as i32, height as i32, stride, wl_shm::Format::Argb8888)
                .expect("failed to allocate shm buffer")
                .0
        });

        let canvas = match self.pool.canvas(buffer) {
            Some(canvas) => canvas,
            None => {
                let (next_buffer, canvas) = self
                    .pool
                    .create_buffer(
                        width as i32,
                        height as i32,
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

        for (index, chunk) in canvas.chunks_exact_mut(4).enumerate() {
            let x = (index % width as usize) as i32;
            let y = (index / width as usize) as i32;
            let interactive = self
                .runtime
                .input_region
                .rects()
                .iter()
                .any(|rect| point_in_rect(x, y, rect));
            let visible = visible_region
                .rects()
                .iter()
                .any(|rect| point_in_rect(x, y, rect));
            let color = match frame_ref {
                Some(frame) => color_from_frame_or_background(
                    frame,
                    x,
                    y,
                    visible,
                    self.runtime.transparent_outside_input_region,
                ),
                None => {
                    if interactive {
                        argb(0xFF, 0xFF, 0xD0, 0x3B)
                    } else if visible {
                        argb(0xFF, 0xC9, 0x4D, 0x4D)
                    } else if self.runtime.transparent_outside_input_region {
                        argb(0x00, 0x00, 0x00, 0x00)
                    } else {
                        argb(0xFF, 0x72, 0x12, 0x12)
                    }
                }
            };

            let array: &mut [u8; 4] = chunk.try_into().expect("chunk size must be 4");
            *array = color.to_le_bytes();
        }

        self.window
            .wl_surface()
            .damage_buffer(0, 0, width as i32, height as i32);
        buffer
            .attach_to(self.window.wl_surface())
            .context("failed to attach buffer to wl_surface")?;
        self.window.commit();
        Ok(())
    }
}

fn point_in_rect(x: i32, y: i32, rect: &crate::wayland::input_region::InteractiveRect) -> bool {
    let right = rect.x.saturating_add(rect.width as i32);
    let bottom = rect.y.saturating_add(rect.height as i32);
    x >= rect.x && y >= rect.y && x < right && y < bottom
}

fn argb(a: u32, r: u32, g: u32, b: u32) -> u32 {
    (a << 24) | (r << 16) | (g << 8) | b
}

fn color_from_frame_or_background(
    frame: &RawHostFrame,
    x: i32,
    y: i32,
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

    if x < 0 || y < 0 {
        return argb(0x00, 0x00, 0x00, 0x00);
    }

    let src_x = x as u32;
    let src_y = y as u32;
    if src_x >= frame.width || src_y >= frame.height {
        return if transparent_outside_input_region {
            argb(0x00, 0x00, 0x00, 0x00)
        } else {
            argb(0xFF, 0x72, 0x12, 0x12)
        };
    }

    let base = ((src_y * frame.width + src_x) * 4) as usize;
    let r = frame.rgba[base] as u32;
    let g = frame.rgba[base + 1] as u32;
    let b = frame.rgba[base + 2] as u32;
    let a = frame.rgba[base + 3] as u32;
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
        self.buffer = None;
        self.runtime.width = configure
            .new_size
            .0
            .map(|value| value.get())
            .unwrap_or(self.runtime.width);
        self.runtime.height = configure
            .new_size
            .1
            .map(|value| value.get())
            .unwrap_or(self.runtime.height);
        eprintln!(
            "raw host configure: {}x{}",
            self.runtime.width, self.runtime.height
        );

        if self.first_configure {
            self.first_configure = false;
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
                    if self.runtime.log_pointer_events {
                        eprintln!("pointer enter serial={serial} pos={:?}", event.position);
                    }
                }
                PointerEventKind::Leave { serial } => {
                    self.pointer_inside = false;
                    if self.runtime.log_pointer_events {
                        eprintln!("pointer leave serial={serial}");
                    }
                }
                PointerEventKind::Motion { .. } => {
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
                    if self.runtime.log_pointer_events {
                        eprintln!(
                            "pointer press button={} serial={} pos={:?}",
                            button, serial, event.position
                        );
                    }
                }
                PointerEventKind::Release { button, serial, .. } => {
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

delegate_compositor!(RawHostApp);
delegate_output!(RawHostApp);
delegate_shm!(RawHostApp);
delegate_seat!(RawHostApp);
delegate_keyboard!(RawHostApp);
delegate_pointer!(RawHostApp);
delegate_xdg_shell!(RawHostApp);
delegate_xdg_window!(RawHostApp);
delegate_registry!(RawHostApp);
