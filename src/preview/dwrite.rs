//! Direct2D and DirectWrite, loaded on the first preview. Both factory functions are resolved with
//! GetProcAddress so neither DLL enters FastPad.exe's import table; the `markdown_preview`
//! integration test enforces that. Never call the `windows` crate's `D2D1CreateFactory` or
//! `DWriteCreateFactory` wrappers: they add static imports.

use crate::platform::{OwnedModule, last_error, wide_null};
use crate::{FastPadError, Result};
use std::ffi::{CStr, c_void};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_OPTIONS, D2D1_FACTORY_TYPE, D2D1_FACTORY_TYPE_SINGLE_THREADED, ID2D1Factory,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE,
    DWRITE_FONT_WEIGHT, IDWriteFactory, IDWriteTextFormat,
};
use windows::core::{GUID, HRESULT, Interface, PCWSTR};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

type D2D1CreateFactoryFn = unsafe extern "system" fn(
    D2D1_FACTORY_TYPE,
    *const GUID,
    *const D2D1_FACTORY_OPTIONS,
    *mut *mut c_void,
) -> HRESULT;
type DWriteCreateFactoryFn =
    unsafe extern "system" fn(DWRITE_FACTORY_TYPE, *const GUID, *mut *mut c_void) -> HRESULT;

/// Field order matters: the factories drop before the modules that implement them.
pub struct Graphics {
    pub d2d: ID2D1Factory,
    pub dwrite: IDWriteFactory,
    _d2d_module: OwnedModule,
    _dwrite_module: OwnedModule,
}

impl std::fmt::Debug for Graphics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Graphics")
    }
}

pub fn hresult_error(error: windows::core::Error) -> FastPadError {
    FastPadError::Win32(error.code().0 as u32)
}

fn load_system_library(name: &str) -> Result<OwnedModule> {
    let wide = wide_null(name);
    let raw = unsafe {
        LoadLibraryExW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    unsafe { OwnedModule::from_raw_owned(raw) }
}

/// # Safety
/// `F` must be the exact `extern "system"` signature of the export `name`.
unsafe fn resolve<F: Copy>(module: &OwnedModule, name: &CStr) -> Result<F> {
    assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<usize>());
    let proc = unsafe { GetProcAddress(module.as_raw(), name.as_ptr().cast()) };
    let Some(proc) = proc else {
        return Err(last_error());
    };
    Ok(unsafe { std::mem::transmute_copy::<unsafe extern "system" fn() -> isize, F>(&proc) })
}

impl Graphics {
    pub fn load() -> Result<Self> {
        let d2d_module = load_system_library("d2d1.dll")?;
        let dwrite_module = load_system_library("dwrite.dll")?;
        let d2d = unsafe {
            let create: D2D1CreateFactoryFn = resolve(&d2d_module, c"D2D1CreateFactory")?;
            let mut raw = std::ptr::null_mut();
            create(
                D2D1_FACTORY_TYPE_SINGLE_THREADED,
                &ID2D1Factory::IID,
                std::ptr::null(),
                &mut raw,
            )
            .ok()
            .map_err(hresult_error)?;
            ID2D1Factory::from_raw(raw)
        };
        let dwrite = unsafe {
            let create: DWriteCreateFactoryFn = resolve(&dwrite_module, c"DWriteCreateFactory")?;
            let mut raw = std::ptr::null_mut();
            create(DWRITE_FACTORY_TYPE_SHARED, &IDWriteFactory::IID, &mut raw)
                .ok()
                .map_err(hresult_error)?;
            IDWriteFactory::from_raw(raw)
        };
        Ok(Self {
            d2d,
            dwrite,
            _d2d_module: d2d_module,
            _dwrite_module: dwrite_module,
        })
    }

    pub fn text_format(
        &self,
        family: &str,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        style: DWRITE_FONT_STYLE,
    ) -> Result<IDWriteTextFormat> {
        let family = wide_null(family);
        let locale = wide_null("");
        unsafe {
            self.dwrite.CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                weight,
                style,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                PCWSTR(locale.as_ptr()),
            )
        }
        .map_err(hresult_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_METRICS,
    };

    #[test]
    fn graphics_loads_both_factories_and_measures_text() {
        let graphics = Graphics::load().unwrap();
        let format = graphics
            .text_format(
                "Segoe UI",
                16.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            )
            .unwrap();
        let text = "Hello".encode_utf16().collect::<Vec<_>>();
        let layout = unsafe {
            graphics
                .dwrite
                .CreateTextLayout(&text, &format, 500.0, 100.0)
        }
        .unwrap();
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut metrics) }.unwrap();
        assert!(metrics.width > 0.0 && metrics.height > 0.0);
    }
}
