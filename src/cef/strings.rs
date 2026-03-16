#![allow(dead_code)]

use anyhow::{Result, anyhow};

use crate::cef::ffi::cef_string_t;
#[cfg(has_cef_sdk)]
use crate::cef::ffi::{cef_string_utf8_to_utf16, cef_string_utf16_clear};

#[derive(Debug, Clone)]
pub struct CefString {
    utf16: Vec<u16>,
}

impl CefString {
    pub fn new(value: impl AsRef<str>) -> Self {
        Self {
            utf16: value.as_ref().encode_utf16().collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.utf16.is_empty()
    }

    pub fn to_cef(&mut self) -> cef_string_t {
        cef_string_t {
            str_: self.utf16.as_mut_ptr(),
            length: self.utf16.len(),
            dtor: None,
        }
    }
}

#[derive(Debug)]
pub struct CefOwnedString {
    inner: cef_string_t,
}

impl CefOwnedString {
    pub fn new(value: impl AsRef<str>) -> Self {
        #[cfg(has_cef_sdk)]
        {
            let mut inner = cef_string_t {
                str_: std::ptr::null_mut(),
                length: 0,
                dtor: None,
            };
            let value = value.as_ref();
            let bytes = value.as_bytes();
            unsafe {
                let _ = cef_string_utf8_to_utf16(
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    &mut inner,
                );
            }
            Self { inner }
        }

        #[cfg(not(has_cef_sdk))]
        {
            let mut backing: Vec<u16> = value.as_ref().encode_utf16().collect();
            let inner = cef_string_t {
                str_: backing.as_mut_ptr(),
                length: backing.len(),
                dtor: None,
            };
            std::mem::forget(backing);
            Self { inner }
        }
    }

    pub fn as_cef(&self) -> &cef_string_t {
        &self.inner
    }

    pub fn into_raw(mut self) -> cef_string_t {
        let inner = std::mem::replace(
            &mut self.inner,
            cef_string_t {
                str_: std::ptr::null_mut(),
                length: 0,
                dtor: None,
            },
        );
        std::mem::forget(self);
        inner
    }
}

impl Drop for CefOwnedString {
    fn drop(&mut self) {
        #[cfg(has_cef_sdk)]
        unsafe {
            cef_string_utf16_clear(&mut self.inner);
        }
    }
}

pub fn cef_string_from_option(value: Option<&str>) -> Result<CefOwnedString> {
    match value {
        Some(value) => Ok(CefOwnedString::new(value)),
        None => Ok(CefOwnedString::new("")),
    }
}

pub fn decode_cef_string(value: &cef_string_t) -> Result<String> {
    if value.str_.is_null() || value.length == 0 {
        return Ok(String::new());
    }

    let slice = unsafe { std::slice::from_raw_parts(value.str_, value.length) };
    String::from_utf16(slice).map_err(|err| anyhow!("failed to decode cef_string_t: {err}"))
}
