#![allow(dead_code)]

use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone)]
pub enum CefLifecycleEvent {
    BrowserCreated,
    BrowserBeforeClose,
    LoadStart {
        transition_type: i32,
    },
    LoadEnd {
        http_status_code: i32,
    },
    LoadError {
        error_code: i32,
        error_text: String,
        failed_url: String,
    },
    LoadingStateChange {
        is_loading: bool,
        can_go_back: bool,
        can_go_forward: bool,
    },
    Console {
        level: i32,
        source: String,
        line: i32,
        message: String,
    },
}

type EventCallback = Arc<dyn Fn(CefLifecycleEvent) + Send + Sync + 'static>;

fn event_callback_cell() -> &'static Mutex<Option<EventCallback>> {
    static CELL: OnceLock<Mutex<Option<EventCallback>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

pub fn install_event_callback(callback: impl Fn(CefLifecycleEvent) + Send + Sync + 'static) {
    let mut slot = event_callback_cell()
        .lock()
        .expect("cef event callback mutex poisoned");
    *slot = Some(Arc::new(callback));
}

pub fn clear_event_callback() {
    let mut slot = event_callback_cell()
        .lock()
        .expect("cef event callback mutex poisoned");
    *slot = None;
}

pub fn emit(event: CefLifecycleEvent) {
    let callback = {
        let slot = event_callback_cell()
            .lock()
            .expect("cef event callback mutex poisoned");
        slot.clone()
    };

    if let Some(callback) = callback {
        callback(event);
    }
}
