//! A fixed-size DirectWrite inline object that reserves an image's space inside a text layout. The
//! preview draws the image itself (`DrawOp::Image`) where DirectWrite placed the object, so `Draw`
//! does nothing. Hand-written COM, like the MSAA providers, so no proc-macro dependency is needed.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{E_NOINTERFACE, E_POINTER, S_OK};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_BREAK_CONDITION, DWRITE_BREAK_CONDITION_NEUTRAL, DWRITE_INLINE_OBJECT_METRICS,
    DWRITE_OVERHANG_METRICS, IDWriteInlineObject,
};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface};

#[repr(C)]
struct InlineBox {
    vtable: &'static Vtable,
    references: AtomicU32,
    width: f32,
    height: f32,
}

/// `IDWriteInlineObject`'s vtable: `IUnknown`, then `Draw`, `GetMetrics`, `GetOverhangMetrics`,
/// and `GetBreakConditions`.
#[repr(C)]
struct Vtable {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    draw: unsafe extern "system" fn(
        *mut c_void,
        *const c_void,
        *mut c_void,
        f32,
        f32,
        BOOL,
        BOOL,
        *mut c_void,
    ) -> HRESULT,
    get_metrics:
        unsafe extern "system" fn(*mut c_void, *mut DWRITE_INLINE_OBJECT_METRICS) -> HRESULT,
    get_overhang_metrics:
        unsafe extern "system" fn(*mut c_void, *mut DWRITE_OVERHANG_METRICS) -> HRESULT,
    get_break_conditions: unsafe extern "system" fn(
        *mut c_void,
        *mut DWRITE_BREAK_CONDITION,
        *mut DWRITE_BREAK_CONDITION,
    ) -> HRESULT,
}

static VTABLE: Vtable = Vtable {
    query_interface,
    add_ref,
    release,
    draw,
    get_metrics,
    get_overhang_metrics,
    get_break_conditions,
};

