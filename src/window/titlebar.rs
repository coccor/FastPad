use crate::window::palette::Palette;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DwmDefWindowProc, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, DC_BRUSH, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CALCRECT,
    DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
    DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW, EndPaint, FW_NORMAL, FillRect,
    GetMonitorInfoW, GetStockObject, HDC, HFONT, IntersectClipRect, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, MonitorFromWindow, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, RestoreDC, SRCCOPY, SaveDC, ScreenToClient, SelectObject, SetBkMode,
    SetDCBrushColor, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::SetWindowTheme;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP,
    HTTOPLEFT, HTTOPRIGHT, IsZoomed, MINMAXINFO, NCCALCSIZE_PARAMS, SM_CXPADDEDBORDER, SM_CYFRAME,
    SM_CYSIZE, WM_NCCALCSIZE, WM_NCHITTEST,
};

const GLYPH_MINIMIZE: &str = "\u{E921}";
const GLYPH_MAXIMIZE: &str = "\u{E922}";
const GLYPH_RESTORE: &str = "\u{E923}";
const GLYPH_CLOSE: &str = "\u{E8BB}";
const GLYPH_MORE: &str = "\u{E712}";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

impl Size {
    pub const fn new(width: i32, height: i32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub const fn left(self) -> i32 {
        self.left
    }

    pub const fn right(self) -> i32 {
        self.right
    }

    pub const fn center(self) -> Point {
        Point::new((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }

    pub const fn contains(self, point: Point) -> bool {
        point.x >= self.left && point.x < self.right && point.y >= self.top && point.y < self.bottom
    }

    fn centered_square(self, size: i32) -> Self {
        let center = self.center();
        let half = size.min(self.right - self.left).min(self.bottom - self.top) / 2;
        Self::new(
            center.x - half,
            center.y - half,
            center.x + half,
            center.y + half,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HitTarget {
    Client,
    Caption,
    Minimize,
    Maximize,
    Close,
    Tab(usize),
    CloseTab(usize),
    /// The band along the bottom of the tab viewport while the tabs overflow it.
    ScrollBar,
    Overflow,
    ResizeTop,
    ResizeTopLeft,
    ResizeTopRight,
}

impl HitTarget {
    /// Caption buttons report non-client hit codes, so their pointer messages are non-client.
    pub const fn is_caption_button(self) -> bool {
        matches!(self, Self::Minimize | Self::Maximize | Self::Close)
    }

    pub const fn is_interactive(self) -> bool {
        matches!(
            self,
            Self::Minimize
                | Self::Maximize
                | Self::Close
                | Self::Tab(_)
                | Self::CloseTab(_)
                | Self::ScrollBar
                | Self::Overflow
        )
    }

    pub fn from_nonclient_code(code: usize) -> Option<Self> {
        match u32::try_from(code).ok()? {
            HTMINBUTTON => Some(Self::Minimize),
            HTMAXBUTTON => Some(Self::Maximize),
            HTCLOSE => Some(Self::Close),
            _ => None,
        }
    }
}

/// Which title-strip target the pointer is over and which one a primary button went down on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PointerState {
    pub hovered: Option<HitTarget>,
    pub pressed: Option<HitTarget>,
}

impl PointerState {
    #[must_use]
    pub fn hover(self, target: Option<HitTarget>) -> Self {
        Self {
            hovered: target.filter(|target| target.is_interactive()),
            ..self
        }
    }

    #[must_use]
    pub fn press(self, target: Option<HitTarget>) -> Self {
        Self {
            pressed: target.filter(|target| target.is_interactive()),
            ..self
        }
    }

    pub fn is_pressed(self, target: HitTarget) -> bool {
        self.pressed == Some(target)
    }

    /// Clears the press and returns the target to activate when released over the pressed target.
    #[must_use]
    pub fn release(self, target: Option<HitTarget>) -> (Self, Option<HitTarget>) {
        let activated = self.pressed.filter(|pressed| target == Some(*pressed));
        (
            Self {
                pressed: None,
                ..self
            },
            activated,
        )
    }

    /// Client and non-client leave notifications arrive independently; each clears only its own
    /// area's hover so an out-of-order leave cannot drop the other area's highlight.
    #[must_use]
    pub fn leave(self, nonclient: bool) -> Self {
        if self
            .hovered
            .is_some_and(|target| target.is_caption_button() == nonclient)
        {
            Self::default()
        } else {
            self
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TitleBarLayout {
    /// The visible tab viewport; tabs scrolled outside it are clipped and never hit.
    pub tabs: Rect,
    /// Empty strip beside the tabs: drags the window, opens a tab on double-click.
    pub drag_region: Rect,
    pub minimize: Rect,
    pub maximize: Rect,
    pub close: Rect,
    pub overflow: Rect,
    pub height: i32,
    /// Height of the top band that resizes a restored window.
    pub resize_border: i32,
    /// How far the tabs are scrolled left, clamped to `max_scroll`.
    pub scroll: i32,
    pub max_scroll: i32,
    /// The draggable scroll band, present only while the tabs overflow the viewport.
    pub scroll_bar: Option<Rect>,
    min_thumb: i32,
    tab_width: i32,
    tab_rects: Vec<Rect>,
    close_tab_rects: Vec<Rect>,
}

const fn scale(value: i32, dpi: u32) -> i32 {
    let dpi = if dpi == 0 { 1 } else { dpi };
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}

impl TitleBarLayout {
    pub fn calculate(client: Size, dpi: u32, tab_count: usize) -> Self {
        Self::calculate_scrolled(client, dpi, tab_count, 0)
    }

    pub fn calculate_scrolled(client: Size, dpi: u32, tab_count: usize, scroll: i32) -> Self {
        let width = client.width.max(0);
        let dpi = dpi.max(1);
        let height = scale(40, dpi).max(unsafe { GetSystemMetricsForDpi(SM_CYSIZE, dpi) });
        let resize_border = unsafe {
            GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
        }
        .clamp(1, (height / 2).max(1));
        let caption_width = scale(46, dpi).max(1).min(width / 3);
        let close = Rect::new(width - caption_width, 0, width, height);
        let maximize = Rect::new(close.left - caption_width, 0, close.left, height);
        let minimize = Rect::new(maximize.left - caption_width, 0, maximize.left, height);

        let actions_right = minimize.left.max(0);
        let overflow_left = (actions_right - scale(40, dpi)).max(0);
        let overflow = Rect::new(overflow_left, 0, actions_right, height);

        // Some empty strip always stays reachable, however many tabs are open.
        let tabs_right = overflow_left - scale(48, dpi).min(overflow_left);
        let tabs = Rect::new(0, 0, tabs_right, height);
        let preferred_tab_width = scale(200, dpi);
        let tab_width = if tab_count == 0 {
            0
        } else {
            (tabs_right / tab_count as i32).clamp(scale(120, dpi), preferred_tab_width)
        };
        let content_width = tab_width.saturating_mul(tab_count as i32);
        let max_scroll = (content_width - tabs_right).max(0);
        let scroll = scroll.clamp(0, max_scroll);

        let close_size = scale(32, dpi).min(tab_width);
        let mut tab_rects = Vec::with_capacity(tab_count);
        let mut close_tab_rects = Vec::with_capacity(tab_count);
        for index in 0..tab_count {
            let left = index as i32 * tab_width - scroll;
            let right = left + tab_width;
            tab_rects.push(Rect::new(left, 0, right, height));
            close_tab_rects.push(Rect::new(right - close_size, 0, right, height));
        }

        let drag_region = Rect::new(
            (content_width - scroll).clamp(0, tabs_right),
            0,
            overflow_left,
            height,
        );
        let scroll_bar = (max_scroll > 0)
            .then(|| Rect::new(tabs.left, height - scale(8, dpi), tabs.right, height));

        Self {
            tabs,
            drag_region,
            minimize,
            maximize,
            close,
            overflow,
            height,
            resize_border,
            scroll,
            max_scroll,
            scroll_bar,
            min_thumb: scale(24, dpi),
            tab_width,
            tab_rects,
            close_tab_rects,
        }
    }

    /// The scroll offset that brings the whole of tab `index` into the viewport.
    pub fn scroll_to_reveal(&self, index: usize) -> i32 {
        let left = index as i32 * self.tab_width;
        let right = left + self.tab_width;
        let viewport = self.tabs.right - self.tabs.left;
        let scroll = if left < self.scroll {
            left
        } else if right > self.scroll + viewport {
            right - viewport
        } else {
            self.scroll
        };
        scroll.clamp(0, self.max_scroll)
    }

    /// The scroll offset after a mouse wheel turn of `delta`; one notch moves half a tab.
    pub fn scroll_by_wheel(&self, delta: i32, wheel_delta: i32) -> i32 {
        let step = (self.tab_width / 2).max(1);
        let pixels = (i64::from(delta) * i64::from(step) / i64::from(wheel_delta.max(1))) as i32;
        self.scroll.saturating_add(pixels).clamp(0, self.max_scroll)
    }

    /// The thumb inside `scroll_bar`, sized by how much of the tab strip is visible.
    pub fn scroll_thumb(&self) -> Option<Rect> {
        let bar = self.scroll_bar?;
        let track = bar.right - bar.left;
        let thumb = self.thumb_width(track);
        let left = bar.left + self.scroll * (track - thumb) / self.max_scroll;
        Some(Rect::new(left, bar.top, left + thumb, bar.bottom))
    }

    fn thumb_width(&self, track: i32) -> i32 {
        let content = track + self.max_scroll;
        (track * track / content.max(1))
            .max(self.min_thumb)
            .min(track)
    }

    /// The scroll offset that puts the thumb's left edge at `thumb_left`.
    pub fn scroll_for_thumb(&self, thumb_left: i32) -> i32 {
        let Some(bar) = self.scroll_bar else {
            return 0;
        };
        let track = bar.right - bar.left;
        let travel = (track - self.thumb_width(track)).max(1);
        let offset = i64::from((thumb_left - bar.left).clamp(0, travel));
        (offset * i64::from(self.max_scroll) / i64::from(travel)) as i32
    }

    pub fn hit_test(&self, point: Point) -> HitTarget {
        if self.close.contains(point) {
            return HitTarget::Close;
        }
        if self.maximize.contains(point) {
            return HitTarget::Maximize;
        }
        if self.minimize.contains(point) {
            return HitTarget::Minimize;
        }
        if self.overflow.contains(point) {
            return HitTarget::Overflow;
        }
        if self.scroll_bar.is_some_and(|bar| bar.contains(point)) {
            return HitTarget::ScrollBar;
        }
        if self.tabs.contains(point) {
            for (index, rect) in self.close_tab_rects.iter().enumerate() {
                if rect.contains(point) {
                    return HitTarget::CloseTab(index);
                }
            }
            for (index, rect) in self.tab_rects.iter().enumerate() {
                if rect.contains(point) {
                    return HitTarget::Tab(index);
                }
            }
        }
        if self.drag_region.contains(point) {
            return HitTarget::Caption;
        }
        HitTarget::Client
    }

    /// Like `hit_test`, but a restored window's top band resizes (the frame no longer reserves a
    /// native top border). Caption buttons always keep their targets; the right frame border outside
    /// the client area still reports the top-right corner through DefWindowProc.
    pub fn frame_hit_test(&self, point: Point, maximized: bool) -> HitTarget {
        let target = self.hit_test(point);
        if maximized || point.y < 0 || point.y >= self.resize_border {
            return target;
        }
        if target.is_caption_button() {
            target
        } else if point.x < self.resize_border {
            HitTarget::ResizeTopLeft
        } else if point.x >= self.close.right - self.resize_border {
            HitTarget::ResizeTopRight
        } else {
            HitTarget::ResizeTop
        }
    }

    pub fn tab(&self, index: usize) -> Rect {
        self.tab_rects[index]
    }

    pub fn close_tab(&self, index: usize) -> Rect {
        self.close_tab_rects[index]
    }

    fn strip(&self) -> Rect {
        Rect::new(0, 0, self.close.right, self.height)
    }
}

/// `work_area` is `Some` only for a maximized window: its client fills the monitor work area so no
/// frame or caption button lands off-screen. A restored window keeps the proposed top edge and the
/// default left/right/bottom resize borders.
pub fn frame_client_rect(proposed: Rect, default_client: Rect, work_area: Option<Rect>) -> Rect {
    match work_area {
        Some(work) => Rect::new(
            proposed.left.max(work.left),
            proposed.top.max(work.top),
            proposed.right.min(work.right),
            proposed.bottom.min(work.bottom),
        ),
        None => Rect {
            top: proposed.top,
            ..default_client
        },
    }
}

pub(crate) fn layout_for_window(hwnd: HWND, tab_count: usize, scroll: i32) -> TitleBarLayout {
    let mut client = RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut client);
    }
    TitleBarLayout::calculate_scrolled(
        Size::new(client.right - client.left, client.bottom - client.top),
        unsafe { GetDpiForWindow(hwnd) }.max(96),
        tab_count,
        scroll,
    )
}

pub(crate) fn invalidate_strip(hwnd: HWND) {
    let strip = native_rect(layout_for_window(hwnd, 0, 0).strip());
    unsafe {
        InvalidateRect(hwnd, &strip, 0);
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TitleFontHandles {
    text: HFONT,
    glyph: HFONT,
}

impl Default for TitleFontHandles {
    fn default() -> Self {
        Self {
            text: std::ptr::null_mut(),
            glyph: std::ptr::null_mut(),
        }
    }
}

/// Title-strip fonts for one DPI, deleted on drop (with the App at `WM_NCDESTROY`).
#[derive(Debug)]
pub(crate) struct TitleFonts {
    dpi: u32,
    handles: TitleFontHandles,
}

impl TitleFonts {
    pub(crate) fn create(dpi: u32) -> Self {
        Self {
            dpi,
            handles: TitleFontHandles {
                text: create_font(scale(12, dpi), "Segoe UI"),
                glyph: create_font(scale(10, dpi), "Segoe MDL2 Assets"),
            },
        }
    }

    pub(crate) fn dpi(&self) -> u32 {
        self.dpi
    }

    pub(crate) fn handles(&self) -> TitleFontHandles {
        self.handles
    }
}

impl TitleFontHandles {
    /// The UI text font, null before chrome fonts exist.
    pub(crate) fn text(&self) -> HFONT {
        self.text
    }
}

impl Drop for TitleFonts {
    fn drop(&mut self) {
        for font in [self.handles.text, self.handles.glyph] {
            if !font.is_null() {
                unsafe {
                    DeleteObject(font);
                }
            }
        }
    }
}

fn create_font(pixel_height: i32, face: &str) -> HFONT {
    let face = crate::platform::wide_null(face);
    unsafe {
        CreateFontW(
            -pixel_height,
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            u32::from(DEFAULT_CHARSET),
            u32::from(OUT_DEFAULT_PRECIS),
            u32::from(CLIP_DEFAULT_PRECIS),
            u32::from(CLEARTYPE_QUALITY),
            u32::from(DEFAULT_PITCH),
            face.as_ptr(),
        )
    }
}

pub(crate) struct TitlePaint<'a> {
    pub titles: &'a [&'a str],
    pub active: usize,
    pub scroll: i32,
    /// Shown in place of the hidden editor while no tab is open.
    pub empty_hint: Option<&'a str>,
    /// Present once deferred chrome is built; the bar is painted along the bottom edge.
    pub status: Option<&'a crate::window::status::StatusBarText>,
    pub palette: Palette,
    pub fonts: TitleFontHandles,
    pub pointer: PointerState,
}

pub(crate) unsafe fn paint(hwnd: HWND, input: &TitlePaint<'_>) {
    let mut paint = PAINTSTRUCT::default();
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    if dc.is_null() {
        return;
    }

    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let layout = layout_for_window(hwnd, input.titles.len(), input.scroll);
    let maximized = unsafe { IsZoomed(hwnd) } != 0;
    if paint.rcPaint.top < layout.height {
        unsafe { paint_strip_buffered(dc, &layout, dpi, maximized, input) };
    }

    let mut client = RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut client);
    }
    let status_height = if input.status.is_some() {
        crate::window::status::status_height(dpi)
    } else {
        0
    };
    if let Some(hint) = input.empty_hint {
        let content = Rect::new(
            0,
            layout.height,
            client.right,
            client.bottom - status_height,
        );
        let margin = scale(24, dpi);
        let middle = (content.top + content.bottom) / 2;
        unsafe {
            fill(dc, content, input.palette.editor_background);
            SetBkMode(dc, TRANSPARENT as i32);
            let previous = select_font(dc, input.fonts.text);
            SetTextColor(dc, input.palette.muted_foreground);
            draw_text(
                dc,
                hint,
                Rect::new(
                    content.left + margin,
                    middle - scale(20, dpi),
                    (content.right - margin).max(content.left + margin),
                    middle + scale(20, dpi),
                ),
                DT_CENTER | DT_WORDBREAK | DT_NOPREFIX,
            );
            restore_font(dc, previous);
        }
    }

    if let Some(status) = input.status {
        let bar = Rect::new(
            0,
            client.bottom - status_height,
            client.right,
            client.bottom,
        );
        let margin = scale(10, dpi);
        let gap = scale(24, dpi);
        let text = Rect::new(
            margin,
            bar.top,
            (bar.right - margin).max(margin),
            bar.bottom,
        );
        let format = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;
        unsafe {
            fill(dc, bar, input.palette.strip_background);
            SetBkMode(dc, TRANSPARENT as i32);
            let previous = select_font(dc, input.fonts.text);
            // The document details keep their full width; a long notice is what gets ellipsized.
            let right_width = if status.right.is_empty() {
                0
            } else {
                SetTextColor(dc, input.palette.muted_foreground);
                draw_text(dc, &status.right, text, format | DT_RIGHT);
                measure_text(dc, &status.right, format) + gap
            };
            SetTextColor(dc, input.palette.strip_foreground);
            draw_text(
                dc,
                &status.left,
                Rect::new(
                    text.left,
                    text.top,
                    (text.right - right_width).max(text.left),
                    text.bottom,
                ),
                format | DT_END_ELLIPSIS,
            );
            restore_font(dc, previous);
        }
    }

    unsafe {
        EndPaint(hwnd, &paint);
    }
}

unsafe fn paint_strip_buffered(
    dc: HDC,
    layout: &TitleBarLayout,
    dpi: u32,
    maximized: bool,
    input: &TitlePaint<'_>,
) {
    let strip = layout.strip();
    let (width, height) = (strip.right, strip.bottom);
    if width <= 0 || height <= 0 {
        return;
    }
    let memory = unsafe { CreateCompatibleDC(dc) };
    let bitmap = if memory.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { CreateCompatibleBitmap(dc, width, height) }
    };
    if bitmap.is_null() {
        unsafe { draw_strip(dc, layout, dpi, maximized, input) };
    } else {
        unsafe {
            let previous = SelectObject(memory, bitmap);
            draw_strip(memory, layout, dpi, maximized, input);
            BitBlt(dc, 0, 0, width, height, memory, 0, 0, SRCCOPY);
            SelectObject(memory, previous);
            DeleteObject(bitmap);
        }
    }
    if !memory.is_null() {
        unsafe {
            DeleteDC(memory);
        }
    }
}

unsafe fn draw_strip(
    dc: HDC,
    layout: &TitleBarLayout,
    dpi: u32,
    maximized: bool,
    input: &TitlePaint<'_>,
) {
    let palette = input.palette;
    let pointer = input.pointer;
    let centered = DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX;
    unsafe {
        fill(dc, layout.strip(), palette.strip_background);
        SetBkMode(dc, TRANSPARENT as i32);
    }
    let previous_font = unsafe { select_font(dc, input.fonts.text) };

    let saved = unsafe { SaveDC(dc) };
    unsafe {
        IntersectClipRect(
            dc,
            layout.tabs.left,
            layout.tabs.top,
            layout.tabs.right,
            layout.tabs.bottom,
        );
    }
    for (index, title) in input.titles.iter().enumerate() {
        let tab = layout.tab(index);
        if tab.right <= layout.tabs.left || tab.left >= layout.tabs.right {
            continue;
        }
        let close = layout.close_tab(index);
        let selected = index == input.active;
        let tab_hovered = matches!(
            pointer.hovered,
            Some(HitTarget::Tab(hovered) | HitTarget::CloseTab(hovered)) if hovered == index
        );
        let (background, foreground) = if selected {
            (palette.active_tab_background(), palette.editor_foreground)
        } else if tab_hovered {
            (palette.hover_background, palette.hover_foreground)
        } else {
            (palette.strip_background, palette.muted_foreground)
        };
        let close_hovered = pointer.hovered == Some(HitTarget::CloseTab(index));
        unsafe {
            fill(dc, tab, background);
            select_font(dc, input.fonts.text);
            SetTextColor(dc, foreground);
            draw_text(
                dc,
                title,
                Rect::new(
                    tab.left + scale(12, dpi),
                    tab.top,
                    close.left.max(tab.left),
                    tab.bottom,
                ),
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            if close_hovered {
                let pressed = pointer.is_pressed(HitTarget::CloseTab(index));
                fill(
                    dc,
                    close.centered_square(scale(24, dpi)),
                    if pressed {
                        palette.pressed_background
                    } else {
                        palette.hover_background
                    },
                );
            }
            select_font(dc, input.fonts.glyph);
            SetTextColor(
                dc,
                if close_hovered || (tab_hovered && !selected) {
                    palette.hover_foreground
                } else {
                    palette.muted_foreground
                },
            );
            draw_text(dc, GLYPH_CLOSE, close, centered);
        }
    }
    if saved != 0 {
        unsafe {
            RestoreDC(dc, saved);
        }
    }
    if let Some(thumb) = layout.scroll_thumb() {
        // A slim line at rest that thickens into the full grab band under the pointer.
        let active = pointer.hovered == Some(HitTarget::ScrollBar)
            || pointer.is_pressed(HitTarget::ScrollBar);
        let thumb = if active {
            thumb
        } else {
            Rect::new(
                thumb.left,
                thumb.bottom - scale(3, dpi),
                thumb.right,
                thumb.bottom,
            )
        };
        unsafe { fill(dc, thumb, palette.pressed_background) };
    }

    let overflow_hovered = pointer.hovered == Some(HitTarget::Overflow);
    unsafe {
        select_font(dc, input.fonts.glyph);
        if overflow_hovered {
            fill(
                dc,
                layout.overflow.centered_square(scale(32, dpi)),
                if pointer.is_pressed(HitTarget::Overflow) {
                    palette.pressed_background
                } else {
                    palette.hover_background
                },
            );
        }
        SetTextColor(
            dc,
            if overflow_hovered {
                palette.hover_foreground
            } else {
                palette.muted_foreground
            },
        );
        draw_text(dc, GLYPH_MORE, layout.overflow, centered);
    }

    let maximize_glyph = if maximized {
        GLYPH_RESTORE
    } else {
        GLYPH_MAXIMIZE
    };
    for (target, rect, glyph) in [
        (HitTarget::Minimize, layout.minimize, GLYPH_MINIMIZE),
        (HitTarget::Maximize, layout.maximize, maximize_glyph),
        (HitTarget::Close, layout.close, GLYPH_CLOSE),
    ] {
        let hovered = pointer.hovered == Some(target);
        let pressed = hovered && pointer.is_pressed(target);
        let (background, foreground) = match (target, hovered, pressed) {
            (HitTarget::Close, true, true) => (
                Some(palette.close_pressed_background),
                palette.close_hover_foreground,
            ),
            (HitTarget::Close, true, false) => (
                Some(palette.close_hover_background),
                palette.close_hover_foreground,
            ),
            (_, true, true) => (Some(palette.pressed_background), palette.hover_foreground),
            (_, true, false) => (Some(palette.hover_background), palette.hover_foreground),
            _ => (None, palette.strip_foreground),
        };
        unsafe {
            if let Some(background) = background {
                fill(dc, rect, background);
            }
            SetTextColor(dc, foreground);
            draw_text(dc, glyph, rect, centered);
        }
    }

    unsafe { restore_font(dc, previous_font) };
}

pub(crate) unsafe fn nonclient_hit_test(
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
    tab_count: usize,
    scroll: i32,
) -> LRESULT {
    let mut dwm_result = 0;
    if unsafe { DwmDefWindowProc(hwnd, WM_NCHITTEST, wparam, lparam, &mut dwm_result) } != 0 {
        return dwm_result;
    }

    let mut point = POINT {
        x: (lparam as u32 & 0xffff) as u16 as i16 as i32,
        y: ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32,
    };
    unsafe {
        ScreenToClient(hwnd, &mut point);
    }
    let layout = layout_for_window(hwnd, tab_count, scroll);
    if point.x < 0 || point.x >= layout.close.right || point.y < 0 || point.y >= layout.height {
        return unsafe { DefWindowProcW(hwnd, WM_NCHITTEST, wparam, lparam) };
    }
    let maximized = unsafe { IsZoomed(hwnd) } != 0;
    (match layout.frame_hit_test(Point::new(point.x, point.y), maximized) {
        HitTarget::Caption => HTCAPTION,
        HitTarget::Minimize => HTMINBUTTON,
        HitTarget::Maximize => HTMAXBUTTON,
        HitTarget::Close => HTCLOSE,
        HitTarget::ResizeTop => HTTOP,
        HitTarget::ResizeTopLeft => HTTOPLEFT,
        HitTarget::ResizeTopRight => HTTOPRIGHT,
        HitTarget::Client
        | HitTarget::Tab(_)
        | HitTarget::CloseTab(_)
        | HitTarget::ScrollBar
        | HitTarget::Overflow => HTCLIENT,
    }) as LRESULT
}

pub(crate) unsafe fn reclaim_caption(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if lparam == 0 {
        return unsafe { DefWindowProcW(hwnd, WM_NCCALCSIZE, wparam, lparam) };
    }
    let client = if wparam == 0 {
        unsafe { &mut *(lparam as *mut RECT) }
    } else {
        unsafe { &mut (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0] }
    };
    let proposed = from_native(*client);
    let work_area = if unsafe { IsZoomed(hwnd) } != 0 {
        work_area_for(proposed)
    } else {
        None
    };
    let default_client = if work_area.is_some() {
        proposed
    } else {
        unsafe {
            DefWindowProcW(hwnd, WM_NCCALCSIZE, wparam, lparam);
        }
        from_native(*client)
    };
    *client = native_rect(frame_client_rect(proposed, default_client, work_area));
    0
}

fn work_area_for(rect: Rect) -> Option<Rect> {
    let monitor = unsafe { MonitorFromRect(&native_rect(rect), MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then(|| from_native(info.rcWork))
}

pub(crate) unsafe fn constrain_maximized_window(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return 0;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return 0;
    }
    let minmax = unsafe { &mut *(lparam as *mut MINMAXINFO) };
    minmax.ptMaxPosition.x = info.rcWork.left - info.rcMonitor.left;
    minmax.ptMaxPosition.y = info.rcWork.top - info.rcMonitor.top;
    minmax.ptMaxSize.x = info.rcWork.right - info.rcWork.left;
    minmax.ptMaxSize.y = info.rcWork.bottom - info.rcWork.top;
    0
}

/// Requests leave notification for the client area or, for caption buttons, the non-client area.
pub(crate) fn track_pointer_leave(hwnd: HWND, nonclient: bool) {
    let mut event = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE | if nonclient { TME_NONCLIENT } else { 0 },
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe {
        TrackMouseEvent(&mut event);
    }
}

/// Best-effort dark frame and editor scrollbars; failures silently keep the light appearance.
pub(crate) fn apply_frame_theme(hwnd: HWND, editor: HWND, dark: bool) {
    let enabled = windows_sys::core::BOOL::from(dark);
    let theme = crate::platform::wide_null("DarkMode_Explorer");
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&raw const enabled).cast(),
            std::mem::size_of::<windows_sys::core::BOOL>() as u32,
        );
        let _ = SetWindowTheme(
            editor,
            if dark {
                theme.as_ptr()
            } else {
                std::ptr::null()
            },
            std::ptr::null(),
        );
    }
}

unsafe fn fill(dc: HDC, rect: Rect, color: u32) {
    unsafe {
        SetDCBrushColor(dc, color);
        FillRect(dc, &native_rect(rect), GetStockObject(DC_BRUSH));
    }
}

unsafe fn select_font(dc: HDC, font: HFONT) -> HFONT {
    if font.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { SelectObject(dc, font) }
}

unsafe fn restore_font(dc: HDC, previous: HFONT) {
    if !previous.is_null() {
        unsafe {
            SelectObject(dc, previous);
        }
    }
}

fn native_rect(rect: Rect) -> RECT {
    RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

fn from_native(rect: RECT) -> Rect {
    Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

unsafe fn measure_text(dc: HDC, text: &str, format: u32) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let wide = text.encode_utf16().collect::<Vec<_>>();
    let mut rect = RECT::default();
    unsafe {
        DrawTextW(
            dc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut rect,
            format | DT_CALCRECT,
        );
    }
    rect.right - rect.left
}

unsafe fn draw_text(dc: HDC, text: &str, rect: Rect, format: u32) {
    // An empty Vec's pointer is dangling, and DT_END_ELLIPSIS makes DrawTextW read through it; the
    // bottom bar's left side is empty whenever no tab is open and no notice is pending.
    if text.is_empty() {
        return;
    }
    let wide = text.encode_utf16().collect::<Vec<_>>();
    let mut rect = native_rect(rect);
    unsafe {
        DrawTextW(dc, wide.as_ptr(), wide.len() as i32, &mut rect, format);
    }
}

#[cfg(test)]
mod tests {
    use super::{HitTarget, PointerState, Rect, Size, TitleBarLayout, frame_client_rect};
    use windows_sys::Win32::UI::WindowsAndMessaging::{HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP};

    #[test]
    fn caption_buttons_sit_flush_with_the_top_right_edge() {
        // Break caught: a layout that starts below y=0 or leaves room right of Close recreates the
        // native caption band above/beside FastPad's own buttons.
        let layout = TitleBarLayout::calculate(Size::new(1200, 800), 96, 2);
        for rect in [layout.minimize, layout.maximize, layout.close] {
            assert_eq!(rect.top, 0);
            assert_eq!(rect.bottom, layout.height);
        }
        assert_eq!(layout.close.right, 1200);
        assert_eq!(layout.maximize.right, layout.close.left);
        assert_eq!(layout.minimize.right, layout.maximize.left);
        assert_eq!(layout.close.right - layout.close.left, 46);
        assert_eq!(
            TitleBarLayout::calculate(Size::new(1200, 800), 192, 2)
                .close
                .left,
            1200 - 92
        );
    }

    #[test]
    fn restored_frame_keeps_the_proposed_top_edge_and_other_borders() {
        let proposed = Rect::new(100, 50, 1380, 770);
        let default_client = Rect::new(108, 81, 1372, 762);
        assert_eq!(
            frame_client_rect(proposed, default_client, None),
            Rect::new(108, 50, 1372, 762)
        );
    }

    #[test]
    fn maximized_frame_is_clamped_to_the_work_area() {
        // Break caught: a maximized window whose frame extends past the monitor would push the
        // caption buttons and the editor's edges off-screen.
        let work = Rect::new(0, 0, 1920, 1040);
        assert_eq!(
            frame_client_rect(
                Rect::new(-8, -8, 1928, 1048),
                Rect::new(0, 23, 1920, 1040),
                Some(work)
            ),
            work
        );
        assert_eq!(
            frame_client_rect(work, Rect::new(8, 31, 1912, 1032), Some(work)),
            work
        );
    }

    #[test]
    fn top_band_of_a_restored_window_resizes_except_over_caption_buttons() {
        let layout = TitleBarLayout::calculate(Size::new(1200, 800), 96, 2);
        let band = layout.resize_border;
        assert!(band > 0 && band < layout.height);
        let tab = layout.tab(1).center();
        let top = super::Point::new(tab.x, 0);
        assert_eq!(layout.frame_hit_test(top, false), HitTarget::ResizeTop);
        assert_eq!(
            layout.frame_hit_test(super::Point::new(0, 0), false),
            HitTarget::ResizeTopLeft
        );
        assert_eq!(
            layout.frame_hit_test(super::Point::new(layout.close.right - 1, 0), false),
            HitTarget::Close,
            "the top-right corner of Close must close, not resize"
        );
        let maximize_top = super::Point::new(layout.maximize.center().x, 0);
        assert_eq!(
            layout.frame_hit_test(maximize_top, false),
            HitTarget::Maximize
        );
        assert_eq!(layout.frame_hit_test(top, true), HitTarget::Tab(1));
        assert_eq!(
            layout.frame_hit_test(super::Point::new(tab.x, band), false),
            HitTarget::Tab(1)
        );
        assert_eq!(
            layout.frame_hit_test(layout.drag_region.center(), false),
            HitTarget::Caption
        );
    }

    #[test]
    fn nonclient_codes_map_only_to_caption_buttons() {
        assert_eq!(
            HitTarget::from_nonclient_code(HTMINBUTTON as usize),
            Some(HitTarget::Minimize)
        );
        assert_eq!(
            HitTarget::from_nonclient_code(HTMAXBUTTON as usize),
            Some(HitTarget::Maximize)
        );
        assert_eq!(
            HitTarget::from_nonclient_code(HTCLOSE as usize),
            Some(HitTarget::Close)
        );
        assert_eq!(HitTarget::from_nonclient_code(HTTOP as usize), None);
    }

    #[test]
    fn pointer_state_tracks_hover_press_and_release_over_the_same_target() {
        let idle = PointerState::default();
        let hovered = idle.hover(Some(HitTarget::Close));
        assert_eq!(hovered.hovered, Some(HitTarget::Close));
        assert_eq!(
            idle.hover(Some(HitTarget::Caption)),
            idle,
            "drag region is not a hover target"
        );

        let pressed = hovered.press(Some(HitTarget::Close));
        assert!(pressed.is_pressed(HitTarget::Close));
        let (released, activated) = pressed.release(Some(HitTarget::Close));
        assert_eq!(activated, Some(HitTarget::Close));
        assert_eq!(released.pressed, None);

        let (released, activated) = pressed
            .hover(Some(HitTarget::Maximize))
            .release(Some(HitTarget::Maximize));
        assert_eq!(
            activated, None,
            "release over a different button must not act"
        );
        assert_eq!(released.pressed, None);
    }

    #[test]
    fn leaving_clears_only_the_matching_area_hover() {
        // Break caught: a client-area leave that arrives after the pointer moved onto a caption
        // button (non-client) would otherwise drop the caption button's hover highlight.
        let caption = PointerState::default()
            .hover(Some(HitTarget::Minimize))
            .press(Some(HitTarget::Minimize));
        assert_eq!(caption.leave(false), caption);
        assert_eq!(caption.leave(true), PointerState::default());

        let tab = PointerState::default().hover(Some(HitTarget::CloseTab(0)));
        assert_eq!(tab.leave(true), tab);
        assert_eq!(tab.leave(false), PointerState::default());
    }

    #[test]
    fn hit_test_preserves_drag_and_maximize_regions() {
        let layout = TitleBarLayout::calculate(Size::new(1200, 800), 144, 2);
        assert_eq!(
            layout.hit_test(layout.maximize.center()),
            HitTarget::Maximize
        );
        assert_eq!(layout.hit_test(layout.tab(0).center()), HitTarget::Tab(0));
        assert_eq!(
            layout.hit_test(layout.drag_region.center()),
            HitTarget::Caption
        );
    }

    #[test]
    fn narrow_window_never_overlaps_caption_buttons() {
        let layout = TitleBarLayout::calculate(Size::new(320, 600), 96, 8);
        assert!(layout.tabs.right() <= layout.minimize.left());
    }

    #[test]
    fn interactive_title_targets_are_disjoint() {
        let layout = TitleBarLayout::calculate(Size::new(1200, 800), 192, 1);
        assert_eq!(
            layout.hit_test(layout.overflow.center()),
            HitTarget::Overflow
        );
        assert_eq!(
            layout.hit_test(layout.minimize.center()),
            HitTarget::Minimize
        );
        assert_eq!(layout.hit_test(layout.close.center()), HitTarget::Close);
    }

    #[test]
    fn crowded_tabs_scroll_inside_the_viewport_and_keep_an_empty_strip() {
        // Break caught: tabs shrinking to unreadable slivers or spilling over the caption buttons,
        // and no empty strip left to drag the window or double-click for a new tab.
        let layout = TitleBarLayout::calculate(Size::new(1200, 800), 96, 30);
        assert!(layout.max_scroll > 0);
        assert!(layout.tab(1).left - layout.tab(0).left >= 120);
        assert!(layout.tabs.right < layout.overflow.left);
        assert!(layout.drag_region.left < layout.drag_region.right);
        assert_eq!(
            layout.hit_test(layout.drag_region.center()),
            HitTarget::Caption
        );
        assert_eq!(layout.hit_test(layout.tab(29).center()), HitTarget::Client);

        let reveal = layout.scroll_to_reveal(29);
        assert_eq!(reveal, layout.max_scroll);
        let scrolled = TitleBarLayout::calculate_scrolled(Size::new(1200, 800), 96, 30, reveal);
        assert!(scrolled.tab(29).right <= scrolled.tabs.right);
        assert_eq!(
            scrolled.hit_test(scrolled.tab(29).center()),
            HitTarget::Tab(29)
        );
        assert_eq!(scrolled.scroll_to_reveal(0), 0);

        let bar = layout
            .scroll_bar
            .expect("overflowing tabs get a scroll bar");
        let thumb = layout.scroll_thumb().unwrap();
        assert_eq!(layout.hit_test(thumb.center()), HitTarget::ScrollBar);
        assert_eq!(layout.scroll_for_thumb(bar.left - 50), 0);
        assert_eq!(layout.scroll_for_thumb(bar.right), layout.max_scroll);
        let dragged = TitleBarLayout::calculate_scrolled(
            Size::new(1200, 800),
            96,
            30,
            layout.scroll_for_thumb(bar.right),
        );
        assert_eq!(dragged.scroll_thumb().unwrap().right, bar.right);
        assert_eq!(scrolled.scroll_by_wheel(-120_000, 120), 0);
        assert_eq!(
            TitleBarLayout::calculate_scrolled(Size::new(1200, 800), 96, 2, 500).scroll,
            0,
            "tabs that fit never stay scrolled"
        );
    }

    #[test]
    fn no_tabs_leaves_the_whole_strip_empty() {
        let layout = TitleBarLayout::calculate(Size::new(1200, 800), 96, 0);
        assert_eq!(layout.drag_region.left, 0);
        assert_eq!(layout.max_scroll, 0);
        assert_eq!(layout.scroll_bar, None);
        assert_eq!(
            layout.hit_test(super::Point::new(10, layout.height / 2)),
            HitTarget::Caption
        );
    }
}
