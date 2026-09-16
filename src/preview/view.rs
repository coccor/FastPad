//! The `FastPadPreview` child window. It owns the preview model, lays out only the blocks that
//! scroll into view, and paints them with Direct2D. User actions are posted to the parent (never
//! sent), so the parent can call back into the view without re-entering a borrowed state.

use crate::Result;
use crate::platform::{last_error, wide_null};
use crate::preview::colors::{ColorRole, PreviewColors};
use crate::preview::dwrite::Graphics;
use crate::preview::heights::{HeightIndex, estimate_height, line_for_offset, offset_for_line};
use crate::preview::images::ImageCache;
use crate::preview::incremental::{Edit, PreviewDocument, SourceText, Update};
use crate::preview::layout::{
    LaidBlock, LayoutContext, PreviewFonts, block_has_link, layout_block,
};
use crate::preview::links::SlugSet;
use crate::preview::model::BlockKind;
use crate::preview::render::{Brushes, color_f, create_hwnd_target, draw_ops};
use crate::window::{
    WM_FASTPAD_PREVIEW_ACTIVATE, WM_FASTPAD_PREVIEW_ESCAPE, WM_FASTPAD_PREVIEW_HOVER,
    WM_FASTPAD_PREVIEW_IMAGE, WM_FASTPAD_PREVIEW_LINK, WM_FASTPAD_PREVIEW_REFRESH,
    WM_FASTPAD_PREVIEW_SCROLLED,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use windows::Win32::Foundation::D2DERR_RECREATE_TARGET;
use windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U;
use windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget;
use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL};
use windows_numerics::{Matrix3x2, Vector2};
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{SetScrollInfo, SetWindowTheme, WM_MOUSELEAVE};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DOWN, VK_END, VK_ESCAPE,
    VK_HOME, VK_NEXT, VK_PRIOR, VK_RETURN, VK_SHIFT, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DLGC_WANTALLKEYS, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetClientRect,
    GetParent, GetScrollInfo, GetWindowLongPtrW, HTCLIENT, IDC_ARROW, IDC_HAND, LoadCursorW,
    PostMessageW, RegisterClassW, SB_BOTTOM, SB_LINEDOWN, SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP,
    SB_THUMBTRACK, SB_TOP, SB_VERT, SCROLLINFO, SIF_ALL, SIF_TRACKPOS, SetCursor,
    SetWindowLongPtrW, WM_ERASEBKGND, WM_GETDLGCODE, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT,
    WM_SETCURSOR, WM_SETFOCUS, WM_SIZE, WM_VSCROLL, WNDCLASSW, WS_CHILD, WS_CLIPSIBLINGS,
    WS_TABSTOP, WS_VSCROLL,
};

const CLASS_NAME: &str = "FastPadPreview";
/// `MK_SHIFT` from WinUser.h; its windows-sys home needs a feature FastPad does not enable.
const MK_SHIFT: u32 = 0x0004;
const PADDING: f32 = 16.0;
const MAX_CENTERED_WIDTH: f32 = 980.0;
const PAUSED_BAR_HEIGHT: f32 = 32.0;
const PAUSED_TEXT: &str =
    "Live preview is paused for this large file. Click here to refresh the preview.";

#[derive(Clone, Copy, Debug, Default)]
pub struct PreviewStats {
    pub block_count: usize,
    pub revision: u64,
    pub first_frame_micros: u64,
    pub last_update_micros: u64,
    /// Updates whose frame has been painted; `last_update_micros` belongs to the latest one.
    pub painted_updates: u64,
}

/// `Debug`, `PartialEq`, and `Eq` are written by hand: windows-sys `RECT` derives none of them.
#[derive(Clone)]
pub struct VisibleLink {
    pub text: String,
    pub dest: String,
    /// Client pixels of the link's first line.
    pub rect: RECT,
}

impl std::fmt::Debug for VisibleLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let RECT {
            left,
            top,
            right,
            bottom,
        } = self.rect;
        formatter
            .debug_struct("VisibleLink")
            .field("text", &self.text)
            .field("dest", &self.dest)
            .field("rect", &(left, top, right, bottom))
            .finish()
    }
}

impl PartialEq for VisibleLink {
    fn eq(&self, other: &Self) -> bool {
        let rect = |rect: &RECT| (rect.left, rect.top, rect.right, rect.bottom);
        self.text == other.text && self.dest == other.dest && rect(&self.rect) == rect(&other.rect)
    }
}

impl Eq for VisibleLink {}

pub fn content_frame(view_width: f32, centered: bool) -> (f32, f32) {
    let available = (view_width - 2.0 * PADDING).max(1.0);
    if centered && available > MAX_CENTERED_WIDTH {
        (
            PADDING + (available - MAX_CENTERED_WIDTH) / 2.0,
            MAX_CENTERED_WIDTH,
        )
    } else {
        (PADDING, available)
    }
}

struct ViewState {
    hwnd: HWND,
    target: Option<ID2D1HwndRenderTarget>,
    brushes: Option<Brushes>,
    colors: PreviewColors,
    fonts: PreviewFonts,
    document: PreviewDocument,
    document_dir: Option<PathBuf>,
    layouts: Vec<Option<LaidBlock>>,
    heights: HeightIndex,
    layout_width: f32,
    scroll_y: f32,
    h_scroll: HashMap<usize, f32>,
    hover: Option<(usize, usize)>,
    pressed: Option<(usize, usize)>,
    focus: Option<(usize, usize)>,
    tracking_mouse: bool,
    images: ImageCache,
    centered: bool,
    paused: bool,
    live_resize: bool,
    /// Set after a failed paint schedules its one retry; cleared by the next successful paint.
    paint_retried: bool,
    /// Heading slugs and their block indices; `None` until an anchor lookup needs them after the
    /// document changed.
    anchors: Option<Vec<(String, usize)>>,
    stats: PreviewStats,
    opened_at: Option<Instant>,
    update_started: Option<Instant>,
    /// On-screen links as of the last paint, shared with the accessibility provider.
    accessible: Arc<RwLock<Vec<VisibleLink>>>,
    /// Declared last so it drops last: the target, brushes, text layouts, and bitmaps above all
    /// live in d2d1.dll and dwrite.dll, which the final `Graphics` reference unloads.
    graphics: Rc<Graphics>,
}

