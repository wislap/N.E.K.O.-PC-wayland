use std::time::Duration;

use anyhow::{Context, Result, bail};
use gdk::prelude::DisplayExtManual;
use gtk::glib::object::ObjectType;
use gtk::prelude::{BoxExt, ContainerExt, CssProviderExt, EventBoxExt, OverlayExt, WidgetExt};
use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
#[cfg(target_os = "linux")]
use tao::platform::unix::WindowExtUnix;
use tao::window::WindowBuilder;

#[derive(Debug, Clone, Copy)]
struct InteractiveRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone)]
enum UserEvent {
    Apply(InteractiveRect),
}

fn main() {
    if let Err(err) = run() {
        eprintln!("tao-region-probe failed: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let rect = InteractiveRect {
        x: 0,
        y: 0,
        width: 220,
        height: 320,
    };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("tao-region-probe")
        .with_decorations(false)
        .with_transparent(false)
        .with_inner_size(LogicalSize::new(800.0, 600.0))
        .build(&event_loop)
        .context("failed to create tao probe window")?;

    install_probe_visual(&window)?;
    dump_window_surface(&window)?;
    schedule_reapply(proxy.clone(), rect);

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                if let Err(err) = apply_input_region_to_window(&window, rect) {
                    eprintln!("initial input-region apply failed: {err:#}");
                }
            }
            Event::UserEvent(UserEvent::Apply(rect)) => {
                if let Err(err) = apply_input_region_to_window(&window, rect) {
                    eprintln!("deferred input-region apply failed: {err:#}");
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    })
}

fn schedule_reapply(proxy: EventLoopProxy<UserEvent>, rect: InteractiveRect) {
    for delay_ms in [100_u64, 350, 1000, 2000] {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(delay_ms));
            let _ = proxy.send_event(UserEvent::Apply(rect));
        });
    }
}

fn install_probe_visual(window: &tao::window::Window) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use tao::platform::unix::WindowExtUnix;

        let gtk_window = window.gtk_window();
        gtk_window.set_app_paintable(true);
        install_probe_css()?;
        let vbox = window
            .default_vbox()
            .context("probe tao window does not provide default GTK vbox")?;
        let overlay = gtk::Overlay::new();
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);

        let panel = gtk::EventBox::new();
        panel.set_visible_window(true);
        panel.set_above_child(false);
        panel.set_widget_name("probe-panel");
        panel.set_size_request(800, 600);

        let label = gtk::Label::new(Some(
            "tao-region-probe\nred area visible\ninput region should only keep 220x320 clickable",
        ));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Start);
        label.set_margin_top(16);
        label.set_margin_start(16);
        label.set_widget_name("probe-label");

        let marker = gtk::EventBox::new();
        marker.set_visible_window(true);
        marker.set_widget_name("probe-marker");
        marker.set_size_request(220, 320);
        marker.set_halign(gtk::Align::Start);
        marker.set_valign(gtk::Align::Start);

        overlay.add(&panel);
        overlay.add_overlay(&marker);
        overlay.add_overlay(&label);

        vbox.pack_start(&overlay, true, true, 0);
        gtk_window.show_all();
    }

    Ok(())
}

fn install_probe_css() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let provider = gtk::CssProvider::new();
        provider
            .load_from_data(
                b"#probe-panel { background: rgba(180, 20, 20, 0.92); }\n#probe-marker { background: rgba(255, 230, 80, 0.92); }\n#probe-label { color: white; font: 16px sans; }",
            )
            .context("failed to load probe CSS")?;

        let screen = gdk::Screen::default().context("failed to get default GDK screen")?;
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    Ok(())
}

fn dump_window_surface(window: &tao::window::Window) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let gtk_window = window.gtk_window();
        gtk_window.realize();
        let gdk_window = gtk_window
            .window()
            .context("probe GTK window has not been realized")?;
        let wl_surface = unsafe {
            gdk_wayland_sys::gdk_wayland_window_get_wl_surface(gdk_window.as_ptr() as *mut _)
        };
        eprintln!(
            "probe surface pointers: gdk_window={:p} wl_surface={:p}",
            gdk_window.as_ptr(),
            wl_surface
        );
    }

    Ok(())
}

