use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wayland::detect::WaylandProfile;
use crate::wayland::engine::StrategySelection;
use crate::wayland::input_region::{InputRegion, InteractiveRect};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostEvent {
    Ready {
        profile: WaylandProfileSnapshot,
        strategy: StrategySelectionSnapshot,
        capabilities: HostCapabilitiesSnapshot,
        dark_mode: bool,
    },
    Pong {
        nonce: Option<Value>,
    },
    HostInfo {
        profile: WaylandProfileSnapshot,
        strategy: StrategySelectionSnapshot,
        capabilities: HostCapabilitiesSnapshot,
        dark_mode: bool,
    },
    LogAck {
        message: String,
    },
    InputRegionApplied {
        rect_count: usize,
    },
    ScreenInfo {
        current_display_id: Option<String>,
        displays: Vec<DisplaySnapshot>,
    },
    WindowState {
        state: WindowStateSnapshot,
    },
    DarkModeChanged {
        enabled: bool,
    },
    ExternalOpened {
        url: String,
    },
    ClipboardText {
        text: Option<String>,
    },
    FileDialogResult {
        paths: Vec<String>,
    },
    SaveDialogResult {
        path: Option<String>,
    },
    FileRead {
        path: String,
        name: String,
        content_base64: String,
    },
    FileWriteComplete {
        path: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum FrontendCommand {
    Ping { nonce: Option<Value> },
    GetHostInfo,
    Log { message: String },
    SetInputRegion { rects: Vec<InteractiveRectPayload> },
    GetScreenInfo,
    GetWindowState,
    GetDarkMode,
    SetDarkMode { enabled: bool },
    OpenExternal { url: String },
    GetClipboardText,
    SetClipboardText { text: String },
    MoveWindowToDisplay { screen_x: i32, screen_y: i32 },
    OpenFileDialog {
        directory: bool,
        multiple: bool,
        title: Option<String>,
        filters: Vec<FileDialogFilterPayload>,
    },
    SaveFileDialog {
        title: Option<String>,
        suggested_name: Option<String>,
        directory: Option<String>,
        filters: Vec<FileDialogFilterPayload>,
    },
    ReadFile {
        path: String,
    },
    WriteFile {
        path: String,
        content_base64: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InteractiveRectPayload {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl From<InteractiveRectPayload> for InteractiveRect {
    fn from(value: InteractiveRectPayload) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WaylandProfileSnapshot {
    pub session_type: String,
    pub compositor_family: String,
    pub current_desktop: Option<String>,
    pub session_desktop: Option<String>,
    pub wayland_display: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StrategySelectionSnapshot {
    pub tier: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostCapabilitiesSnapshot {
    pub input_region: bool,
    pub screen_info: bool,
    pub window_state: bool,
    pub dark_mode: bool,
    pub open_external: bool,
    pub clipboard: bool,
    pub move_window: bool,
    pub file_dialog: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisplaySnapshot {
    pub id: String,
    pub name: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowStateSnapshot {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub fullscreen: bool,
    pub maximized: bool,
    pub visible: bool,
    pub focused: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileDialogFilterPayload {
    pub name: String,
    pub extensions: Vec<String>,
}

impl From<&WaylandProfile> for WaylandProfileSnapshot {
    fn from(value: &WaylandProfile) -> Self {
        Self {
            session_type: format!("{:?}", value.session_type).to_ascii_lowercase(),
            compositor_family: format!("{:?}", value.compositor_family).to_ascii_lowercase(),
            current_desktop: value.current_desktop.clone(),
            session_desktop: value.session_desktop.clone(),
            wayland_display: value.wayland_display.clone(),
        }
    }
}

impl From<&StrategySelection> for StrategySelectionSnapshot {
    fn from(value: &StrategySelection) -> Self {
        Self {
            tier: format!("{:?}", value.tier).to_ascii_lowercase(),
            reason: value.reason.to_string(),
        }
    }
}

pub fn init_script() -> &'static str {
    r#"
(function () {
  const listeners = new Set();
  const TRANSPARENT_PIXEL = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==';
  let latestScreenInfo = null;
  let latestWindowState = null;
  let latestDarkMode = false;
  function emit(payload) {
    if (payload && payload.type === 'screen_info') latestScreenInfo = payload;
    if (payload && payload.type === 'window_state') latestWindowState = payload;
    if (payload && payload.type === 'dark_mode_changed') latestDarkMode = !!payload.enabled;
    for (const listener of listeners) {
      try { listener(payload); } catch (err) { console.error('[NekoHost] listener error', err); }
    }
    window.dispatchEvent(new CustomEvent('neko-host-message', { detail: payload }));
  }
  function waitForEvent(expectedType, message, timeoutMs = 3000) {
    return new Promise((resolve, reject) => {
      const off = window.NekoHost.addEventListener((payload) => {
        if (!payload || !payload.type) return;
        if (payload.type === 'error') {
          clearTimeout(timer);
          off();
          reject(new Error(payload.message || 'host error'));
          return;
        }
        if (payload.type !== expectedType) return;
        clearTimeout(timer);
        off();
        resolve(payload);
      });
      const timer = setTimeout(() => {
        off();
        reject(new Error('timeout waiting for host event: ' + expectedType));
      }, timeoutMs);
      try {
        window.NekoHost.postMessage(message);
      } catch (err) {
        clearTimeout(timer);
        off();
        reject(err);
      }
    });
  }
  function normalizeFilter(filter) {
    return {
      name: String(filter && filter.name ? filter.name : 'Files'),
      extensions: Array.isArray(filter && filter.extensions)
        ? filter.extensions.map((value) => String(value).replace(/^\./, '')).filter(Boolean)
        : []
      };
  }
  function normalizePickerTypes(types) {
    if (!Array.isArray(types)) return [];
    return types.flatMap((item) => {
      if (!item || typeof item !== 'object') return [];
      const accept = item.accept && typeof item.accept === 'object' ? item.accept : {};
      const extensions = Object.values(accept)
        .flatMap((value) => Array.isArray(value) ? value : [])
        .map((value) => String(value).replace(/^\./, ''))
        .filter(Boolean);
      return extensions.length > 0
        ? [{ name: String(item.description || 'Files'), extensions }]
        : [];
    });
  }
  function base64ToUint8Array(base64) {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
      bytes[i] = binary.charCodeAt(i);
    }
    return bytes;
  }
  function uint8ArrayToBase64(bytes) {
    let binary = '';
    for (let i = 0; i < bytes.length; i += 1) {
      binary += String.fromCharCode(bytes[i]);
    }
    return btoa(binary);
  }
  async function readHostFile(path) {
    return waitForEvent('file_read', { cmd: 'read_file', path: String(path) });
  }
  async function writeHostFile(path, contentBase64) {
    return waitForEvent('file_write_complete', {
      cmd: 'write_file',
      path: String(path),
      content_base64: String(contentBase64),
    });
  }
  function basename(path) {
    const normalized = String(path).replace(/\\/g, '/');
    return normalized.split('/').pop() || normalized;
  }
  function createFileHandle(path) {
    const normalizedPath = String(path);
    return {
      kind: 'file',
      name: basename(normalizedPath),
      path: normalizedPath,
      async getFile() {
        const payload = await readHostFile(normalizedPath);
        const bytes = base64ToUint8Array(payload.content_base64 || '');
        return new File([bytes], payload.name || basename(normalizedPath));
      },
      async createWritable() {
        let buffer = new Uint8Array(0);
        return {
          async write(data) {
            if (typeof data === 'string') {
              buffer = new TextEncoder().encode(data);
              return;
            }
            if (data instanceof Blob) {
              buffer = new Uint8Array(await data.arrayBuffer());
              return;
            }
            if (data instanceof ArrayBuffer) {
              buffer = new Uint8Array(data);
              return;
            }
            if (ArrayBuffer.isView(data)) {
              buffer = new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
              return;
            }
            if (data && typeof data === 'object' && data.type === 'write' && data.data != null) {
              return this.write(data.data);
            }
            throw new TypeError('Unsupported writable payload');
          },
          async close() {
            await writeHostFile(normalizedPath, uint8ArrayToBase64(buffer));
          }
        };
      }
    };
  }
  function mapDisplay(display, index) {
    return {
      id: display.id,
      label: display.name || ('Display ' + (index + 1)),
      name: display.name || ('Display ' + (index + 1)),
      x: display.x,
      y: display.y,
      width: display.width,
      height: display.height,
      bounds: { x: display.x, y: display.y, width: display.width, height: display.height },
      workArea: { x: display.x, y: display.y, width: display.width, height: display.height },
      scaleFactor: display.scale_factor,
      primary: index === 0,
      isCurrent: !!display.is_current,
    };
  }
  async function getMappedDisplays() {
    const payload = await waitForEvent('screen_info', { cmd: 'get_screen_info' });
    const displays = Array.isArray(payload.displays) ? payload.displays : [];
    return displays.map(mapDisplay);
  }
  window.__NEKO_HOST_EMIT__ = emit;
  window.NekoHost = {
    postMessage(message) {
      const payload = typeof message === 'string' ? message : JSON.stringify(message);
      if (!window.ipc || typeof window.ipc.postMessage !== 'function') {
        throw new Error('window.ipc.postMessage is unavailable');
      }
      window.ipc.postMessage(payload);
    },
    addEventListener(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    removeEventListener(listener) {
      listeners.delete(listener);
    },
    setInputRegion(rects) {
      window.NekoHost.postMessage({ cmd: 'set_input_region', rects });
    },
    getHostInfo() {
      window.NekoHost.postMessage({ cmd: 'get_host_info' });
    },
    getScreenInfo() {
      window.NekoHost.postMessage({ cmd: 'get_screen_info' });
    },
    getWindowState() {
      window.NekoHost.postMessage({ cmd: 'get_window_state' });
    },
    getDarkMode() {
      window.NekoHost.postMessage({ cmd: 'get_dark_mode' });
    },
    setDarkMode(enabled) {
      window.NekoHost.postMessage({ cmd: 'set_dark_mode', enabled: !!enabled });
    },
    openExternal(url) {
      window.NekoHost.postMessage({ cmd: 'open_external', url: String(url) });
    },
    getClipboardText() {
      return waitForEvent('clipboard_text', { cmd: 'get_clipboard_text' }).then((payload) => payload.text ?? '');
    },
    setClipboardText(text) {
      return waitForEvent('clipboard_text', { cmd: 'set_clipboard_text', text: String(text) }).then((payload) => payload.text ?? '');
    },
    moveWindowToDisplay(screenX, screenY) {
      return waitForEvent('window_state', {
        cmd: 'move_window_to_display',
        screen_x: Math.trunc(screenX),
        screen_y: Math.trunc(screenY),
      });
    },
    openFileDialog(options = {}) {
      return waitForEvent('file_dialog_result', {
        cmd: 'open_file_dialog',
        directory: !!options.directory,
        multiple: !!options.multiple,
        title: options.title == null ? null : String(options.title),
        filters: Array.isArray(options.filters) ? options.filters.map(normalizeFilter) : [],
      }).then((payload) => Array.isArray(payload.paths) ? payload.paths : []);
    },
    saveFileDialog(options = {}) {
      return waitForEvent('save_dialog_result', {
        cmd: 'save_file_dialog',
        title: options.title == null ? null : String(options.title),
        suggested_name: options.suggestedName == null ? null : String(options.suggestedName),
        directory: options.directory == null ? null : String(options.directory),
        filters: Array.isArray(options.filters) ? options.filters.map(normalizeFilter) : [],
      }).then((payload) => payload.path ?? null);
    }
  };
  window.nekoDarkMode = window.nekoDarkMode || {
    get() {
      return waitForEvent('dark_mode_changed', { cmd: 'get_dark_mode' }).then((payload) => !!payload.enabled);
    },
    set(enabled) {
      return waitForEvent('dark_mode_changed', { cmd: 'set_dark_mode', enabled: !!enabled }).then((payload) => !!payload.enabled);
    },
    toggle() {
      return this.get().then((current) => this.set(!current));
    }
  };
  window.electronShell = window.electronShell || {
    openExternal(url) {
      return waitForEvent('external_opened', { cmd: 'open_external', url: String(url) }).then(() => true);
    }
  };
  window.electronScreen = window.electronScreen || {
    async getAllDisplays() {
      return getMappedDisplays();
    },
    async getCurrentDisplay() {
      const payload = await waitForEvent('screen_info', { cmd: 'get_screen_info' });
      const displays = Array.isArray(payload.displays) ? payload.displays : [];
      const currentId = payload.current_display_id;
      const mapped = displays.map(mapDisplay);
      return mapped.find((display) => display.id === currentId) || mapped[0] || null;
    },
    async isPointOnScreen(x, y) {
      const displays = await getMappedDisplays();
      return displays.some((display) =>
        x >= display.x && x < display.x + display.width &&
        y >= display.y && y < display.y + display.height
      );
    },
    async clampToScreen(x, y, elementWidth = 0, elementHeight = 0) {
      const displays = await getMappedDisplays();
      const target = displays.find((display) =>
        x >= display.x && x < display.x + display.width &&
        y >= display.y && y < display.y + display.height
      ) || displays[0];
      if (!target) return { x, y, clamped: false };
      const clampedX = Math.max(target.x, Math.min(x, target.x + target.width - elementWidth));
      const clampedY = Math.max(target.y, Math.min(y, target.y + target.height - elementHeight));
      return { x: clampedX, y: clampedY, clamped: clampedX !== x || clampedY !== y };
    },
    async moveWindowToDisplay(screenX, screenY) {
      return window.NekoHost.moveWindowToDisplay(screenX, screenY);
    }
  };
  window.electronDesktopCapturer = window.electronDesktopCapturer || {
    async getSources(options = {}) {
      const types = Array.isArray(options.types) ? options.types : ['screen'];
      if (!types.includes('screen')) return [];
      const displays = await getMappedDisplays();
      return displays.map((display, index) => ({
        id: 'screen:' + display.id + ':0',
        name: display.name || ('Screen ' + (index + 1)),
        display_id: display.id,
        thumbnail: TRANSPARENT_PIXEL,
        appIcon: null,
      }));
    }
  };
  window.nekoClipboard = window.nekoClipboard || {
    readText() {
      return window.NekoHost.getClipboardText();
    },
    writeText(text) {
      return window.NekoHost.setClipboardText(text);
    }
  };
  window.nekoDialog = window.nekoDialog || {
    openFile(options) {
      return window.NekoHost.openFileDialog(options);
    },
    pickDirectory(options = {}) {
      return window.NekoHost.openFileDialog({ ...options, directory: true, multiple: false });
    },
    saveFile(options) {
      return window.NekoHost.saveFileDialog(options);
    }
  };
  if (typeof window.showOpenFilePicker !== 'function') {
    window.showOpenFilePicker = async function showOpenFilePicker(options = {}) {
      const paths = await window.NekoHost.openFileDialog({
        multiple: !!options.multiple,
        title: options.id || options.title,
        filters: normalizePickerTypes(options.types)
      });
      if (!paths.length) {
        throw new DOMException('The user aborted a request.', 'AbortError');
      }
      return paths.map(createFileHandle);
    };
  }
  if (typeof window.showSaveFilePicker !== 'function') {
    window.showSaveFilePicker = async function showSaveFilePicker(options = {}) {
      const path = await window.NekoHost.saveFileDialog({
        title: options.id || options.title,
        suggestedName: options.suggestedName,
        filters: normalizePickerTypes(options.types)
      });
      if (!path) {
        throw new DOMException('The user aborted a request.', 'AbortError');
      }
      return createFileHandle(path);
    };
  }
})();
"#
}

pub enum HostAction {
    Emit(HostEvent),
    ApplyInputRegion(InputRegion),
    Request(HostRequest),
}

#[derive(Debug, Clone)]
pub enum HostRequest {
    GetHostInfo,
    GetScreenInfo,
    GetWindowState,
    GetDarkMode,
    SetDarkMode { enabled: bool },
    OpenExternal { url: String },
    GetClipboardText,
    SetClipboardText { text: String },
    MoveWindowToDisplay { screen_x: i32, screen_y: i32 },
    OpenFileDialog {
        directory: bool,
        multiple: bool,
        title: Option<String>,
        filters: Vec<FileDialogFilterPayload>,
    },
    SaveFileDialog {
        title: Option<String>,
        suggested_name: Option<String>,
        directory: Option<String>,
        filters: Vec<FileDialogFilterPayload>,
    },
    ReadFile {
        path: String,
    },
    WriteFile {
        path: String,
        content_base64: String,
    },
}

pub fn handle_frontend_message(
    raw: &str,
) -> HostAction {
    match serde_json::from_str::<FrontendCommand>(raw) {
        Ok(FrontendCommand::Ping { nonce }) => HostAction::Emit(HostEvent::Pong { nonce }),
        Ok(FrontendCommand::GetHostInfo) => HostAction::Request(HostRequest::GetHostInfo),
        Ok(FrontendCommand::Log { message }) => {
            eprintln!("[frontend] {message}");
            HostAction::Emit(HostEvent::LogAck { message })
        }
        Ok(FrontendCommand::SetInputRegion { rects }) => HostAction::ApplyInputRegion(
            InputRegion::from_rects(rects.into_iter().map(InteractiveRect::from).collect()),
        ),
        Ok(FrontendCommand::GetScreenInfo) => HostAction::Request(HostRequest::GetScreenInfo),
        Ok(FrontendCommand::GetWindowState) => HostAction::Request(HostRequest::GetWindowState),
        Ok(FrontendCommand::GetDarkMode) => HostAction::Request(HostRequest::GetDarkMode),
        Ok(FrontendCommand::SetDarkMode { enabled }) => {
            HostAction::Request(HostRequest::SetDarkMode { enabled })
        }
        Ok(FrontendCommand::OpenExternal { url }) => {
            HostAction::Request(HostRequest::OpenExternal { url })
        }
        Ok(FrontendCommand::GetClipboardText) => {
            HostAction::Request(HostRequest::GetClipboardText)
        }
        Ok(FrontendCommand::SetClipboardText { text }) => {
            HostAction::Request(HostRequest::SetClipboardText { text })
        }
        Ok(FrontendCommand::MoveWindowToDisplay { screen_x, screen_y }) => {
            HostAction::Request(HostRequest::MoveWindowToDisplay { screen_x, screen_y })
        }
        Ok(FrontendCommand::OpenFileDialog {
            directory,
            multiple,
            title,
            filters,
        }) => HostAction::Request(HostRequest::OpenFileDialog {
            directory,
            multiple,
            title,
            filters,
        }),
        Ok(FrontendCommand::SaveFileDialog {
            title,
            suggested_name,
            directory,
            filters,
        }) => HostAction::Request(HostRequest::SaveFileDialog {
            title,
            suggested_name,
            directory,
            filters,
        }),
        Ok(FrontendCommand::ReadFile { path }) => {
            HostAction::Request(HostRequest::ReadFile { path })
        }
        Ok(FrontendCommand::WriteFile { path, content_base64 }) => {
            HostAction::Request(HostRequest::WriteFile { path, content_base64 })
        }
        Err(err) => HostAction::Emit(HostEvent::Error {
            message: format!("invalid frontend IPC payload: {err}"),
        }),
    }
}

pub fn build_emit_script(event: &HostEvent) -> Result<String> {
    let json = serde_json::to_string(event).map_err(|err| anyhow!(err))?;
    Ok(format!(
        "window.__NEKO_HOST_EMIT__ && window.__NEKO_HOST_EMIT__({json});"
    ))
}