/// A handle to a preview window; copies are cheap and all refer to the same window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewView {
    hwnd: HWND,
}

impl PreviewView {
    pub fn create(
        parent: HWND,
        graphics: Rc<Graphics>,
        colors: PreviewColors,
        fonts: PreviewFonts,
    ) -> Result<Self> {
        let hwnd = create_window(parent)?;
        let state = Box::new(ViewState {
            hwnd,
            graphics,
            target: None,
            brushes: None,
            colors,
            fonts,
            document: PreviewDocument::default(),
            document_dir: None,
            layouts: Vec::new(),
            heights: HeightIndex::default(),
            layout_width: 0.0,
            scroll_y: 0.0,
            h_scroll: HashMap::new(),
            hover: None,
            pressed: None,
            focus: None,
            tracking_mouse: false,
            images: ImageCache::new(hwnd, WM_FASTPAD_PREVIEW_IMAGE),
            centered: false,
            paused: false,
            live_resize: false,
            paint_retried: false,
            anchors: None,
            stats: PreviewStats::default(),
            opened_at: None,
            update_started: None,
            accessible: Arc::new(RwLock::new(Vec::new())),
        });
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize) };
        Ok(Self { hwnd })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn destroy(self) {
        unsafe { DestroyWindow(self.hwnd) };
    }

    fn with<R>(&self, action: impl FnOnce(&mut ViewState) -> R) -> Option<R> {
        with_state(self.hwnd, action)
    }

    pub fn replace_document(
        &self,
        document: PreviewDocument,
        document_dir: Option<PathBuf>,
        started: Instant,
    ) {
        self.with(|state| {
            state.document = document;
            state.document_dir = document_dir;
            state.scroll_y = 0.0;
            accept_update(state, Update::Full, started);
        });
    }

    pub fn apply_edits<S: SourceText + ?Sized>(
        &self,
        source: &S,
        edits: &[Edit],
        started: Instant,
        allow_full_parse: bool,
    ) -> Option<Update> {
        self.with(|state| {
            let update = if allow_full_parse {
                Some(state.document.apply(source, edits))
            } else {
                state.document.try_apply(source, edits)
            }?;
            accept_update(state, update.clone(), started);
            Some(update)
        })
        .flatten()
    }

    pub fn reparse<S: SourceText + ?Sized>(&self, source: &S, started: Instant) -> Update {
        self.with(|state| {
            let update = state.document.reparse(source);
            accept_update(state, update.clone(), started);
            update
        })
        .unwrap_or(Update::Unchanged)
    }

    pub fn set_appearance(&self, colors: PreviewColors, fonts: PreviewFonts, dark_scrollbar: bool) {
        self.with(|state| {
            state.colors = colors;
            state.fonts = fonts;
            state.brushes = None;
            relayout(state);
        });
        let theme = wide_null(if dark_scrollbar {
            "DarkMode_Explorer"
        } else {
            "Explorer"
        });
        unsafe { SetWindowTheme(self.hwnd, theme.as_ptr(), std::ptr::null()) };
        invalidate(self.hwnd);
    }

    pub fn set_document_dir(&self, document_dir: Option<PathBuf>) {
        self.with(|state| {
            if state.document_dir != document_dir {
                state.document_dir = document_dir;
                relayout(state);
            }
        });
        invalidate(self.hwnd);
    }

    pub fn set_centered(&self, centered: bool) {
        self.with(|state| state.centered = centered);
        invalidate(self.hwnd);
    }

    pub fn set_paused(&self, paused: bool) {
        self.with(|state| state.paused = paused);
        invalidate(self.hwnd);
    }

    pub fn is_paused(&self) -> bool {
        self.with(|state| state.paused).unwrap_or(false)
    }

    pub fn colors(&self) -> PreviewColors {
        self.with(|state| state.colors).unwrap_or_else(|| {
            crate::preview::colors::preview_colors(crate::platform::theme::Theme::Light, false)
        })
    }

    pub fn set_live_resize(&self, live: bool) {
        self.with(|state| state.live_resize = live);
        invalidate(self.hwnd);
    }

    pub fn mark_opened(&self, at: Instant) {
        self.with(|state| state.opened_at = Some(at));
    }

    pub fn scroll_to_line(&self, line: usize) {
        self.with(|state| {
            let ViewState {
                document, heights, ..
            } = state;
            let offset = offset_for_line(&document.blocks, heights, line);
            set_scroll(state, offset, false);
        });
    }

    pub fn top_line(&self) -> usize {
        self.with(|state| {
            let ViewState {
                document,
                heights,
                scroll_y,
                ..
            } = state;
            line_for_offset(&document.blocks, heights, *scroll_y)
        })
        .unwrap_or(0)
    }

    pub fn scroll_to_anchor(&self, anchor: &str) -> bool {
        self.with(|state| {
            let Some(index) = anchors(state)
                .iter()
                .find(|(slug, _)| slug == anchor)
                .map(|(_, index)| *index)
            else {
                return false;
            };
            let top = state.heights.top(index);
            set_scroll(state, top, true);
            true
        })
        .unwrap_or(false)
    }

    pub fn stats(&self) -> PreviewStats {
        self.with(|state| state.stats).unwrap_or_default()
    }

    pub fn visible_links(&self) -> Vec<VisibleLink> {
        self.with(visible_links).unwrap_or_default()
    }

    pub fn accessible_links(&self) -> Arc<RwLock<Vec<VisibleLink>>> {
        self.with(|state| Arc::clone(&state.accessible))
            .unwrap_or_default()
    }
}

/// Kept out of `PreviewView::create` so the public function never hands a raw `HWND` straight to an
/// unsafe call (`clippy::not_unsafe_ptr_arg_deref`); Windows validates the handle itself.
fn create_window(parent: HWND) -> Result<HWND> {
    register_class()?;
    let class = wide_null(CLASS_NAME);
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_CLIPSIBLINGS | WS_TABSTOP | WS_VSCROLL,
            0,
            0,
            0,
            0,
            parent,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err(last_error());
    }
    Ok(hwnd)
}

