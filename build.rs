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

    let Ok(sdk_dir) = env::var("NEKO_CEF_SDK_DIR") else {
        return;
    };

    let sdk_dir = PathBuf::from(sdk_dir);
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
