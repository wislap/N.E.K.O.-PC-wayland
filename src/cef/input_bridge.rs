use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use xkeysym::key;

use crate::cef::{CefKeyEventKind, CefMouseButton, CefOsrBridge};
use crate::tray::TrayCommand;
use crate::wayland::input_region::InteractiveRect;
use crate::wayland::raw_host::{
    RawHostHandle, RawHostKeyboardEvent, RawHostModifiers, RawHostPointerButton,
    RawHostPointerEvent,
};

fn idle_pump_sleep(frame_rate: u32) -> Duration {
    let frame_rate = frame_rate.max(1);
    let millis = ((1000 / frame_rate) / 2).clamp(1, 4);
    Duration::from_millis(u64::from(millis))
}

fn input_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("NEKO_CEF_INPUT_TRACE")
            .map(|value| {
                let value = value.trim().to_ascii_lowercase();
                matches!(value.as_str(), "1" | "true" | "yes" | "on")
            })
            .unwrap_or(false)
    })
}

pub struct RawInputSource {
    pub handle: RawHostHandle,
    pub pointer_events: Receiver<RawHostPointerEvent>,
    pub keyboard_events: Receiver<RawHostKeyboardEvent>,
    pub viewport: InteractiveRect,
}

pub fn run_raw_input_loop(
    bridge: &CefOsrBridge,
    handle: &RawHostHandle,
    pointer_events: Receiver<RawHostPointerEvent>,
    keyboard_events: Receiver<RawHostKeyboardEvent>,
) {
    let (width, height) = bridge.render_size();
    run_multi_raw_input_loop(
        bridge,
        vec![RawInputSource {
            handle: handle.clone(),
            pointer_events,
            keyboard_events,
            viewport: InteractiveRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        }],
    );
}

pub fn run_multi_raw_input_loop(bridge: &CefOsrBridge, sources: Vec<RawInputSource>) {
    run_multi_raw_input_loop_with_tray_commands(bridge, sources, None, |_bridge, _command| {});
}

