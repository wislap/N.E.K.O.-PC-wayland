use serde::Serialize;
use std::env;
use std::ffi::c_void;

use crate::wayland::detect::{CompositorFamily, WaylandProfile};
use crate::wayland::input_region::InputRegion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StrategyTier {
    NativeInputRegion,
    OverlayNoInputRegion,
    StandardWindow,
}

#[derive(Debug, Clone, Serialize)]
pub struct StrategySelection {
    pub tier: StrategyTier,
    pub reason: &'static str,
}

pub fn choose_strategy(profile: &WaylandProfile) -> StrategySelection {
    if !profile.is_wayland() {
        return StrategySelection {
            tier: StrategyTier::StandardWindow,
            reason: "non-Wayland session detected",
        };
    }

    match profile.compositor_family {
        CompositorFamily::Mutter
        | CompositorFamily::KWin
        | CompositorFamily::Wlroots
        | CompositorFamily::Niri => StrategySelection {
            tier: StrategyTier::NativeInputRegion,
            reason: "Wayland session detected; using GTK/GDK input-shape bridge",
        },
        CompositorFamily::Unknown => StrategySelection {
            tier: StrategyTier::StandardWindow,
            reason: "unknown compositor family; conservative fallback",
        },
    }
}

#[cfg(target_os = "linux")]
pub fn apply_input_region_to_window(
    window: &tao::window::Window,
    region: &InputRegion,
) -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    use cairo::{RectangleInt, Region};
    use gdk::prelude::DisplayExtManual;
    use gtk::glib::object::ObjectType;
    use gtk::prelude::WidgetExt;
    use tao::platform::unix::WindowExtUnix;

    let gtk_window = window.gtk_window();
    gtk_window.realize();
    let trace = trace_input_region();

    let display = gtk_window.display();
    if !display.supports_input_shapes() {
        bail!("GDK display does not report input-shape support");
    }

    let gdk_window = gtk_window
        .window()
        .context("GTK window has not been realized into a GDK window yet")?;

    if display.backend().is_wayland() {
        return apply_wayland_input_region(
            window,
            gtk_window.display().as_ptr() as *mut _,
            gdk_window.as_ptr() as *mut _,
            region,
        );
    }

    if trace {
        eprintln!(
            "applying GTK/GDK input region to window {:?}: {} rect(s): {:?}",
            window.id(),
            region.rects().len(),
            region.rects()
        );
    }

    if region.is_empty() {
        let empty_region = Region::create();
        gdk_window.set_pass_through(true);
        gdk_window.input_shape_combine_region(&empty_region, 0, 0);
        gtk_window.input_shape_combine_region(Some(&empty_region));
        if trace {
            eprintln!("applied empty input region; window should become fully click-through");
        }
        return Ok(());
    }

    let rectangles = region
        .rects()
        .iter()
        .map(|rect| RectangleInt::new(rect.x, rect.y, rect.width as i32, rect.height as i32))
        .collect::<Vec<_>>();
    let cairo_region = Region::create_rectangles(&rectangles);

    gdk_window.set_pass_through(false);
    gdk_window.input_shape_combine_region(&cairo_region, 0, 0);
    gtk_window.input_shape_combine_region(Some(&cairo_region));

    if trace {
        eprintln!(
            "applied non-empty input region; window should accept input only inside declared rects"
        );
    }

    Ok(())
}

