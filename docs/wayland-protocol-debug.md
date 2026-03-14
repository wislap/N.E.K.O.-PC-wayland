# Wayland Protocol Debug

Use this when the window looks transparent but still blocks clicks.

## Quick Run

```bash
cd /mnt/k_disk/programe/N.E.K.O.-PC-wayland
./scripts/trace-wayland-input-region.sh
```

This writes two files under `target/wayland-traces/`:

- `trace-*.log`: full stdout/stderr, including `WAYLAND_DEBUG=1`
- `trace-*.focus.log`: filtered lines for `wl_surface`, `wl_region`, `set_input_region`, `commit`, and host logs

## What To Look For

The current host should emit:

```text
GTK widget tree for window ...
applied raw Wayland input region to window ...
```

The protocol log should also show:

```text
wl_compositor@...create_region(...)
wl_region@...add(...)
wl_surface@...set_input_region(wl_region@...)
wl_surface@...commit()
```

## Failure Patterns

If clicks still do not pass through, compare the trace around the first `set_input_region`:

1. If there is no `set_input_region`, the host never reached the raw Wayland path.
2. If `set_input_region` appears but is immediately followed by a later `set_input_region(NULL)` or a different region on the same `wl_surface`, another layer is overwriting it.
3. If `set_input_region` appears on one `wl_surface`, but input is still blocked and another `wl_surface@...commit()` stream seems to carry the real content, the host may still be targeting the wrong effective surface.
4. If the protocol looks correct and stable, the compositor may simply not honor this setup for this GTK/WebKit toplevel.

## Custom Region

You can override the debug region:

```bash
NEKO_WAYLAND_DEBUG_INPUT_REGION='0,0,220,320' ./scripts/trace-wayland-input-region.sh
```
