# Wayland Native Window / Input-Region Design

## Goal

Design a Linux Wayland-native desktop host layer for `N.E.K.O.-PC-wayland` that can eventually support:

- native Wayland session detection
- compositor-aware behavior selection
- transparent desktop-hosted windowing
- explicit input-region control instead of Electron-style whole-window click-through
- a single distributed AppImage with runtime branching for common compositors

This document focuses on the window/input-region layer only. The frontend remains HTML/CSS/JS, and the backend remains the existing Python service stack.

## Why Electron's approach is insufficient on Wayland

The current Electron shell relies on:

- large transparent top-level window
- `setIgnoreMouseEvents(true/false)`
- web-side hit-testing to decide whether input should pass through

That maps well to Windows, but under native Wayland the compositor is the ultimate authority for:

- surface roles
- layer placement
- focus
- input routing
- region clipping

So the replacement strategy must be:

- stop treating "input transparency" as a browser-window toggle
- start treating it as a compositor-visible input region

## Architecture

There are four layers:

1. `Host App`
   The current Rust desktop process. Responsible for startup, backend lifecycle, logging, config, and UI hosting.

2. `Wayland Runtime Detection`
   Detects session/compositor/runtime capabilities and selects a strategy.

3. `Wayland Window Engine`
   Owns compositor-facing primitives:
   - surface role choice
   - layer or xdg-shell strategy
   - transparent surface creation
   - input-region updates

4. `Web UI Bridge`
   Receives geometry/hit-shape information from the frontend and converts it into native region updates.

## Runtime strategy

We should not pretend that all Wayland compositors are equal.

Runtime selection should branch on:

- session type: Wayland or not
- compositor family
- protocol support

### Supported target families

1. `Mutter`
   GNOME family.

2. `KWin`
   KDE Plasma family.

3. `wlroots`
   Sway, Hyprland and related ecosystems.

4. `niri`
   Needs separate validation even if some assumptions overlap with wlroots-style environments.

## Strategy tiers

The window engine should not expose compositor-specific details upward. Instead it should expose a strategy tier.

### Tier A: Native input-region

Desired end state.

- transparent native surface
- explicit input region
- model/dialog areas receive input
- empty transparent regions do not intercept input

### Tier B: Native overlay without input-region

Fallback if the compositor/window stack allows overlay placement but does not behave well enough for selective click-through.

- transparent window
- regular input behavior
- smaller bounded host window only around active content

### Tier C: Standard application window

Last-resort compatibility mode.

- ordinary Wayland app window
- no desktop-pet illusion
- still runs the same frontend/backend stack

## Input-region model

The model should be frontend-driven but host-owned.

The frontend already knows:

- model bounds
- dialog bounds
- popup bounds
- hit-test approximations

The native host should receive a compact description:

- visible content rectangles
- optional rounded corners or masks later
- "interactive only" regions

### Phase 1 shape model

Use rectangle unions only:

- one or more `InteractiveRect`
- optional "content bounds"

This is enough to get the pipeline working.

### Phase 2 shape model

Add:

- rounded rects
- polygon masks
- explicit non-interactive holes

## Proposed data flow

1. Frontend calculates current interactive zones.
2. Frontend sends geometry update to the host.
3. Host normalizes and diffs the region set.
4. Wayland engine updates the surface input region.
5. Compositor routes input only to those regions.

## Web bridge design

The web bridge should eventually support messages like:

- `window_input_region.set`
- `window_input_region.clear`
- `window_mode.set`
- `window_anchor.set`

The first milestone only needs:

- `set_input_region(rects[])`

## Rust module plan

### `wayland::detect`

Responsibilities:

- read environment hints
- classify compositor family
- provide a stable `WaylandProfile`

### `wayland::input_region`

Responsibilities:

- hold region types
- normalize and diff updates
- provide validation helpers

### `wayland::strategy`

Responsibilities:

- select strategy tier
- describe compositor-facing behavior choices in plain data

## Implementation phases

### Phase 0: Design skeleton

- detection structs
- strategy structs
- input-region request model
- app startup logs selected profile

### Phase 1: Web bridge

- host-side IPC endpoint
- frontend test page or JS injection
- region update logging only

### Phase 2: Real region plumbing

- add Wayland client dependencies
- create compositor-facing engine
- set input region on native surfaces

### Phase 3: Compositor tuning

- validate GNOME/KDE/Hyprland/Sway/niri
- add strategy-specific quirks

## Libraries to evaluate

- `wayland-client`
- `wayland-protocols`
- `smithay-client-toolkit`

`smithay-client-toolkit` is the best candidate for early implementation because it provides higher-level Wayland client ergonomics while still exposing the primitives we need.

## Current recommendation

Keep the rendering path centered on CEF OSR plus the native Wayland host. The important architectural shift is:

- browser layer renders UI
- native Wayland layer owns window/input behavior

That separation is what lets us keep one AppImage while still branching per compositor at runtime.

## Current implementation status

The repository now contains a `wayland::raw_host` module that proves the native route works on real Wayland sessions:

- pure Rust Wayland `xdg_toplevel`
- explicit `wl_surface.set_input_region`
- transparent pixels outside the interactive region
- native `xdg_toplevel.move` from the interactive region
- runtime `set_input_region(...)` updates through a host control handle
- separate visible-region and input-region handling inside the native host
- runtime RGBA frame submission through the native host control handle

The main `neko-pc-wayland` app now defaults to the native raw-host + CEF OSR route.

The app now also supports an explicit host mode switch:

- `NEKO_WAYLAND_HOST_MODE=raw_only`
  Starts the native Wayland host directly from the main app entry.
  If no explicit input region is provided yet, the app now falls back to a built-in debug region and visible debug background so the host is not mistaken for an empty transparent window.

- `NEKO_WAYLAND_HOST_MODE=official_helper_probe`
  Runs the official helper probing path.

- `NEKO_WAYLAND_HOST_MODE=official_helper_run`
  Runs the official helper runtime path.

- `NEKO_WAYLAND_HOST_MODE=c_standalone_probe`
  Runs the standalone helper probe path.
