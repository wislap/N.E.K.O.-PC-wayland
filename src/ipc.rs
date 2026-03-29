use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wayland::detect::WaylandProfile;
use crate::wayland::strategy::StrategySelection;
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
    MoveWindowToDisplayId { display_id: String },
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
  function updateHostedWindowThemes() {
    const isDark = !!latestDarkMode;
    document.querySelectorAll('.neko-hosted-window').forEach((node) => {
      node.setAttribute('data-theme', isDark ? 'dark' : 'light');
    });
  }
  function emit(payload) {
    if (payload && payload.type === 'screen_info') latestScreenInfo = payload;
    if (payload && payload.type === 'window_state') latestWindowState = payload;
    if (payload && payload.type === 'dark_mode_changed') latestDarkMode = !!payload.enabled;
    if (payload && payload.type === 'dark_mode_changed') updateHostedWindowThemes();
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
      screenX: display.x,
      screenY: display.y,
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
    moveWindowToDisplayId(displayId) {
      return waitForEvent('window_state', {
        cmd: 'move_window_to_display_id',
        display_id: String(displayId),
      });
    },
    async moveWindowToDisplay(screenX, screenY) {
      const targetX = Math.trunc(screenX);
      const targetY = Math.trunc(screenY);
      const displaysBefore = await getMappedDisplays();
      const before = await window.electronScreen.getCurrentDisplay();
      const target = displaysBefore.find((display) =>
        targetX >= display.x && targetX < display.x + display.width &&
        targetY >= display.y && targetY < display.y + display.height
      ) || null;
      const state = target
        ? await window.NekoHost.moveWindowToDisplayId(target.id)
        : await waitForEvent('window_state', {
            cmd: 'move_window_to_display',
            screen_x: targetX,
            screen_y: targetY,
          });
      const after = await window.electronScreen.getCurrentDisplay();
      const sameDisplay = !!(before && after && before.id === after.id);
      const beforeScale = before && before.scaleFactor ? before.scaleFactor : 1;
      const afterScale = after && after.scaleFactor ? after.scaleFactor : 1;
      return {
        success: !!state,
        sameDisplay,
        state,
        displayId: after ? after.id : (target ? target.id : null),
        screenX: after ? after.x : (target ? target.x : targetX),
        screenY: after ? after.y : (target ? target.y : targetY),
        scaleRatio: beforeScale ? (afterScale / beforeScale) : 1,
      };
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
  const hostedWindows = new Map();
  let hostedWindowRoot = null;
  let hostedWindowStylesInstalled = false;
  let hostedWindowMessageBridgeInstalled = false;
  let hostedWindowZIndex = 2147482000;
  const hostedWindowStoragePrefix = 'neko_hosted_window_bounds:';
  const nativeWindowOpen = typeof window.open === 'function' ? window.open.bind(window) : null;
  function resolveHostedWindowUrl(url) {
    if (url == null || url === '') return null;
    try {
      return new URL(String(url), window.location.href);
    } catch (_) {
      return null;
    }
  }
  function shouldUseHostedWindow(url) {
    const resolved = resolveHostedWindowUrl(url);
    if (!resolved) return false;
    if (!/^https?:$/.test(resolved.protocol)) return false;
    return resolved.origin === window.location.origin;
  }
  function parseFeatureMap(features) {
    const result = Object.create(null);
    String(features || '')
      .split(',')
      .map((part) => part.trim())
      .filter(Boolean)
      .forEach((part) => {
        const [rawKey, rawValue] = part.split('=');
        const key = String(rawKey || '').trim().toLowerCase();
        const value = rawValue == null ? 'yes' : String(rawValue).trim();
        if (key) result[key] = value;
      });
    return result;
  }
  function parseFeatureNumber(featureMap, key, fallback) {
    const raw = featureMap[key];
    if (raw == null || raw === '') return fallback;
    const value = Number(raw);
    return Number.isFinite(value) ? value : fallback;
  }
  function installHostedWindowStyles() {
    if (hostedWindowStylesInstalled || document.getElementById('neko-hosted-window-styles')) return;
    const style = document.createElement('style');
    style.id = 'neko-hosted-window-styles';
    style.textContent = `
      #neko-hosted-window-root {
        position: fixed;
        inset: 0;
        pointer-events: none;
        z-index: 2147481999;
      }
      .neko-hosted-window {
        position: fixed;
        display: flex;
        flex-direction: column;
        min-width: 360px;
        min-height: 240px;
        max-width: calc(100vw - 24px);
        max-height: calc(100vh - 24px);
        background: rgba(255, 255, 255, 0.92);
        color: #1f2937;
        border: 1px solid rgba(148, 163, 184, 0.45);
        border-radius: 14px;
        box-shadow: 0 24px 80px rgba(15, 23, 42, 0.25);
        backdrop-filter: blur(18px) saturate(160%);
        overflow: hidden;
        pointer-events: auto;
      }
      .neko-hosted-window[data-theme="dark"] {
        background: rgba(15, 23, 42, 0.88);
        color: #e5e7eb;
        border-color: rgba(148, 163, 184, 0.25);
      }
      .neko-hosted-window-titlebar {
        display: flex;
        align-items: center;
        gap: 10px;
        min-height: 40px;
        padding: 8px 12px;
        cursor: move;
        user-select: none;
        background: rgba(255, 255, 255, 0.18);
        border-bottom: 1px solid rgba(148, 163, 184, 0.22);
      }
      .neko-hosted-window-title {
        flex: 1;
        min-width: 0;
        font-size: 13px;
        font-weight: 600;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      .neko-hosted-window-close {
        width: 28px;
        height: 28px;
        border: none;
        border-radius: 999px;
        background: rgba(239, 68, 68, 0.14);
        color: inherit;
        cursor: pointer;
        font-size: 16px;
        line-height: 1;
      }
      .neko-hosted-window-frame {
        flex: 1;
        width: 100%;
        border: none;
        background: transparent;
      }
      .neko-hosted-window-resize {
        position: absolute;
        right: 0;
        bottom: 0;
        width: 18px;
        height: 18px;
        cursor: nwse-resize;
        border-bottom-right-radius: 14px;
        background:
          linear-gradient(135deg, transparent 0 45%, rgba(148, 163, 184, 0.55) 45% 55%, transparent 55%),
          linear-gradient(135deg, transparent 0 62%, rgba(148, 163, 184, 0.45) 62% 72%, transparent 72%);
      }
    `;
    document.head.appendChild(style);
    hostedWindowStylesInstalled = true;
  }
  function hostedWindowStorageKey(name) {
    return hostedWindowStoragePrefix + String(name || '');
  }
  function loadHostedWindowBounds(name) {
    try {
      const raw = window.localStorage.getItem(hostedWindowStorageKey(name));
      if (!raw) return null;
      const parsed = JSON.parse(raw);
      if (!parsed || typeof parsed !== 'object') return null;
      if (![parsed.left, parsed.top, parsed.width, parsed.height].every(Number.isFinite)) return null;
      return clampHostedWindowBounds(parsed);
    } catch (_) {
      return null;
    }
  }
  function persistHostedWindowBounds(entry) {
    if (!entry || !entry.container) return;
    const rect = entry.container.getBoundingClientRect();
    try {
      window.localStorage.setItem(hostedWindowStorageKey(entry.name), JSON.stringify({
        left: Math.round(rect.left),
        top: Math.round(rect.top),
        width: Math.round(rect.width),
        height: Math.round(rect.height),
      }));
    } catch (_) {}
  }
  function ensureHostedWindowRoot() {
    if (hostedWindowRoot && hostedWindowRoot.isConnected) return hostedWindowRoot;
    if (!document.body) {
      throw new Error('document.body is unavailable for hosted window creation');
    }
    installHostedWindowStyles();
    const root = document.createElement('div');
    root.id = 'neko-hosted-window-root';
    root.setAttribute('aria-hidden', 'true');
    document.body.appendChild(root);
    hostedWindowRoot = root;
    return root;
  }
  function getHostedWindowProxy(name) {
    const entry = hostedWindows.get(String(name || ''));
    return entry ? entry.proxy : null;
  }
  window.__NEKO_HOSTED_WINDOW_LOOKUP__ = window.__NEKO_HOSTED_WINDOW_LOOKUP__ || getHostedWindowProxy;
  function bringHostedWindowToFront(entry) {
    if (!entry || !entry.container) return;
    hostedWindowZIndex += 1;
    entry.container.style.zIndex = String(hostedWindowZIndex);
    if (entry.frame && typeof entry.frame.focus === 'function') {
      try { entry.frame.focus(); } catch (_) {}
    }
  }
  function clampHostedWindowBounds(bounds) {
    const width = Math.max(360, Math.min(bounds.width, Math.max(360, window.innerWidth - 24)));
    const height = Math.max(240, Math.min(bounds.height, Math.max(240, window.innerHeight - 24)));
    const left = Math.max(12, Math.min(bounds.left, Math.max(12, window.innerWidth - width - 12)));
    const top = Math.max(12, Math.min(bounds.top, Math.max(12, window.innerHeight - height - 12)));
    return { width, height, left, top };
  }
  function reclampHostedWindowEntry(entry) {
    if (!entry || !entry.container) return;
    const rect = entry.container.getBoundingClientRect();
    const bounds = clampHostedWindowBounds({
      width: rect.width,
      height: rect.height,
      left: rect.left,
      top: rect.top,
    });
    entry.container.style.width = bounds.width + 'px';
    entry.container.style.height = bounds.height + 'px';
    entry.container.style.left = bounds.left + 'px';
    entry.container.style.top = bounds.top + 'px';
    persistHostedWindowBounds(entry);
  }
  function hostedWindowMetrics(entry) {
    if (!entry || !entry.container) {
      return { left: 0, top: 0, width: 0, height: 0 };
    }
    const rect = entry.container.getBoundingClientRect();
    return {
      left: Math.round(rect.left),
      top: Math.round(rect.top),
      width: Math.round(rect.width),
      height: Math.round(rect.height),
    };
  }
  function setHostedWindowBounds(entry, nextBounds) {
    if (!entry || !entry.container) return null;
    const current = hostedWindowMetrics(entry);
    const finiteOr = (value, fallback) => {
      const numeric = Number(value);
      return Number.isFinite(numeric) ? numeric : fallback;
    };
    const bounds = clampHostedWindowBounds({
      left: nextBounds.left == null ? current.left : finiteOr(nextBounds.left, current.left),
      top: nextBounds.top == null ? current.top : finiteOr(nextBounds.top, current.top),
      width: nextBounds.width == null ? current.width : finiteOr(nextBounds.width, current.width),
      height: nextBounds.height == null ? current.height : finiteOr(nextBounds.height, current.height),
    });
    entry.container.style.left = bounds.left + 'px';
    entry.container.style.top = bounds.top + 'px';
    entry.container.style.width = bounds.width + 'px';
    entry.container.style.height = bounds.height + 'px';
    persistHostedWindowBounds(entry);
    return bounds;
  }
  function computeHostedWindowBounds(features) {
    const featureMap = parseFeatureMap(features);
    const fallbackWidth = Math.min(Math.max(720, Math.round(window.innerWidth * 0.78)), Math.max(720, window.innerWidth - 24));
    const fallbackHeight = Math.min(Math.max(540, Math.round(window.innerHeight * 0.82)), Math.max(540, window.innerHeight - 24));
    const bounds = clampHostedWindowBounds({
      width: parseFeatureNumber(featureMap, 'width', fallbackWidth),
      height: parseFeatureNumber(featureMap, 'height', fallbackHeight),
      left: parseFeatureNumber(featureMap, 'left', Math.round((window.innerWidth - fallbackWidth) / 2)),
      top: parseFeatureNumber(featureMap, 'top', Math.round((window.innerHeight - fallbackHeight) / 2)),
    });
    return bounds;
  }
  function closeHostedWindow(name) {
    const entry = hostedWindows.get(name);
    if (!entry) return false;
    entry.closed = true;
    if (entry.container && entry.container.parentNode) {
      entry.container.parentNode.removeChild(entry.container);
    }
    hostedWindows.delete(name);
    if (window._openedWindows && window._openedWindows[name] === entry.proxy) {
      delete window._openedWindows[name];
    }
    return true;
  }
  function installHostedWindowMessageBridge() {
    if (hostedWindowMessageBridgeInstalled) return;
    window.addEventListener('message', (event) => {
      for (const entry of hostedWindows.values()) {
        if (!entry.frame || !entry.frame.contentWindow || event.source !== entry.frame.contentWindow) continue;
        bringHostedWindowToFront(entry);
        const data = event.data && typeof event.data === 'object' ? event.data : null;
        const type = data && data.type ? String(data.type) : '';
        if (type === 'close_page' || type === 'close_window' || type.startsWith('close_')) {
          closeHostedWindow(entry.name);
        }
        break;
      }
    });
    hostedWindowMessageBridgeInstalled = true;
  }
  function createHostedWindowProxy(entry) {
    return {
      focus() {
        bringHostedWindowToFront(entry);
      },
      blur() {},
      close() {
        closeHostedWindow(entry.name);
      },
      postMessage(message, targetOrigin = '*') {
        if (entry.frame && entry.frame.contentWindow) {
          entry.frame.contentWindow.postMessage(message, targetOrigin);
        }
      },
      moveTo(x, y) {
        setHostedWindowBounds(entry, {
          left: Number(x),
          top: Number(y),
        });
      },
      moveBy(x, y) {
        const current = hostedWindowMetrics(entry);
        setHostedWindowBounds(entry, {
          left: current.left + Number(x),
          top: current.top + Number(y),
        });
      },
      resizeTo(width, height) {
        setHostedWindowBounds(entry, {
          width: Number(width),
          height: Number(height),
        });
      },
      resizeBy(width, height) {
        const current = hostedWindowMetrics(entry);
        setHostedWindowBounds(entry, {
          width: current.width + Number(width),
          height: current.height + Number(height),
        });
      },
      get closed() {
        return !hostedWindows.has(entry.name) || !!entry.closed;
      },
      get name() {
        return entry.name;
      },
      get location() {
        return entry.frame ? entry.frame.contentWindow?.location || null : null;
      },
      get screenX() {
        return hostedWindowMetrics(entry).left;
      },
      get screenY() {
        return hostedWindowMetrics(entry).top;
      },
      get outerWidth() {
        return hostedWindowMetrics(entry).width;
      },
      get outerHeight() {
        return hostedWindowMetrics(entry).height;
      },
      get innerWidth() {
        return hostedWindowMetrics(entry).width;
      },
      get innerHeight() {
        return hostedWindowMetrics(entry).height;
      }
    };
  }
  function createHostedChildOpenerProxy(entry) {
    const targetWindow = window;
    const popupProxy = entry.proxy;
    const overrides = {
      closed: false,
      focus: () => popupProxy.focus(),
      blur: () => popupProxy.blur(),
      postMessage: (message, targetOrigin = '*') => targetWindow.postMessage(message, targetOrigin),
      open: (url, windowName, features) => window.nekoWindowing.openOrFocusWindow(url, windowName, features),
      openOrFocusWindow: (url, windowName, features) => window.nekoWindowing.openOrFocusWindow(url, windowName, features),
      closeNamedWindow: (windowName) => window.nekoWindowing.close(windowName),
      nekoWindowing: window.nekoWindowing,
    };
    return new Proxy(targetWindow, {
      get(target, prop, receiver) {
        if (prop in overrides) {
          const value = overrides[prop];
          return typeof value === 'function' ? value : value;
        }
        const value = Reflect.get(target, prop, receiver);
        return typeof value === 'function' ? value.bind(target) : value;
      },
      set(target, prop, value, receiver) {
        if (prop in overrides) return false;
        return Reflect.set(target, prop, value, receiver);
      },
      has(target, prop) {
        return prop in overrides || prop in target;
      }
    });
  }
  function patchHostedFrameWindow(entry) {
    if (!entry || !entry.frame || !entry.frame.contentWindow) return;
    const frameWindow = entry.frame.contentWindow;
    const openerProxy = entry.proxy;
    if (!entry.childOpenerProxy) {
      entry.childOpenerProxy = createHostedChildOpenerProxy(entry);
    }
    const childOpenerProxy = entry.childOpenerProxy;
    const defineGetter = (target, key, getter) => {
      try {
        const descriptor = Object.getOwnPropertyDescriptor(target, key);
        if (!descriptor || descriptor.configurable) {
          Object.defineProperty(target, key, {
            configurable: true,
            enumerable: false,
            get: getter,
          });
        }
      } catch (_) {}
    };
    try {
      frameWindow.__NEKO_HOSTED_WINDOW_NAME__ = entry.name;
      frameWindow.__NEKO_HOSTED_WINDOW_PROXY__ = openerProxy;
      frameWindow.__NEKO_HOSTED_WINDOW_PARENT__ = window;
    } catch (_) {}
    defineGetter(frameWindow, 'name', () => entry.name);
    defineGetter(frameWindow, 'opener', () => childOpenerProxy);
    defineGetter(frameWindow, 'closed', () => openerProxy.closed);
    defineGetter(frameWindow, 'screenX', () => openerProxy.screenX);
    defineGetter(frameWindow, 'screenY', () => openerProxy.screenY);
    defineGetter(frameWindow, 'outerWidth', () => openerProxy.outerWidth);
    defineGetter(frameWindow, 'outerHeight', () => openerProxy.outerHeight);
    defineGetter(frameWindow, 'innerWidth', () => openerProxy.innerWidth);
    defineGetter(frameWindow, 'innerHeight', () => openerProxy.innerHeight);
    try {
      frameWindow.close = () => openerProxy.close();
    } catch (_) {}
    try {
      frameWindow.focus = () => openerProxy.focus();
    } catch (_) {}
    try {
      frameWindow.blur = () => openerProxy.blur();
    } catch (_) {}
    try {
      frameWindow.moveTo = (x, y) => openerProxy.moveTo(x, y);
    } catch (_) {}
    try {
      frameWindow.moveBy = (x, y) => openerProxy.moveBy(x, y);
    } catch (_) {}
    try {
      frameWindow.resizeTo = (width, height) => openerProxy.resizeTo(width, height);
    } catch (_) {}
    try {
      frameWindow.resizeBy = (width, height) => openerProxy.resizeBy(width, height);
    } catch (_) {}
    try {
      frameWindow.open = (url, windowName, features) =>
        window.nekoWindowing.openOrFocusWindow(url, windowName, features);
    } catch (_) {}
    try {
      frameWindow.openOrFocusWindow = (url, windowName, features) =>
        window.nekoWindowing.openOrFocusWindow(url, windowName, features);
    } catch (_) {}
    try {
      frameWindow.closeNamedWindow = (windowName) => window.nekoWindowing.close(windowName);
    } catch (_) {}
    try {
      frameWindow.nekoWindowing = window.nekoWindowing;
    } catch (_) {}
    try {
      if (frameWindow.document && frameWindow.document.title) {
        entry.title.textContent = frameWindow.document.title;
      }
    } catch (_) {}
  }
  function installHostedWindowDrag(titlebar, entry) {
    titlebar.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return;
      bringHostedWindowToFront(entry);
      titlebar.setPointerCapture(event.pointerId);
      const rect = entry.container.getBoundingClientRect();
      const dragState = {
        startX: event.clientX,
        startY: event.clientY,
        left: rect.left,
        top: rect.top,
      };
      const onMove = (moveEvent) => {
        const bounds = clampHostedWindowBounds({
          width: rect.width,
          height: rect.height,
          left: dragState.left + (moveEvent.clientX - dragState.startX),
          top: dragState.top + (moveEvent.clientY - dragState.startY),
        });
        entry.container.style.left = bounds.left + 'px';
        entry.container.style.top = bounds.top + 'px';
      };
      const stop = () => {
        titlebar.removeEventListener('pointermove', onMove);
        titlebar.removeEventListener('pointerup', stop);
        titlebar.removeEventListener('pointercancel', stop);
        persistHostedWindowBounds(entry);
      };
      titlebar.addEventListener('pointermove', onMove);
      titlebar.addEventListener('pointerup', stop);
      titlebar.addEventListener('pointercancel', stop);
    });
  }
  function installHostedWindowResize(resizeHandle, entry) {
    resizeHandle.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      event.stopPropagation();
      bringHostedWindowToFront(entry);
      resizeHandle.setPointerCapture(event.pointerId);
      const rect = entry.container.getBoundingClientRect();
      const startX = event.clientX;
      const startY = event.clientY;
      const startWidth = rect.width;
      const startHeight = rect.height;
      const onMove = (moveEvent) => {
        setHostedWindowBounds(entry, {
          width: startWidth + (moveEvent.clientX - startX),
          height: startHeight + (moveEvent.clientY - startY),
        });
      };
      const stop = () => {
        resizeHandle.removeEventListener('pointermove', onMove);
        resizeHandle.removeEventListener('pointerup', stop);
        resizeHandle.removeEventListener('pointercancel', stop);
        persistHostedWindowBounds(entry);
      };
      resizeHandle.addEventListener('pointermove', onMove);
      resizeHandle.addEventListener('pointerup', stop);
      resizeHandle.addEventListener('pointercancel', stop);
    });
  }
  function openHostedWindow(url, windowName, features) {
    const resolved = resolveHostedWindowUrl(url);
    if (!resolved) return null;
    const name = String(windowName || '_blank' || '').trim() || ('neko_hosted_' + (hostedWindows.size + 1));
    const existing = hostedWindows.get(name);
    if (existing && !existing.closed) {
      if (existing.frame && existing.frame.src !== resolved.href) {
        existing.frame.src = resolved.href;
      }
      bringHostedWindowToFront(existing);
      return existing.proxy;
    }

    const root = ensureHostedWindowRoot();
    installHostedWindowMessageBridge();
    const bounds = loadHostedWindowBounds(name) || computeHostedWindowBounds(features);

    const container = document.createElement('section');
    container.className = 'neko-hosted-window';
    container.setAttribute('data-neko-interactive', 'window');
    container.setAttribute('data-neko-window-name', name);
    container.setAttribute('role', 'dialog');
    container.setAttribute('aria-modal', 'false');
    container.setAttribute('data-theme', latestDarkMode ? 'dark' : 'light');
    container.style.left = bounds.left + 'px';
    container.style.top = bounds.top + 'px';
    container.style.width = bounds.width + 'px';
    container.style.height = bounds.height + 'px';

    const titlebar = document.createElement('header');
    titlebar.className = 'neko-hosted-window-titlebar';
    titlebar.setAttribute('data-neko-interactive', 'titlebar');

    const title = document.createElement('div');
    title.className = 'neko-hosted-window-title';
    title.textContent = name === '_blank' ? resolved.pathname || resolved.href : name;

    const closeButton = document.createElement('button');
    closeButton.type = 'button';
    closeButton.className = 'neko-hosted-window-close';
    closeButton.textContent = '×';
    closeButton.setAttribute('aria-label', 'Close');
    closeButton.setAttribute('data-neko-interactive', 'close-button');

    const frame = document.createElement('iframe');
    frame.className = 'neko-hosted-window-frame';
    frame.setAttribute('data-neko-interactive', 'frame');
    frame.name = name;
    frame.src = resolved.href;
    frame.loading = 'eager';
    frame.referrerPolicy = 'strict-origin-when-cross-origin';

    const resizeHandle = document.createElement('div');
    resizeHandle.className = 'neko-hosted-window-resize';
    resizeHandle.setAttribute('data-neko-interactive', 'resize-handle');

    titlebar.appendChild(title);
    titlebar.appendChild(closeButton);
    container.appendChild(titlebar);
    container.appendChild(frame);
    container.appendChild(resizeHandle);
    root.appendChild(container);

    const entry = {
      name,
      container,
      frame,
      title,
      closed: false,
      proxy: null,
    };
    entry.proxy = createHostedWindowProxy(entry);
    hostedWindows.set(name, entry);
    window._openedWindows = window._openedWindows || {};
    window._openedWindows[name] = entry.proxy;

    installHostedWindowDrag(titlebar, entry);
    installHostedWindowResize(resizeHandle, entry);
    closeButton.addEventListener('click', () => closeHostedWindow(name));
    container.addEventListener('pointerdown', () => bringHostedWindowToFront(entry), { passive: true });
    frame.addEventListener('load', () => patchHostedFrameWindow(entry));
    persistHostedWindowBounds(entry);
    bringHostedWindowToFront(entry);
    return entry.proxy;
  }
  function openWindowThroughHost(url, windowName, features) {
    const resolved = resolveHostedWindowUrl(url);
    if (resolved && shouldUseHostedWindow(resolved.href)) {
      return openHostedWindow(resolved.href, windowName, features);
    }
    if (resolved && /^https?:$/.test(resolved.protocol) && resolved.origin !== window.location.origin) {
      try {
        window.NekoHost.openExternal(resolved.href);
      } catch (_) {}
      return null;
    }
    return nativeWindowOpen ? nativeWindowOpen(url, windowName, features) : null;
  }
  window.nekoWindowing = window.nekoWindowing || {
    isHostedUrl(url) {
      return shouldUseHostedWindow(url);
    },
    open(url, windowName, features) {
      return openWindowThroughHost(url, windowName, features);
    },
    openOrFocusWindow(url, windowName, features) {
      return openWindowThroughHost(url, windowName, features);
    },
    close(windowName) {
      return closeHostedWindow(String(windowName || ''));
    },
    focus(windowName) {
      const entry = hostedWindows.get(String(windowName || ''));
      if (!entry) return false;
      bringHostedWindowToFront(entry);
      return true;
    }
  };
  if (!window.__NEKO_HOST_WINDOW_OPEN_PATCHED__) {
    window.__NEKO_HOST_WINDOW_OPEN_PATCHED__ = true;
    window.open = function(url, windowName, features) {
      return openWindowThroughHost(url, windowName, features);
    };
  }
  if (!window.__NEKO_HOSTED_WINDOW_RESIZE_PATCHED__) {
    window.__NEKO_HOSTED_WINDOW_RESIZE_PATCHED__ = true;
    window.addEventListener('resize', () => {
      hostedWindows.forEach((entry) => reclampHostedWindowEntry(entry));
    }, { passive: true });
  }
  function isEligibleInteractiveNode(node) {
    if (!node || node.nodeType !== 1 || typeof node.matches !== 'function') return false;
    const style = window.getComputedStyle(node);
    if (!style || style.display === 'none' || style.visibility === 'hidden' || style.pointerEvents === 'none') {
      return false;
    }
    return true;
  }
  function interactiveClassHint(node) {
    const className = node.className ? String(node.className) : '';
    const id = node.id ? String(node.id) : '';
    return (className + ' ' + id).toLowerCase();
  }
  function shouldMarkInteractiveLeaf(node) {
    if (!isEligibleInteractiveNode(node)) return false;
    if (node.matches('button,input,textarea,select,summary,a[href],label,iframe,[tabindex],[contenteditable=\"true\"],[aria-haspopup=\"true\"],[data-action],[onclick]')) {
      return true;
    }
    const role = (node.getAttribute('role') || '').toLowerCase();
    if (['button', 'menuitem', 'switch', 'checkbox', 'radio', 'slider', 'tab', 'option', 'textbox'].includes(role)) {
      return true;
    }
    const style = window.getComputedStyle(node);
    return style.cursor === 'pointer';
  }
  function shouldMarkInteractiveSurface(node) {
    if (!isEligibleInteractiveNode(node)) return false;
    if (node.matches('[data-neko-sidepanel],[data-neko-sidepanel-owner],dialog,[role=\"dialog\"],[role=\"menu\"],[role=\"tooltip\"]')) {
      return true;
    }
    const hints = interactiveClassHint(node);
    if (/(popup|popover|dialog|modal|menu|panel|tooltip|tutorial|drawer|dropdown|overlay)/.test(hints)) {
      return true;
    }
    const style = window.getComputedStyle(node);
    const position = style.position;
    const zIndex = Number.parseInt(style.zIndex || '0', 10);
    return (position === 'fixed' || position === 'absolute' || position === 'sticky') && Number.isFinite(zIndex) && zIndex >= 1000;
  }
  function markInteractiveNode(node) {
    if (!node || node.nodeType !== 1 || node.hasAttribute('data-neko-interactive')) return;
    if (shouldMarkInteractiveSurface(node)) {
      node.setAttribute('data-neko-interactive', 'surface');
      return;
    }
    if (shouldMarkInteractiveLeaf(node)) {
      node.setAttribute('data-neko-interactive', 'auto');
    }
  }
  function scanInteractiveSubtree(root) {
    if (!root) return;
    if (root.nodeType === 1) {
      markInteractiveNode(root);
    }
    if (typeof root.querySelectorAll !== 'function') return;
    const nodes = root.querySelectorAll('*');
    for (const node of nodes) {
      markInteractiveNode(node);
    }
  }
  if (!window.__NEKO_INTERACTIVE_MARKER_INSTALLED__) {
    window.__NEKO_INTERACTIVE_MARKER_INSTALLED__ = true;
    const startInteractiveMarker = () => {
      if (!document.body) return;
      scanInteractiveSubtree(document.body);
      const observer = new MutationObserver((mutations) => {
        for (const mutation of mutations) {
          if (mutation.type === 'childList') {
            mutation.addedNodes.forEach((node) => scanInteractiveSubtree(node));
          } else if (mutation.type === 'attributes') {
            markInteractiveNode(mutation.target);
          }
        }
      });
      observer.observe(document.documentElement || document.body, {
        subtree: true,
        childList: true,
        attributes: true,
        attributeFilter: ['class', 'style', 'role', 'tabindex', 'aria-haspopup', 'data-neko-sidepanel', 'data-neko-sidepanel-owner'],
      });
    };
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', startInteractiveMarker, { once: true });
    } else {
      startInteractiveMarker();
    }
  }
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
    },
    async moveWindowToDisplayId(displayId) {
      const before = await this.getCurrentDisplay();
      const state = await window.NekoHost.moveWindowToDisplayId(displayId);
      const after = await this.getCurrentDisplay();
      const sameDisplay = !!(before && after && before.id === after.id);
      const beforeScale = before && before.scaleFactor ? before.scaleFactor : 1;
      const afterScale = after && after.scaleFactor ? after.scaleFactor : 1;
      return {
        success: !!state,
        sameDisplay,
        state,
        displayId: after ? after.id : String(displayId),
        screenX: after ? after.x : null,
        screenY: after ? after.y : null,
        scaleRatio: beforeScale ? (afterScale / beforeScale) : 1,
      };
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
    MoveWindowToDisplayId { display_id: String },
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
        Ok(FrontendCommand::MoveWindowToDisplayId { display_id }) => {
            HostAction::Request(HostRequest::MoveWindowToDisplayId { display_id })
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
