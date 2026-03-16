#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

#[cfg(has_cef_sdk)]
include!(concat!(env!("OUT_DIR"), "/cef_bindings.rs"));

#[cfg(not(has_cef_sdk))]
#[path = "ffi_fallback.rs"]
mod fallback;

#[cfg(not(has_cef_sdk))]
pub use fallback::*;
