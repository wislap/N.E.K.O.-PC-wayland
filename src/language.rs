use std::env;

const DEFAULT_I18N_LOCALE: &str = "en";
const DEFAULT_CEF_LOCALE: &str = "en-US";

pub fn default_i18n_locale() -> String {
    resolve_i18n_locale_from_env().unwrap_or_else(|| DEFAULT_I18N_LOCALE.to_string())
}

pub fn default_cef_locale() -> String {
    match default_i18n_locale().as_str() {
        "zh-CN" => "zh-CN".to_string(),
        "zh-TW" => "zh-TW".to_string(),
        "ja" => "ja".to_string(),
        "ko" => "ko".to_string(),
        "ru" => "ru".to_string(),
        "en" => DEFAULT_CEF_LOCALE.to_string(),
        _ => DEFAULT_CEF_LOCALE.to_string(),
    }
}

pub fn default_user_language() -> String {
    match default_i18n_locale().as_str() {
        "zh-CN" | "zh-TW" => "zh".to_string(),
        "ja" => "ja".to_string(),
        "ko" => "ko".to_string(),
        "ru" => "ru".to_string(),
        "en" => "en".to_string(),
        _ => "en".to_string(),
    }
}

fn resolve_i18n_locale_from_env() -> Option<String> {
    read_locale_candidates()
        .into_iter()
        .find_map(|candidate| normalize_i18n_locale(&candidate))
}

fn read_locale_candidates() -> Vec<String> {
    let mut candidates = Vec::new();

    if let Some(value) = read_nonempty_env("NEKO_WAYLAND_I18N_LOCALE") {
        candidates.push(value);
    }
    if let Some(value) = read_nonempty_env("LC_ALL") {
        candidates.push(value);
    }
    if let Some(value) = read_nonempty_env("LC_MESSAGES") {
        candidates.push(value);
    }
    if let Some(value) = read_nonempty_env("LANG") {
        candidates.push(value);
    }
    if let Some(value) = read_nonempty_env("LANGUAGE") {
        candidates.extend(
            value
                .split(':')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(ToOwned::to_owned),
        );
    }

    candidates
}

fn read_nonempty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_i18n_locale(raw: &str) -> Option<String> {
    let normalized = raw
        .split('.')
        .next()
        .unwrap_or(raw)
        .split('@')
        .next()
        .unwrap_or(raw)
        .trim()
        .replace('_', "-");
    if normalized.is_empty()
        || normalized.eq_ignore_ascii_case("c")
        || normalized.eq_ignore_ascii_case("posix")
    {
        return None;
    }

    let lower = normalized.to_ascii_lowercase();
    if lower.starts_with("zh") {
        if lower.contains("tw") || lower.contains("hk") || lower.contains("hant") {
            return Some("zh-TW".to_string());
        }
        return Some("zh-CN".to_string());
    }
    if lower.starts_with("ja") {
        return Some("ja".to_string());
    }
    if lower.starts_with("ko") {
        return Some("ko".to_string());
    }
    if lower.starts_with("ru") {
        return Some("ru".to_string());
    }
    if lower.starts_with("en") {
        return Some("en".to_string());
    }

    Some(DEFAULT_I18N_LOCALE.to_string())
}