fn register_class() -> Result<()> {
    let class = wide_null(CLASS_NAME);
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(preview_proc),
        hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
        hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
        lpszClassName: class.as_ptr(),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&window_class) } == 0
        && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
    {
        return Err(last_error());
    }
    Ok(())
}

fn with_state<R>(hwnd: HWND, action: impl FnOnce(&mut ViewState) -> R) -> Option<R> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut ViewState;
    // SAFETY: the pointer is owned by this window until WM_NCDESTROY, and every entry point runs on
    // the window's thread without sending messages while the borrow is live.
    (!pointer.is_null()).then(|| action(unsafe { &mut *pointer }))
}

fn invalidate(hwnd: HWND) {
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
}

fn dpi_scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0
}

fn view_size(hwnd: HWND) -> (f32, f32) {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect) };
    let scale = dpi_scale(hwnd);
    (
        (rect.right - rect.left) as f32 / scale,
        (rect.bottom - rect.top) as f32 / scale,
    )
}

fn bar_height(state: &ViewState) -> f32 {
    if state.paused { PAUSED_BAR_HEIGHT } else { 0.0 }
}

/// Estimated heights for `range` of the document's blocks.
fn estimates(state: &ViewState, range: std::ops::Range<usize>) -> Vec<f32> {
    let line_height = state.fonts.body_size * 1.5;
    let gap = 16.0 * state.fonts.unit();
    state.document.blocks[range]
        .iter()
        .map(|block| estimate_height(block.lines.len(), line_height, gap))
        .collect()
}

fn anchors(state: &mut ViewState) -> &[(String, usize)] {
    state.anchors.get_or_insert_with(|| {
        let mut slugs = SlugSet::default();
        state
            .document
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| match &block.kind {
                BlockKind::Heading { text, .. } => Some((slugs.unique(text.plain_text()), index)),
                _ => None,
            })
            .collect()
    })
}

/// Forgets every layout for a new document: heights fall back to estimates.
fn reset_layouts(state: &mut ViewState) {
    state.layouts = (0..state.document.blocks.len()).map(|_| None).collect();
    let estimates = estimates(state, 0..state.document.blocks.len());
    state.heights.reset(estimates);
    state.hover = None;
    state.pressed = None;
    state.focus = None;
}

/// Forgets every layout of the current document (width, fonts, brushes, or folder changed) while
/// keeping the reading position: the block at the top stays at the top.
fn relayout(state: &mut ViewState) {
    let anchor = (!state.heights.is_empty()).then(|| state.heights.anchor(state.scroll_y));
    reset_layouts(state);
    if let Some(anchor) = anchor
        && anchor.0 < state.heights.len()
    {
        state.scroll_y = state.heights.scroll_for_anchor(anchor);
    }
}

/// Everything tied to the Direct2D device. Layouts hold brushes as drawing effects, so the next
/// paint lays out again once brushes are recreated; decoded pixels survive for new bitmaps.
fn drop_device_resources(state: &mut ViewState) {
    state.target = None;
    state.brushes = None;
    state.images.release_bitmaps();
}

fn accept_update(state: &mut ViewState, update: Update, started: Instant) {
    let repaint = match update {
        Update::Unchanged => return,
        Update::Full => {
            reset_layouts(state);
            state.h_scroll.clear();
            true
        }
        Update::Replaced { old, new } => {
            let (_, view_height) = view_size(state.hwnd);
            let view_top = state.scroll_y;
            let view_bottom = view_top + view_height;
            // Only the replaced range is visited: running sums past it stay stale until paint
            // needs them, so an edit costs its own size, not the document's.
            let old_top = state.heights.top(old.start);
            let old_height = old
                .clone()
                .map(|index| state.heights.height(index))
                .sum::<f32>();
            let old_bottom = old_top + old_height;
            state.layouts.splice(old.clone(), new.clone().map(|_| None));
            let new_estimates = estimates(state, new.clone());
            let new_height = new_estimates.iter().sum::<f32>();
            state.heights.splice(old.clone(), &new_estimates);
            let delta = new.len() as isize - old.len() as isize;
            state.h_scroll = state
                .h_scroll
                .drain()
                .filter_map(|(index, offset)| {
                    if index < old.start {
                        Some((index, offset))
                    } else if index >= old.end {
                        Some(((index as isize + delta) as usize, offset))
                    } else {
                        None
                    }
                })
                .collect();
            state.hover = None;
            state.pressed = None;
            state.focus = None;
            let on_screen = old_top < view_bottom && old_bottom >= view_top;
            let total_changed = (new_height - old_height).abs() > 0.01;
            on_screen || old.len() != new.len() || total_changed
        }
    };
    // Rebuilt on the next anchor lookup: slugging every heading on every keystroke's update is
    // O(document).
    state.anchors = None;
    state.stats.block_count = state.document.blocks.len();
    state.stats.revision = state.document.revision;
    if repaint {
        state.update_started = Some(started);
        invalidate(state.hwnd);
    }
}

/// Lays out `indices` that have no layout yet; true when any height changed.
fn ensure_layouts(state: &mut ViewState, indices: &[usize]) -> Result<bool> {
    let ViewState {
        hwnd,
        graphics,
        brushes,
        fonts,
        document,
        document_dir,
        images,
        layouts,
        heights,
        layout_width,
        ..
    } = state;
    let Some(brushes) = brushes.as_ref() else {
        return Ok(false);
    };
    let mut requests = Vec::new();
    let mut changed = false;
    {
        let sizes = |path: &Path| images.size(path);
        let context =
            LayoutContext::new(graphics, brushes, fonts, document_dir.as_deref(), &sizes)?;
        for &index in indices {
            if index >= document.blocks.len() || layouts[index].is_some() {
                continue;
            }
            let laid = layout_block(&context, &document.blocks[index].kind, *layout_width)?;
            for slot in &laid.images {
                if let Some(path) = &slot.path
                    && !images.is_failed(path)
                {
                    requests.push(path.clone());
                }
            }
            heights.set_measured(index, laid.height);
            layouts[index] = Some(laid);
            changed = true;
        }
    }
    // Decode at the content width in device pixels. The cache caps that at the natural width and
    // decodes again only when a wider pane needs more pixels than the last decode has.
    let pixel_width = (*layout_width * dpi_scale(*hwnd)).ceil().max(1.0) as u32;
    for path in requests {
        images.request(&path, pixel_width);
    }
    Ok(changed)
}

