use serde::Serialize;

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
    use gtk::prelude::WidgetExt;
    use tao::platform::unix::WindowExtUnix;

    let gtk_window = window.gtk_window();
    gtk_window.realize();

    let display = gtk_window.display();
    if !display.supports_input_shapes() {
        bail!("GDK display does not report input-shape support");
    }

    let gdk_window = gtk_window
        .window()
        .context("GTK window has not been realized into a GDK window yet")?;

    if region.is_empty() {
        let empty_region = Region::create();
        gdk_window.set_pass_through(true);
        gdk_window.input_shape_combine_region(&empty_region, 0, 0);
        gtk_window.input_shape_combine_region(Some(&empty_region));
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

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn apply_input_region_to_window(
    _window: &tao::window::Window,
    _region: &InputRegion,
) -> anyhow::Result<()> {
    Ok(())
}
