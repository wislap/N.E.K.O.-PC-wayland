use std::sync::OnceLock;

pub fn trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("NEKO_CEF_TRACE").ok().as_deref(),
            Some("1")
                | Some("true")
                | Some("TRUE")
                | Some("yes")
                | Some("YES")
                | Some("on")
                | Some("ON")
        )
    })
}