#[cfg(target_os = "linux")]
pub fn maybe_dump_widget_tree(window: &tao::window::Window) {
    use gdk::prelude::DisplayExtManual;
    use gtk::glib::Cast;
    use gtk::prelude::WidgetExt;
    use tao::platform::unix::WindowExtUnix;

    if !matches!(
        env::var("NEKO_WAYLAND_TRACE_WIDGET_TREE").ok().as_deref(),
        Some("1")
            | Some("true")
            | Some("TRUE")
            | Some("yes")
            | Some("YES")
            | Some("on")
            | Some("ON")
    ) {
        return;
    }

    let gtk_window = window.gtk_window();
    gtk_window.realize();
    eprintln!("GTK widget tree for window {:?}:", window.id());
    dump_widget(&gtk_window.clone().upcast::<gtk::Widget>(), 0);

    fn dump_widget(widget: &gtk::Widget, depth: usize) {
        use gtk::glib::Cast;
        use gtk::glib::object::ObjectExt;
        use gtk::glib::object::ObjectType;
        use gtk::prelude::{ContainerExt, WidgetExt};

        let indent = "  ".repeat(depth);
        let type_name = widget.type_().name();
        let widget_name = widget.widget_name();
        let has_window = widget.has_window();
        let gdk_window = widget.window();
        let gdk_ptr = gdk_window
            .as_ref()
            .map(|w| format!("{:p}", w.as_ptr()))
            .unwrap_or_else(|| "null".to_string());
        let wl_surface_ptr = gdk_window.as_ref().and_then(|gdk_window| {
            if !gdk_window.display().backend().is_wayland() {
                return None;
            }
            let ptr = unsafe {
                gdk_wayland_sys::gdk_wayland_window_get_wl_surface(gdk_window.as_ptr() as *mut _)
            };
            (!ptr.is_null()).then_some(ptr)
        });
        let wl_surface_ptr = wl_surface_ptr
            .map(|ptr| format!("{ptr:p}"))
            .unwrap_or_else(|| "null".to_string());

        eprintln!(
            "{indent}- type={type_name} widget_name={widget_name:?} has_window={has_window} gdk_window={gdk_ptr} wl_surface={wl_surface_ptr}"
        );

        if let Ok(container) = widget.clone().dynamic_cast::<gtk::Container>() {
            for child in container.children() {
                dump_widget(&child, depth + 1);
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn maybe_dump_widget_tree(_window: &tao::window::Window) {}

#[cfg(not(target_os = "linux"))]
pub fn apply_input_region_to_window(
    _window: &tao::window::Window,
    _region: &InputRegion,
) -> anyhow::Result<()> {
    Ok(())
}

fn trace_input_region() -> bool {
    matches!(
        env::var("NEKO_WAYLAND_TRACE_INPUT_REGION").ok().as_deref(),
        Some("1")
            | Some("true")
            | Some("TRUE")
            | Some("yes")
            | Some("YES")
            | Some("on")
            | Some("ON")
    )
}

#[cfg(target_os = "linux")]
fn apply_wayland_input_region(
    window: &tao::window::Window,
    gdk_display_ptr: *mut c_void,
    gdk_window_ptr: *mut c_void,
    region: &InputRegion,
) -> anyhow::Result<()> {
    use anyhow::{Context, bail};

    let trace = trace_input_region();

    let wl_compositor = unsafe {
        gdk_wayland_sys::gdk_wayland_display_get_wl_compositor(gdk_display_ptr as *mut _)
    };
    if wl_compositor.is_null() {
        bail!("gdk_wayland_display_get_wl_compositor returned null");
    }

    let wl_surface =
        unsafe { gdk_wayland_sys::gdk_wayland_window_get_wl_surface(gdk_window_ptr as *mut _) };
    if wl_surface.is_null() {
        bail!("gdk_wayland_window_get_wl_surface returned null");
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
        bail!("wl_compositor_create_region returned null");
    }

    for rect in region.rects() {
        unsafe {
            wl_proxy_marshal_flags(
                wl_region.cast(),
                WL_REGION_ADD,
                std::ptr::null(),
                wl_proxy_get_version(wl_region.cast()),
                0,
                rect.x,
                rect.y,
                i32::try_from(rect.width).context("input region width does not fit i32")?,
                i32::try_from(rect.height).context("input region height does not fit i32")?,
            );
        }
    }

    unsafe {
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

    if trace {
        eprintln!(
            "applied raw Wayland input region to window {:?}: {} rect(s): {:?}",
            window.id(),
            region.rects().len(),
            region.rects()
        );
    }

    Ok(())
}

#[cfg(target_os = "linux")]
#[allow(non_camel_case_types)]
enum wl_proxy {}
#[cfg(target_os = "linux")]
#[allow(non_camel_case_types)]
enum wl_compositor {}
#[cfg(target_os = "linux")]
#[allow(non_camel_case_types)]
enum wl_surface {}
#[cfg(target_os = "linux")]
#[allow(non_camel_case_types)]
enum wl_region {}
#[cfg(target_os = "linux")]
#[allow(non_camel_case_types)]
struct wl_interface {
    _private: [u8; 0],
}

#[cfg(target_os = "linux")]
const WL_MARSHAL_FLAG_DESTROY: u32 = 1 << 0;
#[cfg(target_os = "linux")]
const WL_COMPOSITOR_CREATE_REGION: u32 = 1;
#[cfg(target_os = "linux")]
const WL_SURFACE_SET_INPUT_REGION: u32 = 5;
#[cfg(target_os = "linux")]
const WL_SURFACE_COMMIT: u32 = 6;
#[cfg(target_os = "linux")]
const WL_REGION_DESTROY: u32 = 0;
#[cfg(target_os = "linux")]
const WL_REGION_ADD: u32 = 1;

#[cfg(target_os = "linux")]
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
