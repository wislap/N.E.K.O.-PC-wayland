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

pub trait WindowInputBackend {
    fn strategy(&self) -> &StrategySelection;
    fn apply_input_region(&mut self, region: &InputRegion) -> anyhow::Result<()>;
}

#[derive(Debug)]
pub struct PrototypeBackend {
    strategy: StrategySelection,
}

impl PrototypeBackend {
    pub fn new(profile: &WaylandProfile) -> Self {
        let strategy = choose_strategy(profile);
        Self { strategy }
    }
}

impl WindowInputBackend for PrototypeBackend {
    fn strategy(&self) -> &StrategySelection {
        &self.strategy
    }

    fn apply_input_region(&mut self, region: &InputRegion) -> anyhow::Result<()> {
        eprintln!(
            "prototype Wayland backend received input region with {} rect(s): {:?}",
            region.rects().len(),
            region.rects()
        );
        Ok(())
    }
}

pub fn choose_strategy(profile: &WaylandProfile) -> StrategySelection {
    if !profile.is_wayland() {
        return StrategySelection {
            tier: StrategyTier::StandardWindow,
            reason: "non-Wayland session detected",
        };
    }

    match profile.compositor_family {
        CompositorFamily::Mutter | CompositorFamily::KWin | CompositorFamily::Wlroots | CompositorFamily::Niri => {
            StrategySelection {
                tier: StrategyTier::OverlayNoInputRegion,
                reason: "Wayland session detected; native input-region engine not implemented yet",
            }
        }
        CompositorFamily::Unknown => StrategySelection {
            tier: StrategyTier::StandardWindow,
            reason: "unknown compositor family; conservative fallback",
        },
    }
}