fn visible_indices(state: &mut ViewState, view_height: f32) -> Vec<usize> {
    let bottom = state.scroll_y + view_height;
    let mut index = state.heights.index_at(state.scroll_y);
    let mut indices = Vec::new();
    while index < state.document.blocks.len() && state.heights.top(index) < bottom {
        indices.push(index);
        index += 1;
    }
    indices
}

fn max_scroll(state: &mut ViewState, view_height: f32) -> f32 {
    (state.heights.total() - (view_height - bar_height(state))).max(0.0)
}

fn set_scroll(state: &mut ViewState, y: f32, user: bool) {
    let (_, view_height) = view_size(state.hwnd);
    let clamped = y.clamp(0.0, max_scroll(state, view_height));
    if (clamped - state.scroll_y).abs() < 0.01 {
        return;
    }
    state.scroll_y = clamped;
    invalidate(state.hwnd);
    if user {
        let ViewState {
            document,
            heights,
            scroll_y,
            hwnd,
            ..
        } = state;
        let line = line_for_offset(&document.blocks, heights, *scroll_y);
        unsafe { PostMessageW(GetParent(*hwnd), WM_FASTPAD_PREVIEW_SCROLLED, line, 0) };
    }
}

/// What `WM_PAINT` must do once the state borrow has ended.
struct PaintOutcome {
    /// Scroll bar state for a completed frame. `SetScrollInfo` can send `WM_SIZE` synchronously,
    /// so it must never run while `ViewState` is borrowed.
    scroll: Option<SCROLLINFO>,
    /// Paint again: the device was lost, or a visible block is still without a layout.
    repaint: bool,
}

fn paint(state: &mut ViewState) -> Result<PaintOutcome> {
    let hwnd = state.hwnd;
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client) };
    let (pixel_width, pixel_height) = (
        (client.right - client.left).max(1) as u32,
        (client.bottom - client.top).max(1) as u32,
    );
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    if state.target.is_none() {
        let target = create_hwnd_target(&state.graphics, hwnd, pixel_width, pixel_height, dpi)?;
        state.brushes = None;
        state.target = Some(target);
    }
    if state.brushes.is_none() {
        let target = state.target.as_ref().expect("created above");
        state.brushes = Some(Brushes::create(target, &state.colors)?);
        relayout(state);
    }
    let (view_width, view_height) = view_size(hwnd);
    let (content_left, content_width) = content_frame(view_width, state.centered);
    if (content_width - state.layout_width).abs() > 0.5
        && (!state.live_resize || state.layout_width == 0.0)
    {
        state.layout_width = content_width;
        relayout(state);
    }
    let bar = bar_height(state);
    for _ in 0..3 {
        let anchor = state.heights.anchor(state.scroll_y);
        let visible = visible_indices(state, view_height - bar);
        if !ensure_layouts(state, &visible)? {
            break;
        }
        state.scroll_y = state.heights.scroll_for_anchor(anchor);
    }
    state.scroll_y = state.scroll_y.clamp(0.0, max_scroll(state, view_height));

    let mut visible = visible_indices(state, view_height - bar);
    if visible.iter().any(|index| state.layouts[*index].is_none()) {
        // The clamp or the last anchor pass uncovered blocks the loop did not lay out.
        let anchor = state.heights.anchor(state.scroll_y);
        if ensure_layouts(state, &visible)? {
            let restored = state.heights.scroll_for_anchor(anchor);
            state.scroll_y = restored.clamp(0.0, max_scroll(state, view_height));
        }
        visible = visible_indices(state, view_height - bar);
    }
    let repaint = visible.iter().any(|index| state.layouts[*index].is_none());
    let tops = visible
        .iter()
        .map(|index| state.heights.top(*index))
        .collect::<Vec<_>>();
    let ViewState {
        target,
        brushes,
        layouts,
        images,
        h_scroll,
        focus,
        scroll_y,
        colors,
        graphics,
        fonts,
        paused,
        ..
    } = state;
    let target = target.as_ref().expect("created above");
    let brushes = brushes.as_ref().expect("created above");
    let result = unsafe {
        target.BeginDraw();
        target.SetTransform(&Matrix3x2::identity());
        target.Clear(Some(&color_f(colors.background)));
        for (index, top) in visible.iter().zip(tops) {
            let Some(laid) = layouts[*index].as_ref() else {
                continue;
            };
            let y = top - *scroll_y + bar;
            let offset = h_scroll.get(index).copied().unwrap_or(0.0);
            draw_ops(
                target,
                brushes,
                &laid.ops,
                &laid.images,
                images,
                content_left,
                y,
                offset,
            );
            if let Some((focus_block, focus_link)) = *focus
                && focus_block == *index
                && let Some(link) = laid.links.get(focus_link)
            {
                for rect in link.visible_rects(offset) {
                    target.DrawRectangle(
                        &rect.offset(content_left, y).inflate(2.0).to_d2d(),
                        brushes.get(ColorRole::Focus),
                        2.0,
                        None,
                    );
                }
            }
        }
        if *paused {
            let bar_rect =
                crate::preview::render::RectF::new(0.0, 0.0, view_width, PAUSED_BAR_HEIGHT);
            target.FillRectangle(&bar_rect.to_d2d(), brushes.get(ColorRole::CodeBackground));
            if let Ok(format) = graphics.text_format(
                &fonts.body_family,
                fonts.body_size,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            ) {
                let text = PAUSED_TEXT.encode_utf16().collect::<Vec<_>>();
                if let Ok(layout) = graphics.dwrite.CreateTextLayout(
                    &text,
                    &format,
                    view_width - 2.0 * PADDING,
                    PAUSED_BAR_HEIGHT,
                ) {
                    target.DrawTextLayout(
                        Vector2 { X: PADDING, Y: 6.0 },
                        &layout,
                        brushes.get(ColorRole::Text),
                        windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
            }
        }
        target.EndDraw(None, None)
    };
    match result {
        Err(error) if error.code() == D2DERR_RECREATE_TARGET => {
            drop_device_resources(state);
            Ok(PaintOutcome {
                scroll: None,
                repaint: true,
            })
        }
        Err(error) => Err(crate::preview::dwrite::hresult_error(error)),
        Ok(()) => {
            if let Some(opened) = state.opened_at.take() {
                state.stats.first_frame_micros = opened.elapsed().as_micros().max(1) as u64;
            }
            if let Some(started) = state.update_started.take() {
                state.stats.last_update_micros = started.elapsed().as_micros().max(1) as u64;
                state.stats.painted_updates += 1;
            }
            let links = visible_links(state);
            *state
                .accessible
                .write()
                .unwrap_or_else(|error| error.into_inner()) = links;
            Ok(PaintOutcome {
                scroll: Some(scroll_info(state, view_height)),
                repaint,
            })
        }
    }
}

fn scroll_info(state: &mut ViewState, view_height: f32) -> SCROLLINFO {
    let scale = dpi_scale(state.hwnd);
    SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        nMin: 0,
        nMax: (state.heights.total() * scale) as i32,
        nPage: ((view_height - bar_height(state)) * scale).max(0.0) as u32,
        nPos: (state.scroll_y * scale) as i32,
        nTrackPos: 0,
    }
}

