# N.E.K.O.-PC-wayland

Wayland-native desktop host prototype for `N.E.K.O`.

## Current scope

This project is the first Rust host skeleton for Linux Wayland. It intentionally starts small:

- discovers the sibling `N.E.K.O` repo
- opens the configured frontend URL in a native Linux webview window
- injects a minimal host/frontend IPC bridge as `window.NekoHost`
- keeps the Wayland strategy/input-region design scaffold for future native window work

It does **not** yet implement transparent desktop-pet behavior, input-region shaping, tray integration, or compositor-specific Wayland handling.

## Why this exists

The Electron shell in `N.E.K.O.-PC` works well on Windows because the transparent overlay + click-through model maps well to Win32 behavior. Under native Linux Wayland, that model is not reliable enough across GNOME, KDE, Hyprland, Sway, and niri.

This Rust host is the first step toward a better architecture:

- UI: keep the existing HTML/CSS/JS frontend
- Backend: out of scope for this binary right now
- Desktop host: move Linux windowing responsibility into a native Rust layer

## Requirements

- Rust toolchain
- `webkit2gtk-4.1` development package
- `gtk4`
- `N.E.K.O` repository available locally

This machine already has:

- `webkit2gtk-4.1 = 2.50.4`
- `gtk4 = 4.20.3`

## Running

From this directory:

```bash
cargo run
```

Or run the explicit binary target:

```bash
cargo run --bin neko-pc-wayland
```

If repo auto-discovery fails:

```bash
NEKO_REPO_ROOT=/path/to/N.E.K.O cargo run
```

If the frontend is exposed on a different URL:

```bash
NEKO_FRONTEND_URL=http://127.0.0.1:48911/ cargo run --bin neko-pc-wayland
```

## IPC bridge

The host injects `window.NekoHost` into the page.

Frontend -> host:

```js
window.NekoHost.postMessage({ cmd: 'ping', nonce: 1 })
window.NekoHost.postMessage({ cmd: 'get_host_info' })
window.NekoHost.postMessage({ cmd: 'log', message: 'hello from frontend' })
window.NekoHost.setInputRegion([
  { x: 120, y: 80, width: 320, height: 480 },
  { x: 420, y: 420, width: 360, height: 180 }
])
```

Host -> frontend:

```js
const off = window.NekoHost.addEventListener((payload) => {
  console.log('host event', payload)
})

window.addEventListener('neko-host-message', (event) => {
  console.log('host event via DOM', event.detail)
})
```

The host currently emits:

- `ready`
- `pong`
- `host_info`
- `log_ack`
- `input_region_applied`
- `error`

## What to build next

The next meaningful milestones are:

1. add a loading window and structured logs
2. add compositor detection
3. build a real Wayland-native input-region engine
4. route interactive region geometry over the new IPC bridge