fn apply_input_region_to_window(
    window: &tao::window::Window,
    rect: InteractiveRect,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let gtk_window = window.gtk_window();
        gtk_window.realize();
        let display = gtk_window.display();
        if !display.backend().is_wayland() {
            bail!("probe is only meaningful on Wayland");
        }

        let gdk_window = gtk_window
            .window()
            .context("probe GTK window has not been realized")?;
        let wl_compositor = unsafe {
            gdk_wayland_sys::gdk_wayland_display_get_wl_compositor(display.as_ptr() as *mut _)
        };
        let wl_surface = unsafe {
            gdk_wayland_sys::gdk_wayland_window_get_wl_surface(gdk_window.as_ptr() as *mut _)
        };

        if wl_compositor.is_null() || wl_surface.is_null() {
            bail!("failed to fetch probe wl_compositor or wl_surface");
        }

        let wl_region = unsafe {
            wl_proxy_marshal_flags(
                wl_compositor.cast(),
                WL_COMPOSITOR_CREATE_REGION,
                &wl_region_interface,
                wl_proxy_get_version(wl_compositor.cast()),
                0,
            )
        } as *mut wl_region;

        if wl_region.is_null() {
            bail!("probe create_region returned null");
        }

        unsafe {
            wl_proxy_marshal_flags(
                wl_region.cast(),
                WL_REGION_ADD,
                std::ptr::null(),
                wl_proxy_get_version(wl_region.cast()),
                0,
                rect.x,
                rect.y,
                i32::try_from(rect.width).context("probe width overflow")?,
                i32::try_from(rect.height).context("probe height overflow")?,
            );
            wl_proxy_marshal_flags(
                wl_surface.cast(),
                WL_SURFACE_SET_INPUT_REGION,
                std::ptr::null(),
                wl_proxy_get_version(wl_surface.cast()),
                0,
                wl_region,
            );
            wl_proxy_marshal_flags(
                wl_surface.cast(),
                WL_SURFACE_COMMIT,
                std::ptr::null(),
                wl_proxy_get_version(wl_surface.cast()),
                0,
            );
            wl_proxy_marshal_flags(
                wl_region.cast(),
                WL_REGION_DESTROY,
                std::ptr::null(),
                wl_proxy_get_version(wl_region.cast()),
                WL_MARSHAL_FLAG_DESTROY,
            );
        }

        eprintln!(
            "probe applied raw input region: x={} y={} width={} height={}",
            rect.x, rect.y, rect.width, rect.height
        );
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (window, rect);
    }

    Ok(())
}

#[allow(non_camel_case_types)]
enum wl_proxy {}
#[allow(non_camel_case_types)]
enum wl_region {}
#[allow(non_camel_case_types)]
#[repr(C)]
struct wl_interface {
    _private: [u8; 0],
}

const WL_MARSHAL_FLAG_DESTROY: u32 = 1 << 0;
const WL_COMPOSITOR_CREATE_REGION: u32 = 1;
const WL_SURFACE_SET_INPUT_REGION: u32 = 5;
const WL_SURFACE_COMMIT: u32 = 6;
const WL_REGION_DESTROY: u32 = 0;
const WL_REGION_ADD: u32 = 1;

#[link(name = "wayland-client")]
unsafe extern "C" {
    static wl_region_interface: wl_interface;

    fn wl_proxy_marshal_flags(
        proxy: *mut wl_proxy,
        opcode: u32,
        interface: *const wl_interface,
        version: u32,
        flags: u32,
        ...
    ) -> *mut wl_proxy;
    fn wl_proxy_get_version(proxy: *mut wl_proxy) -> u32;
}