fn client_point(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam & 0xFFFF) as u16 as i16 as i32,
        ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32,
    )
}

fn link_at(state: &mut ViewState, x: i32, y: i32) -> Option<(usize, usize)> {
    let scale = dpi_scale(state.hwnd);
    let (view_width, _) = view_size(state.hwnd);
    let (content_left, _) = content_frame(view_width, state.centered);
    let document_y = y as f32 / scale + state.scroll_y - bar_height(state);
    let index = state.heights.index_at(document_y);
    let top = state.heights.top(index);
    let laid = state.layouts.get(index)?.as_ref()?;
    let block_x = x as f32 / scale - content_left;
    let block_y = document_y - top;
    let offset = state.h_scroll.get(&index).copied().unwrap_or(0.0);
    laid.links
        .iter()
        .position(|link| {
            link.visible_rects(offset)
                .iter()
                .any(|rect| rect.contains(block_x, block_y))
        })
        .map(|link| (index, link))
}

fn link_dest(state: &ViewState, (block, link): (usize, usize)) -> Option<String> {
    Some(
        state
            .layouts
            .get(block)?
            .as_ref()?
            .links
            .get(link)?
            .dest
            .clone(),
    )
}

fn set_underline(state: &ViewState, target: Option<(usize, usize)>, underline: bool) {
    if let Some((block, link)) = target
        && let Some(Some(laid)) = state.layouts.get(block)
        && let Some(link) = laid.links.get(link)
    {
        let _ = unsafe { link.layout.SetUnderline(underline, link.range) };
    }
}

fn post_link(hwnd: HWND, dest: String) {
    let payload = Box::into_raw(Box::new(dest));
    if unsafe {
        PostMessageW(
            GetParent(hwnd),
            WM_FASTPAD_PREVIEW_LINK,
            0,
            payload as isize,
        )
    } == 0
    {
        drop(unsafe { Box::from_raw(payload) });
    }
}

fn post_hover(hwnd: HWND, dest: Option<String>) {
    let payload = Box::into_raw(Box::new(dest));
    if unsafe {
        PostMessageW(
            GetParent(hwnd),
            WM_FASTPAD_PREVIEW_HOVER,
            0,
            payload as isize,
        )
    } == 0
    {
        drop(unsafe { Box::from_raw(payload) });
    }
}

fn visible_links(state: &mut ViewState) -> Vec<VisibleLink> {
    let scale = dpi_scale(state.hwnd);
    let (view_width, view_height) = view_size(state.hwnd);
    let (content_left, _) = content_frame(view_width, state.centered);
    let bar = bar_height(state);
    let mut links = Vec::new();
    for index in visible_indices(state, view_height - bar) {
        let top = state.heights.top(index) - state.scroll_y + bar;
        let offset = state.h_scroll.get(&index).copied().unwrap_or(0.0);
        let Some(Some(laid)) = state.layouts.get(index) else {
            continue;
        };
        for link in &laid.links {
            // Links scrolled out of a wide table's clip are not on screen.
            let Some(rect) = link.visible_rects(offset).first().copied() else {
                continue;
            };
            let rect = rect.offset(content_left, top);
            links.push(VisibleLink {
                text: link.text.clone(),
                dest: link.dest.clone(),
                rect: RECT {
                    left: (rect.left * scale) as i32,
                    top: (rect.top * scale) as i32,
                    right: (rect.right * scale) as i32,
                    bottom: (rect.bottom * scale) as i32,
                },
            });
        }
    }
    links
}

/// Moves keyboard focus to the next (or previous) link in document order and scrolls it into view.
/// Blocks without links are skipped using the model alone; only the block that receives focus is
/// laid out.
fn move_focus(state: &mut ViewState, forward: bool) {
    let count = state.document.blocks.len();
    if count == 0 {
        return;
    }
    let (mut block, mut link) = match state.focus {
        Some((block, link)) => (block.min(count - 1), Some(link)),
        None => (state.heights.index_at(state.scroll_y).min(count - 1), None),
    };
    // One extra step lets the search wrap back into the block it started from.
    for _ in 0..=count {
        if block_has_link(&state.document.blocks[block].kind) {
            let anchor = state.heights.anchor(state.scroll_y);
            match ensure_layouts(state, &[block]) {
                Err(_) => return,
                // A block above the reading position changed height: keep the view still.
                Ok(true) => state.scroll_y = state.heights.scroll_for_anchor(anchor),
                Ok(false) => {}
            }
            let links = state.layouts[block]
                .as_ref()
                .map_or(0, |laid| laid.links.len());
            let next = match (link, forward) {
                (None, true) if links > 0 => Some(0),
                (None, false) if links > 0 => Some(links - 1),
                (Some(current), true) if current + 1 < links => Some(current + 1),
                (Some(current), false) if current > 0 && current <= links => Some(current - 1),
                _ => None,
            };
            if let Some(next) = next {
                focus_link(state, block, next);
                return;
            }
        }
        link = None;
        block = if forward {
            (block + 1) % count
        } else {
            (block + count - 1) % count
        };
    }
}

