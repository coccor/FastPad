//! Render-target and brush plumbing shared by layout and painting.

use crate::Result;
use crate::preview::colors::{ColorRole, PreviewColors};
use crate::preview::dwrite::{Graphics, hresult_error};
use windows::Win32::Foundation::HWND as WinHwnd;
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D_SIZE_U, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    ID2D1HwndRenderTarget, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows_sys::Win32::Foundation::HWND;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl RectF {
    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub fn offset(&self, dx: f32, dy: f32) -> Self {
        Self::new(
            self.left + dx,
            self.top + dy,
            self.right + dx,
            self.bottom + dy,
        )
    }

    /// The overlap of two rectangles, or `None` when they do not overlap.
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let rect = Self::new(
            self.left.max(other.left),
            self.top.max(other.top),
            self.right.min(other.right),
            self.bottom.min(other.bottom),
        );
        (rect.left < rect.right && rect.top < rect.bottom).then_some(rect)
    }

    pub fn inflate(&self, by: f32) -> Self {
        Self::new(
            self.left - by,
            self.top - by,
            self.right + by,
            self.bottom + by,
        )
    }

    pub fn to_d2d(self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
        }
    }
}

pub fn color_f(colorref: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (colorref & 0xFF) as f32 / 255.0,
        g: ((colorref >> 8) & 0xFF) as f32 / 255.0,
        b: ((colorref >> 16) & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

pub struct Brushes {
    brushes: Vec<ID2D1SolidColorBrush>,
}

impl Brushes {
    pub fn create(target: &ID2D1RenderTarget, colors: &PreviewColors) -> Result<Self> {
        let brushes = ColorRole::ALL
            .iter()
            .map(|role| unsafe { target.CreateSolidColorBrush(&color_f(colors.get(*role)), None) })
            .collect::<windows::core::Result<Vec<_>>>()
            .map_err(hresult_error)?;
        Ok(Self { brushes })
    }

    pub fn get(&self, role: ColorRole) -> &ID2D1SolidColorBrush {
        &self.brushes[role as usize]
    }
}

pub fn create_hwnd_target(
    graphics: &Graphics,
    hwnd: HWND,
    width: u32,
    height: u32,
    dpi: u32,
) -> Result<ID2D1HwndRenderTarget> {
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        dpiX: dpi as f32,
        dpiY: dpi as f32,
        ..Default::default()
    };
    let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd: WinHwnd(hwnd),
        pixelSize: D2D_SIZE_U {
            width: width.max(1),
            height: height.max(1),
        },
        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
    };
    unsafe {
        graphics
            .d2d
            .CreateHwndRenderTarget(&properties, &hwnd_properties)
    }
    .map_err(hresult_error)
}

#[cfg(test)]
pub struct TestWindow(pub HWND);

#[cfg(test)]
impl TestWindow {
    pub fn new(width: i32, height: i32) -> Self {
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WS_POPUP};
        let class = crate::platform::wide_null("STATIC");
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null());
        Self(hwnd)
    }
}

#[cfg(test)]
impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
        }
    }
}

use crate::preview::images::ImageCache;
use crate::preview::layout::{DrawOp, ImageSlot};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ROUNDED_RECT,
};
use windows_numerics::Vector2;

/// Paints `ops` with their block origin at (`x`, `y`). Scrollable groups draw clipped and shifted
/// left by `h_offset`.
#[allow(clippy::too_many_arguments)]
pub fn draw_ops(
    target: &ID2D1RenderTarget,
    brushes: &Brushes,
    ops: &[DrawOp],
    slots: &[ImageSlot],
    cache: &mut ImageCache,
    x: f32,
    y: f32,
    h_offset: f32,
) {
    for op in ops {
        unsafe {
            match op {
                DrawOp::Text {
                    layout,
                    x: left,
                    y: top,
                    role,
                } => target.DrawTextLayout(
                    Vector2 {
                        X: x + left,
                        Y: y + top,
                    },
                    layout,
                    brushes.get(*role),
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                ),
                DrawOp::Fill { rect, role } => {
                    target.FillRectangle(&rect.offset(x, y).to_d2d(), brushes.get(*role))
                }
                DrawOp::RoundedFill { rect, radius, role } => target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: rect.offset(x, y).to_d2d(),
                        radiusX: *radius,
                        radiusY: *radius,
                    },
                    brushes.get(*role),
                ),
                DrawOp::Stroke { rect, role } => target.DrawRectangle(
                    &rect.offset(x, y).inflate(-0.5).to_d2d(),
                    brushes.get(*role),
                    1.0,
                    None,
                ),
                DrawOp::Checkbox { rect, checked } => {
                    let rect = rect.offset(x, y);
                    target.DrawRectangle(
                        &rect.inflate(-0.5).to_d2d(),
                        brushes.get(ColorRole::Border),
                        1.0,
                        None,
                    );
                    if *checked {
                        target.FillRectangle(
                            &rect.inflate(-3.0).to_d2d(),
                            brushes.get(ColorRole::Link),
                        );
                    }
                }
                DrawOp::Image { slot } => {
                    let Some(slot) = slots.get(*slot) else {
                        continue;
                    };
                    let rect = slot.rect.offset(x, y);
                    match slot
                        .path
                        .as_deref()
                        .and_then(|path| cache.bitmap(target, path))
                    {
                        Some(bitmap) => target.DrawBitmap(
                            &bitmap,
                            Some(&rect.to_d2d()),
                            1.0,
                            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                            None,
                        ),
                        None => {
                            target.DrawRectangle(
                                &rect.inflate(-0.5).to_d2d(),
                                brushes.get(ColorRole::Border),
                                1.0,
                                None,
                            );
                            target.DrawTextLayout(
                                Vector2 {
                                    X: rect.left + 8.0,
                                    Y: rect.top + 8.0,
                                },
                                &slot.alt,
                                brushes.get(ColorRole::Muted),
                                D2D1_DRAW_TEXT_OPTIONS_NONE,
                            );
                        }
                    }
                }
                DrawOp::Scrollable { clip, ops, .. } => {
                    target.PushAxisAlignedClip(
                        &clip.offset(x, y).to_d2d(),
                        D2D1_ANTIALIAS_MODE_ALIASED,
                    );
                    draw_ops(target, brushes, ops, slots, cache, x - h_offset, y, 0.0);
                    target.PopAxisAlignedClip();
                }
            }
        }
    }
}
