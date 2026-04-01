use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=NEKO_CEF_SDK_DIR");
    println!("cargo:rerun-if-changed=src/cef/capi_probe.c");
    println!("cargo:rustc-check-cfg=cfg(has_cef_sdk)");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_CEF_OSR");

    if env::var_os("CARGO_FEATURE_CEF_OSR").is_none() {
        return;
    }

    let Some(sdk_dir) = discover_cef_sdk_root() else {
        println!(
            "cargo:warning=CEF SDK was not discovered automatically; set NEKO_CEF_SDK_DIR to a directory containing include/ and Release/libcef.so"
        );
        return;
    };

    let include_dir = sdk_dir.join("include");
    let lib_dir = sdk_dir.join("Release");
    let libcef_so = lib_dir.join("libcef.so");

    if include_dir.exists() && libcef_so.exists() {
        println!("cargo:rustc-cfg=has_cef_sdk");
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
        println!("cargo:rustc-link-lib=dylib=cef");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
        println!("cargo:rustc-env=NEKO_CEF_SDK_DIR={}", sdk_dir.display());
        println!(
            "cargo:warning=building with CEF SDK from {}",
            sdk_dir.display()
        );

        generate_cef_bindings(&sdk_dir);
        build_cef_c_probe(&sdk_dir);
    }
}

fn generate_cef_bindings(sdk_dir: &PathBuf) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR should be set"));
    let bindings_path = out_dir.join("cef_bindings.rs");

    let builder = bindgen::Builder::default()
        .header(sdk_dir.join("include/capi/cef_app_capi.h").display().to_string())
        .header(
            sdk_dir
                .join("include/capi/cef_browser_capi.h")
                .display()
                .to_string(),
        )
        .clang_arg(format!("-I{}", sdk_dir.display()))
        .allowlist_type(
            "cef_(base_ref_counted|main_args|string|settings|app|browser_process_handler|command_line|display_handler|browser|browser_host|life_span_handler|load_handler|frame|render_handler|client|rect|screen_info|mouse_event|key_event|browser_settings|window_info|request_context)_t",
        )
        .allowlist_type("cef_(paint_element_type|mouse_button_type|key_event_type|log_severity|log_items)_t")
        .allowlist_function(
            "cef_(initialize|execute_process|shutdown|do_message_loop_work|api_hash|api_version|browser_host_create_browser|string_ascii_to_utf16|string_utf8_to_utf16|string_utf16_clear)",
        )
        .rustified_enum("cef_(paint_element_type|mouse_button_type|key_event_type)_t")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .layout_tests(false)
        .generate_comments(true);

    let bindings = builder
        .generate()
        .expect("failed to generate CEF bindings from local SDK");
    bindings
        .write_to_file(&bindings_path)
        .expect("failed to write generated CEF bindings");
}

fn build_cef_c_probe(sdk_dir: &PathBuf) {
    cc::Build::new()
        .file("src/cef/capi_probe.c")
        .include(sdk_dir)
        .include(sdk_dir.join("include"))
        .flag_if_supported("-std=c11")
        .compile("neko_cef_c_probe");
}

fn discover_cef_sdk_root() -> Option<PathBuf> {
    if let Some(value) = env::var_os("NEKO_CEF_SDK_DIR") {
        let candidate = PathBuf::from(value);
        if looks_like_cef_sdk_root(&candidate) {
            return Some(candidate);
        }
    }

    let mut roots = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        roots.push(cwd.clone());
        if let Some(parent) = cwd.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let manifest_dir = PathBuf::from(manifest_dir);
        roots.push(manifest_dir.clone());
        if let Some(parent) = manifest_dir.parent() {
            roots.push(parent.to_path_buf());
        }
    }

    for root in roots {
        if looks_like_cef_sdk_root(&root) {
            return Some(root);
        }
        if let Some(found) = scan_cef_sdk_children(&root) {
            return Some(found);
        }
    }

    None
}

fn scan_cef_sdk_children(root: &PathBuf) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.contains("cef") {
            continue;
        }
        if looks_like_cef_sdk_root(&path) {
            return Some(path);
        }
    }
    None
}

fn looks_like_cef_sdk_root(path: &PathBuf) -> bool {
    path.join("include").is_dir() && path.join("Release/libcef.so").is_file()
}