/// Focuses a laid-out link and reveals it: horizontally inside a wide table's clip, then
/// vertically in the view.
fn focus_link(state: &mut ViewState, block: usize, link: usize) {
    state.focus = Some((block, link));
    let Some(laid) = state.layouts.get(block).and_then(Option::as_ref) else {
        return;
    };
    let Some(hit) = laid.links.get(link) else {
        return;
    };
    let rect = hit.rects.first().copied().unwrap_or_default();
    if hit.scrolls
        && let Some(clip) = hit.clip
    {
        let current = state.h_scroll.get(&block).copied().unwrap_or(0.0);
        let limit = (laid.scroll_width - clip.width()).max(0.0);
        let mut offset = current;
        if rect.right - offset > clip.right {
            offset = rect.right - clip.right;
        }
        if rect.left - offset < clip.left {
            offset = rect.left - clip.left;
        }
        let offset = offset.clamp(0.0, limit);
        if (offset - current).abs() > 0.01 {
            state.h_scroll.insert(block, offset);
        }
    }
    let top = state.heights.top(block);
    let (_, view_height) = view_size(state.hwnd);
    let visible_height = view_height - bar_height(state);
    if top + rect.top < state.scroll_y || top + rect.bottom > state.scroll_y + visible_height {
        set_scroll(state, top + rect.top - visible_height / 3.0, true);
    }
    invalidate(state.hwnd);
}