pub fn run_multi_raw_input_loop_with_tray_commands(
    bridge: &CefOsrBridge,
    mut sources: Vec<RawInputSource>,
    tray_commands: Option<&Receiver<TrayCommand>>,
    mut on_tray_command: impl FnMut(&CefOsrBridge, TrayCommand),
) {
    bridge.focus_browser(true);
    bridge.notify_resized();
    let mut mouse_modifiers = 0_u32;
    let mut key_modifiers = 0_u32;
    let mut idle = false;
    let idle_sleep = idle_pump_sleep(bridge.config().frame_rate);

    loop {
        let mut progressed = false;
        let mut any_running = false;

        bridge.do_message_loop_work();
        if let Some(receiver) = tray_commands {
            loop {
                match receiver.try_recv() {
                    Ok(command) => {
                        progressed = true;
                        on_tray_command(bridge, command);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }

        for (index, source) in sources.iter_mut().enumerate() {
            any_running |= source.handle.is_running();

            loop {
                match source.pointer_events.try_recv() {
                    Ok(event) => {
                        progressed = true;
                        if input_trace_enabled() {
                            eprintln!("CEF input pointer event[{index}]: {event:?}");
                        }
                        let event = scale_pointer_event_to_viewport(
                            event,
                            source.handle.surface_size(),
                            &source.viewport,
                        );
                        forward_pointer_event(bridge, event, &mut mouse_modifiers, key_modifiers);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }

            loop {
                match source.keyboard_events.try_recv() {
                    Ok(event) => {
                        progressed = true;
                        if input_trace_enabled() {
                            eprintln!("CEF input keyboard event[{index}]: {event:?}");
                        }
                        key_modifiers = current_key_modifier_flags(&event);
                        forward_keyboard_event(bridge, event);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }

        if !any_running {
            break;
        }

        if progressed {
            idle = false;
            continue;
        }

        if idle {
            thread::sleep(idle_sleep);
        } else {
            idle = true;
        }
    }
}

fn scale_pointer_event_to_viewport(
    event: RawHostPointerEvent,
    window_size: (u32, u32),
    viewport: &InteractiveRect,
) -> RawHostPointerEvent {
    fn scale(value: f64, src: u32, origin: i32, extent: u32) -> f64 {
        if src == 0 || extent == 0 {
            return value + f64::from(origin);
        }
        f64::from(origin) + value * f64::from(extent) / f64::from(src)
    }

    let (window_width, window_height) = window_size;

    match event {
        RawHostPointerEvent::Enter { x, y } => RawHostPointerEvent::Enter {
            x: scale(x, window_width, viewport.x, viewport.width),
            y: scale(y, window_height, viewport.y, viewport.height),
        },
        RawHostPointerEvent::Leave { x, y } => RawHostPointerEvent::Leave {
            x: scale(x, window_width, viewport.x, viewport.width),
            y: scale(y, window_height, viewport.y, viewport.height),
        },
        RawHostPointerEvent::Motion { x, y } => RawHostPointerEvent::Motion {
            x: scale(x, window_width, viewport.x, viewport.width),
            y: scale(y, window_height, viewport.y, viewport.height),
        },
        RawHostPointerEvent::Button {
            x,
            y,
            button,
            pressed,
        } => RawHostPointerEvent::Button {
            x: scale(x, window_width, viewport.x, viewport.width),
            y: scale(y, window_height, viewport.y, viewport.height),
            button,
            pressed,
        },
        RawHostPointerEvent::Wheel {
            x,
            y,
            delta_x,
            delta_y,
        } => RawHostPointerEvent::Wheel {
            x: scale(x, window_width, viewport.x, viewport.width),
            y: scale(y, window_height, viewport.y, viewport.height),
            delta_x,
            delta_y,
        },
    }
}

fn forward_pointer_event(
    bridge: &CefOsrBridge,
    event: RawHostPointerEvent,
    mouse_modifiers: &mut u32,
    key_modifiers: u32,
) {
    const EVENTFLAG_PRECISION_SCROLLING_DELTA: u32 = 1 << 14;

    match event {
        RawHostPointerEvent::Enter { x, y } => {
            bridge.focus_browser(true);
            bridge.send_mouse_move_event(
                x.round() as i32,
                y.round() as i32,
                false,
                key_modifiers | *mouse_modifiers,
            );
        }
        RawHostPointerEvent::Leave { x, y } => {
            bridge.send_mouse_move_event(
                x.round() as i32,
                y.round() as i32,
                true,
                key_modifiers | *mouse_modifiers,
            );
            bridge.focus_browser(false);
        }
        RawHostPointerEvent::Motion { x, y } => {
            bridge.send_mouse_move_event(
                x.round() as i32,
                y.round() as i32,
                false,
                key_modifiers | *mouse_modifiers,
            );
        }
        RawHostPointerEvent::Button {
            x,
            y,
            button,
            pressed,
        } => {
            let Some(cef_button) = map_button(button) else {
                return;
            };
            let flag = button_flag(button);
            if pressed {
                *mouse_modifiers |= flag;
            } else {
                *mouse_modifiers &= !flag;
            }
            bridge.send_mouse_click_event(
                x.round() as i32,
                y.round() as i32,
                cef_button,
                !pressed,
                1,
                key_modifiers | *mouse_modifiers,
            );
        }
        RawHostPointerEvent::Wheel {
            x,
            y,
            delta_x,
            delta_y,
        } => {
            bridge.send_mouse_wheel_event(
                x.round() as i32,
                y.round() as i32,
                delta_x.round() as i32,
                delta_y.round() as i32,
                key_modifiers | *mouse_modifiers | EVENTFLAG_PRECISION_SCROLLING_DELTA,
            );
        }
    }
}

fn forward_keyboard_event(bridge: &CefOsrBridge, event: RawHostKeyboardEvent) {
    match event {
        RawHostKeyboardEvent::Press {
            raw_code,
            keysym,
            utf8,
            modifiers,
        } => {
            let cef_modifiers = map_modifiers(modifiers, keysym, false);
            let windows_key_code = map_windows_key_code(keysym, utf8.as_deref());
            bridge.send_key_event(
                CefKeyEventKind::RawKeyDown,
                windows_key_code,
                raw_code as i32,
                cef_modifiers,
                0,
                0,
            );
            if let Some(text) = utf8 {
                for ch in text.encode_utf16() {
                    bridge.send_key_event(
                        CefKeyEventKind::Char,
                        windows_key_code,
                        raw_code as i32,
                        cef_modifiers,
                        ch,
                        ch,
                    );
                }
            }
        }
        RawHostKeyboardEvent::Repeat {
            raw_code,
            keysym,
            utf8,
            modifiers,
        } => {
            let cef_modifiers = map_modifiers(modifiers, keysym, true);
            let windows_key_code = map_windows_key_code(keysym, utf8.as_deref());
            bridge.send_key_event(
                CefKeyEventKind::RawKeyDown,
                windows_key_code,
                raw_code as i32,
                cef_modifiers,
                0,
                0,
            );
            if let Some(text) = utf8 {
                for ch in text.encode_utf16() {
                    bridge.send_key_event(
                        CefKeyEventKind::Char,
                        windows_key_code,
                        raw_code as i32,
                        cef_modifiers,
                        ch,
                        ch,
                    );
                }
            }
        }
        RawHostKeyboardEvent::Release {
            raw_code,
            keysym,
            modifiers,
        } => {
            let cef_modifiers = map_modifiers(modifiers, keysym, false);
            let windows_key_code = map_windows_key_code(keysym, None);
            bridge.send_key_event(
                CefKeyEventKind::KeyUp,
                windows_key_code,
                raw_code as i32,
                cef_modifiers,
                0,
                0,
            );
        }
    }
}

fn map_button(button: RawHostPointerButton) -> Option<CefMouseButton> {
    match button {
        RawHostPointerButton::Left => Some(CefMouseButton::Left),
        RawHostPointerButton::Middle => Some(CefMouseButton::Middle),
        RawHostPointerButton::Right => Some(CefMouseButton::Right),
        RawHostPointerButton::Other(_) => None,
    }
}

fn button_flag(button: RawHostPointerButton) -> u32 {
    match button {
        RawHostPointerButton::Left => 1 << 4,
        RawHostPointerButton::Middle => 1 << 5,
        RawHostPointerButton::Right => 1 << 6,
        RawHostPointerButton::Other(_) => 0,
    }
}

fn map_modifiers(modifiers: RawHostModifiers, keysym: u32, is_repeat: bool) -> u32 {
    let mut flags = 0_u32;
    if modifiers.caps_lock {
        flags |= 1 << 0;
    }
    if modifiers.shift {
        flags |= 1 << 1;
    }
    if modifiers.ctrl {
        flags |= 1 << 2;
    }
    if modifiers.alt {
        flags |= 1 << 3;
    }
    if modifiers.logo {
        flags |= 1 << 7;
    }
    if modifiers.num_lock {
        flags |= 1 << 8;
    }
    if is_repeat {
        flags |= 1 << 13;
    }
    if xkeysym::Keysym::new(keysym).is_keypad_key() {
        flags |= 1 << 9;
    }
    flags
}

fn current_key_modifier_flags(event: &RawHostKeyboardEvent) -> u32 {
    match event {
        RawHostKeyboardEvent::Press {
            modifiers, keysym, ..
        }
        | RawHostKeyboardEvent::Repeat {
            modifiers, keysym, ..
        }
        | RawHostKeyboardEvent::Release {
            modifiers, keysym, ..
        } => map_modifiers(*modifiers, *keysym, false),
    }
}

fn map_windows_key_code(keysym: u32, utf8: Option<&str>) -> i32 {
    match keysym {
        k if k == key::BackSpace => 0x08,
        k if k == key::Tab => 0x09,
        k if k == key::Return => 0x0D,
        k if k == key::Escape => 0x1B,
        k if k == key::space => 0x20,
        k if k == key::Prior => 0x21,
        k if k == key::Next => 0x22,
        k if k == key::End => 0x23,
        k if k == key::Home => 0x24,
        k if k == key::Left => 0x25,
        k if k == key::Up => 0x26,
        k if k == key::Right => 0x27,
        k if k == key::Down => 0x28,
        k if k == key::Insert => 0x2D,
        k if k == key::Delete => 0x2E,
        k if k == key::KP_0 => 0x60,
        k if k == key::KP_1 => 0x61,
        k if k == key::KP_2 => 0x62,
        k if k == key::KP_3 => 0x63,
        k if k == key::KP_4 => 0x64,
        k if k == key::KP_5 => 0x65,
        k if k == key::KP_6 => 0x66,
        k if k == key::KP_7 => 0x67,
        k if k == key::KP_8 => 0x68,
        k if k == key::KP_9 => 0x69,
        k if k == key::KP_Multiply => 0x6A,
        k if k == key::KP_Add => 0x6B,
        k if k == key::KP_Subtract => 0x6D,
        k if k == key::KP_Decimal => 0x6E,
        k if k == key::KP_Divide => 0x6F,
        k if k == key::F1 => 0x70,
        k if k == key::F2 => 0x71,
        k if k == key::F3 => 0x72,
        k if k == key::F4 => 0x73,
        k if k == key::F5 => 0x74,
        k if k == key::F6 => 0x75,
        k if k == key::F7 => 0x76,
        k if k == key::F8 => 0x77,
        k if k == key::F9 => 0x78,
        k if k == key::F10 => 0x79,
        k if k == key::F11 => 0x7A,
        k if k == key::F12 => 0x7B,
        k if k == key::Shift_L || k == key::Shift_R => 0x10,
        k if k == key::Control_L || k == key::Control_R => 0x11,
        k if k == key::Alt_L || k == key::Alt_R => 0x12,
        k if k == key::Super_L || k == key::Super_R => 0x5B,
        _ => {
            if let Some(text) = utf8.and_then(|value| value.chars().next()) {
                match text {
                    '0'..='9' => text as i32,
                    'a'..='z' => text.to_ascii_uppercase() as i32,
                    'A'..='Z' => text as i32,
                    _ => keysym as i32,
                }
            } else {
                keysym as i32
            }
        }
    }
}
