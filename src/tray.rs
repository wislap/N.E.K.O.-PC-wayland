use std::env;
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::{sync::mpsc, sync::Mutex};

use anyhow::{Result, anyhow};

use crate::desktop_config;
use crate::language;
use crate::wayland::raw_host::RawHostHandle;

pub struct SystemTrayHandle {
    #[cfg(target_os = "linux")]
    handle: Option<ksni::Handle<NekoTray>>,
    receiver: Mutex<Option<mpsc::Receiver<TrayCommand>>>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ReloadFrontend,
    Activate,
    ToggleVisibility,
    SetDarkMode(bool),
    SetStreamerMode(bool),
    SetCompatibilityMode(bool),
}

impl SystemTrayHandle {
    pub fn detached() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            handle: None,
            receiver: Mutex::new(None),
            thread: None,
        }
    }

    pub fn take_receiver(&self) -> Option<mpsc::Receiver<TrayCommand>> {
        self.receiver.lock().ok().and_then(|mut slot| slot.take())
    }

    pub fn attach_raw_host(&self, raw_host: RawHostHandle) {
        #[cfg(target_os = "linux")]
        if let Some(handle) = &self.handle {
            handle.update(|tray| {
                tray.raw_host = Some(raw_host);
            });
        }
    }

    pub fn join(mut self) {
        #[cfg(target_os = "linux")]
        if let Some(handle) = &self.handle {
            handle.shutdown();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(target_os = "linux")]
pub fn spawn_system_tray(
    stop_requested: Arc<AtomicBool>,
    initial_dark_mode: bool,
) -> Result<SystemTrayHandle> {
    let (sender, receiver) = mpsc::channel();
    let tray = NekoTray {
        stop_requested,
        command_sender: sender,
        dark_mode: initial_dark_mode,
        raw_host: None,
        streamer_mode: load_streamer_mode(),
        compatibility_mode: load_compatibility_mode(),
        icon_pixmap: load_tray_icon_pixmap(),
    };
    let service = ksni::TrayService::new(tray);
    let handle = service.handle();
    let thread = thread::Builder::new()
        .name("neko-system-tray".to_string())
        .spawn(move || service.run().unwrap())
        .map_err(|err| anyhow!("failed to spawn system tray thread: {err}"))?;
    Ok(SystemTrayHandle {
        handle: Some(handle),
        receiver: Mutex::new(Some(receiver)),
        thread: Some(thread),
    })
}

#[cfg(not(target_os = "linux"))]
pub fn spawn_system_tray(
    _stop_requested: Arc<AtomicBool>,
    _initial_dark_mode: bool,
) -> Result<SystemTrayHandle> {
    Ok(SystemTrayHandle::detached())
}

#[cfg(target_os = "linux")]
struct NekoTray {
    stop_requested: Arc<AtomicBool>,
    command_sender: mpsc::Sender<TrayCommand>,
    dark_mode: bool,
    raw_host: Option<RawHostHandle>,
    streamer_mode: bool,
    compatibility_mode: bool,
    icon_pixmap: Vec<ksni::Icon>,
}

#[cfg(target_os = "linux")]
impl NekoTray {
    fn texts(&self) -> TrayTexts {
        TrayTexts::for_locale(&language::default_i18n_locale())
    }
}

#[cfg(target_os = "linux")]
impl ksni::Tray for NekoTray {
    fn id(&self) -> String {
        "moe.neko.pc.wayland".to_string()
    }

    fn title(&self) -> String {
        "N.E.K.O.".to_string()
    }

    fn icon_name(&self) -> String {
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon_pixmap.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: String::new(),
            icon_pixmap: self.icon_pixmap.clone(),
            title: "N.E.K.O.".to_string(),
            description: self.texts().tooltip.to_string(),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.command_sender.send(TrayCommand::Activate);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;

        let texts = self.texts();
        let mut menu = Vec::new();
        let raw_window_visible = self
            .raw_host
            .as_ref()
            .map(|raw_host| raw_host.window_state().visible)
            .unwrap_or(true);

        if let Some(raw_host) = self.raw_host.clone() {
            let displays = raw_host.displays();
            let current_display_id = raw_host.current_output_id();
            if !displays.is_empty() {
                let submenu = displays
                    .into_iter()
                    .map(|display| {
                        let display_id = display.id.clone();
                        let label = match &display.name {
                            Some(name) if !name.trim().is_empty() => {
                                format!("{name} ({}x{})", display.width, display.height)
                            }
                            _ => format!("{} ({}x{})", display_id, display.width, display.height),
                        };
                        CheckmarkItem {
                            label,
                            checked: current_display_id.as_deref() == Some(display_id.as_str()),
                            activate: Box::new(move |this: &mut NekoTray| {
                                let Some(raw_host) = this.raw_host.clone() else {
                                    return;
                                };
                                if let Err(err) =
                                    raw_host.set_target_output_id(Some(display_id.clone()))
                                {
                                    eprintln!("failed to switch display from system tray: {err:#}");
                                } else if let Err(err) =
                                    desktop_config::persist_target_display(Some(display_id.as_str()))
                                {
                                    eprintln!(
                                        "failed to persist target display from system tray: {err:#}"
                                    );
                                }
                            }),
                            ..Default::default()
                        }
                        .into()
                    })
                    .collect();
                menu.push(
                    SubMenu {
                        label: texts.displays.to_string(),
                        submenu,
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }

        menu.extend([
            StandardItem {
                label: if raw_window_visible {
                    texts.hide.to_string()
                } else {
                    texts.show.to_string()
                },
                activate: Box::new(|this: &mut NekoTray| {
                    let _ = this.command_sender.send(TrayCommand::ToggleVisibility);
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: texts.dark_mode.to_string(),
                checked: self.dark_mode,
                activate: Box::new(|this: &mut NekoTray| {
                    this.dark_mode = !this.dark_mode;
                    if let Err(err) = desktop_config::persist_dark_mode(this.dark_mode) {
                        eprintln!("failed to persist dark mode from system tray: {err:#}");
                    }
                    let _ = this.command_sender.send(TrayCommand::SetDarkMode(this.dark_mode));
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: texts.streamer_mode.to_string(),
                checked: self.streamer_mode,
                activate: Box::new(|this: &mut NekoTray| {
                    this.streamer_mode = !this.streamer_mode;
                    if let Err(err) = desktop_config::persist_streamer_mode(this.streamer_mode) {
                        eprintln!("failed to persist streamer mode from system tray: {err:#}");
                    }
                    let _ = this
                        .command_sender
                        .send(TrayCommand::SetStreamerMode(this.streamer_mode));
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: texts.compatibility_mode.to_string(),
                checked: self.compatibility_mode,
                activate: Box::new(|this: &mut NekoTray| {
                    this.compatibility_mode = !this.compatibility_mode;
                    if let Err(err) =
                        desktop_config::persist_compatibility_mode(this.compatibility_mode)
                    {
                        eprintln!(
                            "failed to persist compatibility mode from system tray: {err:#}"
                        );
                    }
                    let _ = this
                        .command_sender
                        .send(TrayCommand::SetCompatibilityMode(this.compatibility_mode));
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: texts.reload.to_string(),
                activate: Box::new(|this: &mut NekoTray| {
                    let _ = this.command_sender.send(TrayCommand::ReloadFrontend);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: texts.feedback.to_string(),
                activate: Box::new(|_: &mut NekoTray| {
                    if let Err(err) = open_feedback_url() {
                        eprintln!("failed to open feedback url from system tray: {err:#}");
                    }
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: texts.exit.to_string(),
                activate: Box::new(|this: &mut NekoTray| {
                    this.stop_requested.store(true, Ordering::Relaxed);
                }),
                ..Default::default()
            }
            .into(),
        ]);

        menu
    }
}

#[cfg(target_os = "linux")]
fn open_feedback_url() -> Result<()> {
    Command::new("xdg-open")
        .arg("https://github.com/wislap/N.E.K.O.-PC-wayland/issues/new")
        .spawn()
        .map(|_| ())
        .map_err(|err| anyhow!("failed to launch xdg-open: {err}"))
}

#[cfg(target_os = "linux")]
fn load_streamer_mode() -> bool {
    desktop_config::load_or_migrate(&env::current_dir().unwrap_or_default())
        .ok()
        .and_then(|config| config.streamer_mode)
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn load_compatibility_mode() -> bool {
    desktop_config::load_or_migrate(&env::current_dir().unwrap_or_default())
        .ok()
        .and_then(|config| config.compatibility_mode)
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn load_tray_icon_pixmap() -> Vec<ksni::Icon> {
    let candidates = [
        env::current_dir().ok().map(|cwd| cwd.join("icon.png")),
        env::current_dir()
            .ok()
            .and_then(|cwd| cwd.parent().map(|parent| parent.join("N.E.K.O.-PC/icon.png"))),
        Some(std::path::PathBuf::from("/mnt/k_disk/programe/N.E.K.O.-PC/icon.png")),
    ];
    for path in candidates.into_iter().flatten() {
        if let Ok(reader) = image::ImageReader::open(&path) {
            if let Ok(image) = reader.decode() {
                let rgba = image.to_rgba8();
                let (width, height) = rgba.dimensions();
                let mut argb = Vec::with_capacity((width * height * 4) as usize);
                for pixel in rgba.pixels() {
                    let [r, g, b, a] = pixel.0;
                    argb.extend_from_slice(&[a, r, g, b]);
                }
                return vec![ksni::Icon {
                    width: width as i32,
                    height: height as i32,
                    data: argb,
                }];
            }
        }
    }
    Vec::new()
}

#[derive(Clone, Copy)]
struct TrayTexts {
    tooltip: &'static str,
    displays: &'static str,
    show: &'static str,
    hide: &'static str,
    dark_mode: &'static str,
    streamer_mode: &'static str,
    compatibility_mode: &'static str,
    reload: &'static str,
    feedback: &'static str,
    exit: &'static str,
}

impl TrayTexts {
    fn for_locale(locale: &str) -> Self {
        match locale {
            "zh-CN" => Self {
                tooltip: "系统托盘菜单",
                displays: "显示器",
                show: "显示窗口",
                hide: "隐藏窗口",
                dark_mode: "暗色模式",
                streamer_mode: "窗口模式",
                compatibility_mode: "兼容模式",
                reload: "重新加载",
                feedback: "反馈与建议",
                exit: "退出",
            },
            "zh-TW" => Self {
                tooltip: "系統托盤選單",
                displays: "顯示器",
                show: "顯示視窗",
                hide: "隱藏視窗",
                dark_mode: "深色模式",
                streamer_mode: "視窗模式",
                compatibility_mode: "相容模式",
                reload: "重新載入",
                feedback: "意見回饋",
                exit: "退出",
            },
            "ja" => Self {
                tooltip: "システムトレイメニュー",
                displays: "ディスプレイ",
                show: "ウィンドウを表示",
                hide: "ウィンドウを隠す",
                dark_mode: "ダークモード",
                streamer_mode: "ウィンドウモード",
                compatibility_mode: "互換モード",
                reload: "再読み込み",
                feedback: "フィードバック",
                exit: "終了",
            },
            "ko" => Self {
                tooltip: "시스템 트레이 메뉴",
                displays: "디스플레이",
                show: "창 표시",
                hide: "창 숨기기",
                dark_mode: "다크 모드",
                streamer_mode: "창 모드",
                compatibility_mode: "호환 모드",
                reload: "새로고침",
                feedback: "피드백",
                exit: "종료",
            },
            "ru" => Self {
                tooltip: "Меню в системном трее",
                displays: "Мониторы",
                show: "Показать окно",
                hide: "Скрыть окно",
                dark_mode: "Темная тема",
                streamer_mode: "Оконный режим",
                compatibility_mode: "Режим совместимости",
                reload: "Перезагрузить",
                feedback: "Обратная связь",
                exit: "Выход",
            },
            _ => Self {
                tooltip: "System tray menu",
                displays: "Displays",
                show: "Show Window",
                hide: "Hide Window",
                dark_mode: "Dark mode",
                streamer_mode: "Window mode",
                compatibility_mode: "Compatibility mode",
                reload: "Reload",
                feedback: "Feedback",
                exit: "Exit",
            },
        }
    }
}