#[cfg(test)]
thread_local! {
    static LIVE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// An inline object `width` by `height` DIPs whose bottom edge sits on the text baseline.
pub fn inline_box(width: f32, height: f32) -> IDWriteInlineObject {
    #[cfg(test)]
    LIVE.with(|live| live.set(live.get() + 1));
    let object = Box::into_raw(Box::new(InlineBox {
        vtable: &VTABLE,
        references: AtomicU32::new(1),
        width,
        height,
    }));
    // SAFETY: `InlineBox` starts with a pointer to a vtable laid out as `IDWriteInlineObject`'s, and
    // the reference it was created with moves into the returned interface.
    unsafe { IDWriteInlineObject::from_raw(object.cast()) }
}

unsafe fn object<'a>(this: *mut c_void) -> &'a InlineBox {
    unsafe { &*this.cast::<InlineBox>() }
}

unsafe extern "system" fn query_interface(
    this: *mut c_void,
    iid: *const GUID,
    output: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || output.is_null() {
        return E_POINTER;
    }
    let requested = unsafe { *iid };
    if requested == IUnknown::IID || requested == IDWriteInlineObject::IID {
        unsafe {
            add_ref(this);
            *output = this;
        }
        S_OK
    } else {
        unsafe { *output = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    unsafe { object(this) }
        .references
        .fetch_add(1, Ordering::Relaxed)
        + 1
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let remaining = unsafe { object(this) }
        .references
        .fetch_sub(1, Ordering::Release)
        - 1;
    if remaining == 0 {
        drop(unsafe { Box::from_raw(this.cast::<InlineBox>()) });
        #[cfg(test)]
        LIVE.with(|live| live.set(live.get() - 1));
    }
    remaining
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn draw(
    _this: *mut c_void,
    _context: *const c_void,
    _renderer: *mut c_void,
    _origin_x: f32,
    _origin_y: f32,
    _sideways: BOOL,
    _right_to_left: BOOL,
    _effect: *mut c_void,
) -> HRESULT {
    S_OK
}

unsafe extern "system" fn get_metrics(
    this: *mut c_void,
    metrics: *mut DWRITE_INLINE_OBJECT_METRICS,
) -> HRESULT {
    if metrics.is_null() {
        return E_POINTER;
    }
    let object = unsafe { object(this) };
    unsafe {
        *metrics = DWRITE_INLINE_OBJECT_METRICS {
            width: object.width,
            height: object.height,
            baseline: object.height,
            supportsSideways: false.into(),
        };
    }
    S_OK
}

unsafe extern "system" fn get_overhang_metrics(
    _this: *mut c_void,
    overhangs: *mut DWRITE_OVERHANG_METRICS,
) -> HRESULT {
    if overhangs.is_null() {
        return E_POINTER;
    }
    unsafe { *overhangs = DWRITE_OVERHANG_METRICS::default() };
    S_OK
}

unsafe extern "system" fn get_break_conditions(
    _this: *mut c_void,
    before: *mut DWRITE_BREAK_CONDITION,
    after: *mut DWRITE_BREAK_CONDITION,
) -> HRESULT {
    if before.is_null() || after.is_null() {
        return E_POINTER;
    }
    unsafe {
        *before = DWRITE_BREAK_CONDITION_NEUTRAL;
        *after = DWRITE_BREAK_CONDITION_NEUTRAL;
    }
    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::dwrite::Graphics;
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_HIT_TEST_METRICS,
        DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, IDWriteTextLayout,
    };

    fn layout(graphics: &Graphics, text: &str, width: f32) -> IDWriteTextLayout {
        let format = graphics
            .text_format(
                "Segoe UI",
                16.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            )
            .unwrap();
        let wide = text.encode_utf16().collect::<Vec<_>>();
        unsafe {
            graphics
                .dwrite
                .CreateTextLayout(&wide, &format, width, 1000.0)
        }
        .unwrap()
    }

    fn place(layout: &IDWriteTextLayout, position: u32, width: f32, height: f32) {
        let range = DWRITE_TEXT_RANGE {
            startPosition: position,
            length: 1,
        };
        unsafe { layout.SetInlineObject(&inline_box(width, height), range) }.unwrap();
    }

    fn text_metrics(layout: &IDWriteTextLayout) -> DWRITE_TEXT_METRICS {
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut metrics) }.unwrap();
        metrics
    }

    #[test]
    fn an_inline_box_reserves_its_size_on_the_line() {
        let graphics = Graphics::load().unwrap();
        let layout = layout(&graphics, "a\u{FFFC}b", 500.0);
        place(&layout, 1, 40.0, 30.0);
        assert!(text_metrics(&layout).height >= 30.0);
        let (mut x, mut y) = (0.0, 0.0);
        let mut hit = DWRITE_HIT_TEST_METRICS::default();
        unsafe { layout.HitTestTextPosition(1, false, &mut x, &mut y, &mut hit) }.unwrap();
        assert_eq!(hit.width, 40.0);
        assert!(hit.left > 0.0);
    }

    #[test]
    fn inline_boxes_wrap_like_words() {
        let graphics = Graphics::load().unwrap();
        let layout = layout(&graphics, "\u{FFFC} \u{FFFC} \u{FFFC}", 110.0);
        for position in [0, 2, 4] {
            place(&layout, position, 40.0, 20.0);
        }
        assert_eq!(text_metrics(&layout).lineCount, 2);
    }

    #[test]
    fn inline_boxes_are_freed_with_their_layouts() {
        let graphics = Graphics::load().unwrap();
        let before = LIVE.with(std::cell::Cell::get);
        for _ in 0..100 {
            let layout = layout(&graphics, "\u{FFFC}", 100.0);
            place(&layout, 0, 10.0, 10.0);
            text_metrics(&layout);
        }
        assert_eq!(LIVE.with(std::cell::Cell::get), before);
    }
}
