use serde::Serialize;

use crate::wayland::detect::{CompositorFamily, WaylandProfile};

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
            reason: "Wayland session detected; using raw host input-region path",
        },
        CompositorFamily::Unknown => StrategySelection {
            tier: StrategyTier::StandardWindow,
            reason: "unknown compositor family; conservative fallback",
        },
    }
}