unsafe extern "system" fn preview_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCDESTROY => {
            let pointer = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut ViewState;
            if !pointer.is_null() {
                drop(unsafe { Box::from_raw(pointer) });
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let outcome = with_state(hwnd, |state| match paint(state) {
                Ok(outcome) if outcome.scroll.is_some() => {
                    state.paint_retried = false;
                    outcome
                }
                result => {
                    if result.is_err() {
                        drop_device_resources(state);
                    }
                    // Retry a failed frame once; a device that keeps failing must not spin.
                    PaintOutcome {
                        scroll: None,
                        repaint: !std::mem::replace(&mut state.paint_retried, true),
                    }
                }
            });
            unsafe { ValidateRect(hwnd, std::ptr::null()) };
            // The state borrow has ended: these calls may send messages back to this window.
            if let Some(outcome) = outcome {
                if let Some(info) = outcome.scroll {
                    unsafe { SetScrollInfo(hwnd, SB_VERT, &info, 1) };
                }
                if outcome.repaint {
                    invalidate(hwnd);
                }
            }
            0
        }
        WM_SIZE => {
            let width = (lparam & 0xFFFF) as u32;
            let height = ((lparam >> 16) & 0xFFFF) as u32;
            with_state(hwnd, |state| {
                if let Some(target) = &state.target
                    && unsafe {
                        target.Resize(&D2D_SIZE_U {
                            width: width.max(1),
                            height: height.max(1),
                        })
                    }
                    .is_err()
                {
                    drop_device_resources(state);
                }
            });
            invalidate(hwnd);
            0
        }
        WM_GETDLGCODE => DLGC_WANTALLKEYS as LRESULT,
        WM_SETFOCUS | WM_KILLFOCUS => {
            invalidate(hwnd);
            0
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = ((wparam >> 16) & 0xFFFF) as u16 as i16 as f32;
            let keys = (wparam & 0xFFFF) as u32;
            with_state(hwnd, |state| {
                let step = state.fonts.body_size * 1.5 * 3.0 * delta / 120.0;
                let horizontal = message == WM_MOUSEHWHEEL || keys & MK_SHIFT != 0;
                if horizontal {
                    let mut point = windows_sys::Win32::Foundation::POINT {
                        x: (lparam & 0xFFFF) as u16 as i16 as i32,
                        y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32,
                    };
                    unsafe { windows_sys::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point) };
                    let document_y =
                        point.y as f32 / dpi_scale(hwnd) + state.scroll_y - bar_height(state);
                    let index = state.heights.index_at(document_y);
                    let (view_width, _) = view_size(hwnd);
                    let (_, content_width) = content_frame(view_width, state.centered);
                    if let Some(Some(laid)) = state.layouts.get(index)
                        && laid.scroll_width > content_width
                    {
                        let limit = laid.scroll_width - content_width;
                        let sign = if message == WM_MOUSEHWHEEL { 1.0 } else { -1.0 };
                        let offset = state.h_scroll.entry(index).or_insert(0.0);
                        *offset = (*offset + sign * step).clamp(0.0, limit);
                        invalidate(hwnd);
                    }
                } else {
                    let target = state.scroll_y - step;
                    set_scroll(state, target, true);
                }
            });
            0
        }
        WM_VSCROLL => {
            with_state(hwnd, |state| {
                let (_, view_height) = view_size(hwnd);
                let line = state.fonts.body_size * 1.5;
                let page = (view_height - bar_height(state) - line).max(line);
                let target = match (wparam & 0xFFFF) as i32 {
                    SB_LINEUP => state.scroll_y - line,
                    SB_LINEDOWN => state.scroll_y + line,
                    SB_PAGEUP => state.scroll_y - page,
                    SB_PAGEDOWN => state.scroll_y + page,
                    SB_TOP => 0.0,
                    SB_BOTTOM => f32::MAX,
                    SB_THUMBTRACK => {
                        let mut info = SCROLLINFO {
                            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                            fMask: SIF_TRACKPOS,
                            ..Default::default()
                        };
                        unsafe { GetScrollInfo(hwnd, SB_VERT, &mut info) };
                        info.nTrackPos as f32 / dpi_scale(hwnd)
                    }
                    _ => return,
                };
                set_scroll(state, target, true);
            });
            0
        }
        WM_KEYDOWN => {
            let key = wparam as u16;
            if key == VK_ESCAPE {
                unsafe { PostMessageW(GetParent(hwnd), WM_FASTPAD_PREVIEW_ESCAPE, 0, 0) };
                return 0;
            }
            with_state(hwnd, |state| {
                let (_, view_height) = view_size(hwnd);
                let line = state.fonts.body_size * 1.5;
                let page = (view_height - bar_height(state) - line).max(line);
                match key {
                    VK_UP => set_scroll(state, state.scroll_y - 2.0 * line, true),
                    VK_DOWN => set_scroll(state, state.scroll_y + 2.0 * line, true),
                    VK_PRIOR => set_scroll(state, state.scroll_y - page, true),
                    VK_NEXT => set_scroll(state, state.scroll_y + page, true),
                    VK_HOME => set_scroll(state, 0.0, true),
                    VK_END => set_scroll(state, f32::MAX, true),
                    VK_TAB => {
                        let backward = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
                        move_focus(state, !backward);
                    }
                    VK_RETURN => {
                        if let Some(dest) = state.focus.and_then(|focus| link_dest(state, focus)) {
                            post_link(hwnd, dest);
                        }
                    }
                    _ => {}
                }
            });
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = client_point(lparam);
            with_state(hwnd, |state| {
                if !state.tracking_mouse {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    state.tracking_mouse = unsafe { TrackMouseEvent(&mut track) } != 0;
                }
                let hovered = link_at(state, x, y);
                if hovered != state.hover {
                    set_underline(state, state.hover, false);
                    set_underline(state, hovered, true);
                    state.hover = hovered;
                    post_hover(hwnd, hovered.and_then(|link| link_dest(state, link)));
                    invalidate(hwnd);
                }
            });
            0
        }
        WM_MOUSELEAVE => {
            with_state(hwnd, |state| {
                state.tracking_mouse = false;
                if state.hover.is_some() {
                    set_underline(state, state.hover, false);
                    state.hover = None;
                    post_hover(hwnd, None);
                    invalidate(hwnd);
                }
            });
            0
        }
        WM_SETCURSOR if (lparam & 0xFFFF) as u32 == HTCLIENT => {
            let over_link = with_state(hwnd, |state| state.hover.is_some()).unwrap_or(false);
            unsafe {
                SetCursor(LoadCursorW(
                    std::ptr::null_mut(),
                    if over_link { IDC_HAND } else { IDC_ARROW },
                ))
            };
            1
        }
        WM_LBUTTONDOWN => {
            unsafe { SetFocus(hwnd) };
            let (x, y) = client_point(lparam);
            with_state(hwnd, |state| state.pressed = link_at(state, x, y));
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = client_point(lparam);
            with_state(hwnd, |state| {
                if state.paused && (y as f32) < PAUSED_BAR_HEIGHT * dpi_scale(hwnd) {
                    unsafe { PostMessageW(GetParent(hwnd), WM_FASTPAD_PREVIEW_REFRESH, 0, 0) };
                    return;
                }
                let released = link_at(state, x, y);
                if released.is_some()
                    && released == state.pressed.take()
                    && let Some(dest) = released.and_then(|link| link_dest(state, link))
                {
                    post_link(hwnd, dest);
                }
            });
            0
        }
        windows_sys::Win32::UI::WindowsAndMessaging::WM_GETOBJECT
            if lparam as i32 == windows_sys::Win32::UI::WindowsAndMessaging::OBJID_CLIENT =>
        {
            // Clone the snapshot inside the borrow; `LresultFromObject` runs after it ends.
            match with_state(hwnd, |state| Arc::clone(&state.accessible)) {
                Some(links) => crate::preview::accessible::object_result(hwnd, links, wparam),
                None => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
            }
        }
        WM_FASTPAD_PREVIEW_ACTIVATE => {
            let dest = with_state(hwnd, |state| {
                state
                    .accessible
                    .read()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(wparam)
                    .map(|link| link.dest.clone())
            })
            .flatten();
            if let Some(dest) = dest {
                post_link(hwnd, dest);
            }
            0
        }
        WM_FASTPAD_PREVIEW_IMAGE => {
            with_state(hwnd, |state| {
                if state.images.drain() {
                    for layout in &mut state.layouts {
                        if layout.as_ref().is_some_and(|laid| !laid.images.is_empty()) {
                            *layout = None;
                        }
                    }
                    invalidate(hwnd);
                }
            });
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::theme::Theme;
    use crate::preview::colors::preview_colors;
    use crate::preview::render::TestWindow;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_END, VK_ESCAPE, VK_TAB};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MSG, MoveWindow, PM_REMOVE, PeekMessageW, SendMessageW, WM_KEYDOWN, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WM_PAINT,
    };

    fn view_with(parent: &TestWindow, source: &str) -> PreviewView {
        let graphics = Rc::new(Graphics::load().unwrap());
        let view = PreviewView::create(
            parent.0,
            graphics,
            preview_colors(Theme::Light, false),
            PreviewFonts::from_settings("Consolas", 11),
        )
        .unwrap();
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
            ShowWindow(parent.0, SW_SHOWNOACTIVATE);
            MoveWindow(view.hwnd(), 0, 0, 400, 300, 0);
            ShowWindow(view.hwnd(), SW_SHOWNOACTIVATE);
        }
        view.mark_opened(Instant::now());
        view.replace_document(PreviewDocument::parse(source), None, Instant::now());
        // The view paints without BeginPaint, so a direct WM_PAINT renders synchronously.
        unsafe { SendMessageW(view.hwnd(), WM_PAINT, 0, 0) };
        view
    }

    fn take_posted(parent: &TestWindow, message: u32) -> Option<MSG> {
        let mut msg = MSG::default();
        (unsafe { PeekMessageW(&mut msg, parent.0, message, message, PM_REMOVE) } != 0)
            .then_some(msg)
    }

    #[test]
    fn content_is_padded_and_capped_when_centered() {
        assert_eq!(content_frame(400.0, false), (16.0, 368.0));
        assert_eq!(content_frame(2000.0, true), (510.0, 980.0));
        assert_eq!(content_frame(500.0, true), (16.0, 468.0));
    }

    #[test]
    fn painting_a_document_records_stats() {
        let parent = TestWindow::new(800, 600);
        let view = view_with(&parent, "# A\n\ntext\n");
        let stats = view.stats();
        assert_eq!(stats.block_count, 2);
        assert!(stats.revision > 0);
        assert!(stats.first_frame_micros > 0);
        view.destroy();
    }

    #[test]
    fn scrolling_to_a_line_reports_the_same_top_line() {
        let parent = TestWindow::new(800, 600);
        let source = (0..200)
            .map(|index| format!("para {index}\n\n"))
            .collect::<String>();
        let view = view_with(&parent, &source);
        view.scroll_to_line(100);
        assert_eq!(view.top_line(), 100);
        view.destroy();
    }

    #[test]
    fn end_key_scrolls_to_the_bottom_and_reports_the_scroll() {
        let parent = TestWindow::new(800, 600);
        let source = (0..200)
            .map(|index| format!("para {index}\n\n"))
            .collect::<String>();
        let view = view_with(&parent, &source);
        unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_END as usize, 0) };
        assert!(view.top_line() > 0);
        assert!(take_posted(&parent, WM_FASTPAD_PREVIEW_SCROLLED).is_some());
        view.destroy();
    }

    #[test]
    fn escape_is_forwarded_to_the_parent() {
        let parent = TestWindow::new(800, 600);
        let view = view_with(&parent, "text\n");
        unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_ESCAPE as usize, 0) };
        assert!(take_posted(&parent, WM_FASTPAD_PREVIEW_ESCAPE).is_some());
        view.destroy();
    }

    #[test]
    fn clicking_a_link_posts_its_destination() {
        let parent = TestWindow::new(800, 600);
        let view = view_with(&parent, "[site](https://x.dev)\n");
        let rect = view
            .visible_links()
            .into_iter()
            .next()
            .expect("a laid-out link")
            .rect;
        assert_eq!(view.accessible_links().read().unwrap()[0].text, "site");
        let point =
            (((rect.top + rect.bottom) / 2) << 16 | ((rect.left + rect.right) / 2)) as isize;
        unsafe {
            SendMessageW(view.hwnd(), WM_LBUTTONDOWN, 0, point);
            SendMessageW(view.hwnd(), WM_LBUTTONUP, 0, point);
        }
        let message = take_posted(&parent, WM_FASTPAD_PREVIEW_LINK).expect("link message");
        let dest = unsafe { Box::from_raw(message.lParam as *mut String) };
        assert_eq!(*dest, "https://x.dev");
        view.destroy();
    }

    #[test]
    fn anchors_scroll_to_their_heading() {
        let parent = TestWindow::new(800, 600);
        let mut source = (0..100)
            .map(|index| format!("para {index}\n\n"))
            .collect::<String>();
        source.push_str("## Deep Heading\n");
        let view = view_with(&parent, &source);
        assert!(view.scroll_to_anchor("deep-heading"));
        assert!(view.top_line() >= 150);
        assert!(!view.scroll_to_anchor("missing"));
        view.destroy();
    }

    fn repaint(view: &PreviewView) {
        unsafe { SendMessageW(view.hwnd(), WM_PAINT, 0, 0) };
    }

    #[test]
    fn width_changes_keep_the_reading_position() {
        let parent = TestWindow::new(800, 600);
        let source = (0..200)
            .map(|index| format!("para {index}\n\n"))
            .collect::<String>();
        let view = view_with(&parent, &source);
        view.scroll_to_line(100);
        repaint(&view);
        assert_eq!(view.top_line(), 100);
        unsafe { MoveWindow(view.hwnd(), 0, 0, 300, 300, 0) };
        repaint(&view);
        assert_eq!(view.top_line(), 100);
        view.destroy();
    }

    #[test]
    fn links_clipped_out_of_a_wide_table_are_hidden_until_focus_reveals_them() {
        let parent = TestWindow::new(800, 600);
        let wide = "wide ".repeat(40);
        let view = view_with(
            &parent,
            &format!("| {wide} | [far](https://far.dev) |\n|---|---|\n| a | b |\n"),
        );
        assert!(view.visible_links().is_empty());
        assert!(view.accessible_links().read().unwrap().is_empty());
        unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_TAB as usize, 0) };
        repaint(&view);
        let links = view.visible_links();
        assert_eq!(links.len(), 1, "focus scrolls the table to the link");
        let mut client = RECT::default();
        unsafe { GetClientRect(view.hwnd(), &mut client) };
        assert!(links[0].rect.left >= 0 && links[0].rect.right <= client.right);
        view.destroy();
    }

    #[test]
    fn unchanged_updates_do_not_record_an_update() {
        let parent = TestWindow::new(800, 600);
        let source = "# A\n\ntext\n";
        let view = view_with(&parent, source);
        let before = view.stats().last_update_micros;
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_eq!(
            view.apply_edits(source, &[], Instant::now(), true),
            Some(Update::Unchanged)
        );
        repaint(&view);
        assert_eq!(view.stats().last_update_micros, before);
        view.destroy();
    }

    #[test]
    fn anchors_follow_incremental_edits() {
        // Break caught: anchors are rebuilt lazily; a stale list would miss a heading typed into
        // the document or point a moved heading at its old block.
        let parent = TestWindow::new(800, 600);
        let mut source = (0..100)
            .map(|index| format!("para {index}\n\n"))
            .collect::<String>();
        source.push_str("## Deep Heading\n");
        let view = view_with(&parent, &source);
        assert!(view.scroll_to_anchor("deep-heading"));
        let position = source.find("para 50").unwrap();
        let inserted = "## Added Heading\n\n";
        source.insert_str(position, inserted);
        let update = view.apply_edits(
            source.as_str(),
            &[Edit {
                position,
                removed: 0,
                inserted: inserted.len(),
                lines_delta: 2,
            }],
            Instant::now(),
            false,
        );
        assert!(
            matches!(update, Some(Update::Replaced { .. })),
            "{update:?}"
        );
        view.scroll_to_line(0);
        repaint(&view);
        assert!(view.scroll_to_anchor("added-heading"));
        assert_eq!(view.top_line(), 100);
        assert!(view.scroll_to_anchor("deep-heading"));
        assert!(view.top_line() >= 152);
        view.destroy();
    }
}
