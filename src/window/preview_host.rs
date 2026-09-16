//! The main window's half of the Markdown preview: the mode, the split layout and its divider,
//! loading documents into the preview window, and (in later tasks) the debounced update pipeline,
//! scroll sync, and link actions. Every entry point takes the main window handle. No function holds
//! an `App` borrow while calling Win32 APIs that can send messages back to the main window
//! (`SetFocus`, `ShowWindow`, `MoveWindow`, `ShellExecuteW`).

use crate::Result;
use crate::document::{DocumentId, Language};
use crate::editor::Editor;
use crate::preview::colors::{PreviewColors, preview_colors};
use crate::preview::dwrite::Graphics;
use crate::preview::incremental::{EditLog, PreviewDocument, SourceText};
use crate::preview::layout::PreviewFonts;
use crate::preview::view::PreviewView;
use crate::preview::{LIVE_UPDATE_LIMIT, PreviewMode, WORKER_PARSE_THRESHOLD};
use crate::window::commands::CommandId;
use crate::window::main_window as host_window;
use crate::window::palette::Palette;
use crate::window::panel::scale;
use crate::window::titlebar::HitTarget;
use std::borrow::Cow;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetFocus, ReleaseCapture, SetCapture, SetFocus,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetMessageTime, KillTimer, MoveWindow, SW_HIDE, SW_SHOWNA, ShowWindow,
};

pub(crate) const PREVIEW_TIMER_ID: usize = 0x4650_5056;
const DIVIDER_WIDTH_AT_96_DPI: i32 = 4;
const MIN_RATIO: f32 = 0.2;
const MAX_RATIO: f32 = 0.8;
const NOT_MARKDOWN_NOTICE: &str = "Markdown preview is available for Markdown documents. Choose \
                                   View > Markdown to treat this tab as Markdown.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(dead_code, reason = "scroll sync sets `Preview` in a later change")]
pub(crate) enum ScrollOrigin {
    #[default]
    None,
    Preview,
}

pub(crate) struct PreviewHost {
    pub(crate) mode: PreviewMode,
    pub(crate) view: Option<PreviewView>,
    /// Declared after `view`: the preview window's own state holds `Rc<Graphics>` references, and
    /// this one must outlive every Direct2D and DirectWrite object they create.
    graphics: Option<Rc<Graphics>>,
    ratio: f32,
    pub(crate) edits: EditLog,
    /// The document the preview currently shows; `None` forces a reload when next shown.
    pub(crate) document: Option<DocumentId>,
    pub(crate) parse_generation: u64,
    dragging: bool,
    last_divider_click: Option<u32>,
    area: Option<RECT>,
    divider: Option<RECT>,
    pub(crate) scroll_origin: ScrollOrigin,
    pub(crate) sync_count: u64,
    pub(crate) hover_text: Option<String>,
    button_hint: Option<&'static str>,
}

/// Written by hand: windows-sys `RECT` and `Graphics` implement no `Debug`.
impl std::fmt::Debug for PreviewHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreviewHost")
            .field("mode", &self.mode)
            .field("view", &self.view)
            .field("graphics_loaded", &self.graphics.is_some())
            .field("ratio", &self.ratio)
            .field("edits", &self.edits)
            .field("document", &self.document)
            .field("parse_generation", &self.parse_generation)
            .field("dragging", &self.dragging)
            .field("area", &self.area.map(rect_tuple))
            .field("divider", &self.divider.map(rect_tuple))
            .field("scroll_origin", &self.scroll_origin)
            .field("sync_count", &self.sync_count)
            .field("hover_text", &self.hover_text)
            .field("button_hint", &self.button_hint)
            .finish_non_exhaustive()
    }
}

impl Default for PreviewHost {
    fn default() -> Self {
        Self {
            mode: PreviewMode::Off,
            view: None,
            graphics: None,
            ratio: 0.5,
            edits: EditLog::default(),
            document: None,
            parse_generation: 0,
            dragging: false,
            last_divider_click: None,
            area: None,
            divider: None,
            scroll_origin: ScrollOrigin::None,
            sync_count: 0,
            hover_text: None,
            button_hint: None,
        }
    }
}

impl PreviewHost {
    pub(crate) fn status_hint(&self) -> Option<String> {
        self.button_hint
            .map(str::to_owned)
            .or_else(|| self.hover_text.clone())
    }
}

const fn rect_tuple(rect: RECT) -> (i32, i32, i32, i32) {
    (rect.left, rect.top, rect.right, rect.bottom)
}

/// `Debug`, `PartialEq`, and `Eq` are written by hand: windows-sys `RECT` derives none of them.
#[derive(Clone, Copy)]
pub(crate) struct ContentRects {
    pub editor: Option<RECT>,
    pub divider: Option<RECT>,
    pub preview: Option<RECT>,
}

impl std::fmt::Debug for ContentRects {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContentRects")
            .field("editor", &self.editor.map(rect_tuple))
            .field("divider", &self.divider.map(rect_tuple))
            .field("preview", &self.preview.map(rect_tuple))
            .finish()
    }
}

impl PartialEq for ContentRects {
    fn eq(&self, other: &Self) -> bool {
        self.editor.map(rect_tuple) == other.editor.map(rect_tuple)
            && self.divider.map(rect_tuple) == other.divider.map(rect_tuple)
            && self.preview.map(rect_tuple) == other.preview.map(rect_tuple)
    }
}

impl Eq for ContentRects {}

pub(crate) fn content_rects(
    area: RECT,
    mode: PreviewMode,
    shown: bool,
    ratio: f32,
    dpi: u32,
) -> ContentRects {
    match (mode, shown) {
        (PreviewMode::Full, true) => ContentRects {
            editor: None,
            divider: None,
            preview: Some(area),
        },
        (PreviewMode::Split, true) => {
            let divider = scale(DIVIDER_WIDTH_AT_96_DPI, dpi);
            let usable = (area.right - area.left - divider).max(0);
            let editor_right =
                area.left + (usable as f32 * ratio.clamp(MIN_RATIO, MAX_RATIO)).round() as i32;
            ContentRects {
                editor: Some(RECT {
                    right: editor_right,
                    ..area
                }),
                divider: Some(RECT {
                    left: editor_right,
                    right: editor_right + divider,
                    ..area
                }),
                preview: Some(RECT {
                    left: editor_right + divider,
                    ..area
                }),
            }
        }
        _ => ContentRects {
            editor: Some(area),
            divider: None,
            preview: None,
        },
    }
}

pub(crate) fn ratio_for_x(area: RECT, x: i32, dpi: u32) -> f32 {
    let divider = scale(DIVIDER_WIDTH_AT_96_DPI, dpi);
    let usable = (area.right - area.left - divider).max(1);
    ((x - area.left - divider / 2) as f32 / usable as f32).clamp(MIN_RATIO, MAX_RATIO)
}

/// Document text read straight from Scintilla's buffer without copying. A slice borrows
/// Scintilla's gap buffer and is valid only until the next document change: use it inside one
/// parse call and never across anything that can modify the editor or pump messages.
pub(crate) struct ScintillaSource<'a>(pub &'a Editor);

impl SourceText for ScintillaSource<'_> {
    fn len(&self) -> usize {
        self.0.length().unwrap_or(0)
    }

    fn slice(&self, range: Range<usize>) -> Cow<'_, str> {
        match self.0.range_bytes(range) {
            Ok(bytes) => String::from_utf8_lossy(bytes),
            Err(_) => Cow::Borrowed(""),
        }
    }

    fn line_of(&self, byte: usize) -> usize {
        self.0.line_from_position(byte).unwrap_or(0)
    }
}

pub(crate) fn with_host<R>(hwnd: HWND, action: impl FnOnce(&mut PreviewHost) -> R) -> Option<R> {
    // SAFETY: the App pointer is used only for the immediate field access inside `action`, which
    // never calls back into Win32.
    unsafe { host_window::app_ptr(hwnd) }
        .map(|mut app| action(&mut unsafe { app.as_mut() }.preview))
}

pub(crate) fn mode(hwnd: HWND) -> PreviewMode {
    with_host(hwnd, |host| host.mode).unwrap_or_default()
}

pub(crate) fn view(hwnd: HWND) -> Option<PreviewView> {
    with_host(hwnd, |host| host.view).flatten()
}

pub(crate) fn editor(hwnd: HWND) -> Option<Editor> {
    unsafe { host_window::app_ptr(hwnd) }.and_then(|app| unsafe { app.as_ref() }.editor.clone())
}

/// The active tab's id, language, and folder.
pub(crate) fn active_document(hwnd: HWND) -> Option<(DocumentId, Language, Option<PathBuf>)> {
    let app = unsafe { host_window::app_ptr(hwnd) }?;
    let document = unsafe { app.as_ref() }.tabs.active()?;
    let folder = document
        .path
        .as_ref()
        .and_then(|path| path.parent().map(PathBuf::from));
    Some((document.id, document.language, folder))
}

pub(crate) fn buttons_visible(hwnd: HWND) -> bool {
    active_document(hwnd).is_some_and(|(_, language, _)| language == Language::Markdown)
}

pub(crate) fn preview_shown(hwnd: HWND) -> bool {
    mode(hwnd) != PreviewMode::Off && buttons_visible(hwnd) && view(hwnd).is_some()
}

pub(crate) fn full_view_hwnd(hwnd: HWND) -> Option<HWND> {
    (mode(hwnd) == PreviewMode::Full && preview_shown(hwnd))
        .then(|| view(hwnd).map(|view| view.hwnd()))
        .flatten()
}

pub(crate) fn run_command(hwnd: HWND, command: CommandId) {
    let next = match command {
        CommandId::MarkdownPreviewCycle => match mode(hwnd) {
            PreviewMode::Off => PreviewMode::Split,
            PreviewMode::Split => PreviewMode::Full,
            PreviewMode::Full => PreviewMode::Off,
        },
        CommandId::MarkdownPreviewSide => PreviewMode::Split,
        CommandId::MarkdownPreviewFull => PreviewMode::Full,
        CommandId::MarkdownPreviewClose => PreviewMode::Off,
        _ => return,
    };
    set_mode(hwnd, next);
}

/// Title-strip buttons toggle: the pressed button turns the preview off.
pub(crate) fn click_button(hwnd: HWND, target: HitTarget) {
    let button_mode = match target {
        HitTarget::PreviewSide => PreviewMode::Split,
        HitTarget::PreviewFull => PreviewMode::Full,
        _ => return,
    };
    set_mode(
        hwnd,
        if mode(hwnd) == button_mode {
            PreviewMode::Off
        } else {
            button_mode
        },
    );
}

pub(crate) fn escape(hwnd: HWND) {
    if mode(hwnd) == PreviewMode::Full {
        set_mode(hwnd, PreviewMode::Split);
    }
}

pub(crate) fn set_mode(hwnd: HWND, next: PreviewMode) {
    if next != PreviewMode::Off && !buttons_visible(hwnd) {
        host_window::push_notice(hwnd, NOT_MARKDOWN_NOTICE.to_owned());
        return;
    }
    let previous = mode(hwnd);
    with_host(hwnd, |host| host.mode = next);
    if next == PreviewMode::Off {
        close_view(hwnd);
    }
    sync_visibility(hwnd);
    let focus = match (previous, next) {
        (_, PreviewMode::Full) => view(hwnd).map(|view| view.hwnd()),
        (PreviewMode::Full, _) => unsafe { host_window::editor_hwnd(hwnd) },
        _ => None,
    };
    if let Some(target) = focus {
        unsafe { SetFocus(target) };
    }
}

fn close_view(hwnd: HWND) {
    let closed = with_host(hwnd, |host| {
        host.edits = EditLog::default();
        host.document = None;
        host.divider = None;
        host.hover_text = None;
        host.view.take()
    })
    .flatten();
    unsafe { KillTimer(hwnd, PREVIEW_TIMER_ID) };
    if let Some(view) = closed {
        let had_focus = unsafe { GetFocus() } == view.hwnd();
        view.destroy();
        if had_focus && let Some(editor) = unsafe { host_window::editor_hwnd(hwnd) } {
            unsafe { SetFocus(editor) };
        }
    }
    host_window::invalidate_status_bar(hwnd);
}

fn appearance(hwnd: HWND) -> (PreviewColors, PreviewFonts, bool) {
    let theme = host_window::effective_theme(hwnd);
    unsafe { host_window::app_ptr(hwnd) }
        .map(|app| {
            let app = unsafe { app.as_ref() };
            let high_contrast = app.theme.is_some_and(|system| system.high_contrast);
            let dark = Palette::for_cached_theme(app.theme, app.settings.theme).dark_frame;
            (
                preview_colors(theme, high_contrast),
                PreviewFonts::from_settings(&app.settings.font_face, app.settings.font_size),
                dark,
            )
        })
        .unwrap_or_else(|| {
            (
                preview_colors(theme, false),
                PreviewFonts::from_settings("Consolas", 11),
                false,
            )
        })
}

pub(crate) fn refresh_appearance(hwnd: HWND) {
    if let Some(view) = view(hwnd) {
        let (colors, fonts, dark) = appearance(hwnd);
        view.set_appearance(colors, fonts, dark);
    }
}

fn ensure_view(hwnd: HWND) -> Result<PreviewView> {
    if let Some(view) = view(hwnd) {
        return Ok(view);
    }
    let started = Instant::now();
    let graphics = match with_host(hwnd, |host| host.graphics.clone()).flatten() {
        Some(graphics) => graphics,
        None => {
            let graphics = Rc::new(Graphics::load()?);
            with_host(hwnd, |host| host.graphics = Some(Rc::clone(&graphics)));
            graphics
        }
    };
    let (colors, fonts, dark) = appearance(hwnd);
    let view = PreviewView::create(hwnd, graphics, colors, fonts.clone())?;
    view.set_appearance(colors, fonts, dark);
    view.mark_opened(started);
    with_host(hwnd, |host| {
        host.view = Some(view);
        host.document = None;
    });
    Ok(view)
}

/// Shows or hides the preview and editor for the current mode and active tab, loading the active
/// document when the preview shows something else. Called after mode changes, tab activation or
/// closing, language changes, and file loads.
pub(crate) fn sync_visibility(hwnd: HWND) {
    let markdown = buttons_visible(hwnd);
    if let Some(app) = unsafe { host_window::app_ptr(hwnd) } {
        unsafe { app.as_ref() }.tabs.set_preview_buttons(markdown);
    }
    let wanted = mode(hwnd);
    let mut view = view(hwnd);
    if wanted != PreviewMode::Off && markdown && view.is_none() {
        match ensure_view(hwnd) {
            Ok(created) => view = Some(created),
            Err(error) => {
                with_host(hwnd, |host| host.mode = PreviewMode::Off);
                host_window::push_notice(
                    hwnd,
                    format!("FastPad could not open the Markdown preview: {error}"),
                );
            }
        }
    }
    let shown = wanted != PreviewMode::Off && markdown && view.is_some();
    if let Some(view) = view {
        view.set_centered(wanted == PreviewMode::Full);
        if !shown {
            with_host(hwnd, |host| {
                host.document = None;
                host.edits = EditLog::default();
                host.hover_text = None;
            });
            unsafe { KillTimer(hwnd, PREVIEW_TIMER_ID) };
        }
    }
    if shown {
        let active = active_document(hwnd).map(|(id, ..)| id);
        if with_host(hwnd, |host| host.document).flatten() != active {
            load_active_document(hwnd, false);
        }
    }
    let editor_hwnd = unsafe { host_window::editor_hwnd(hwnd) };
    let hide_editor = shown && wanted == PreviewMode::Full;
    if let (Some(editor), Some(view)) = (editor_hwnd, view)
        && hide_editor
        && unsafe { GetFocus() } == editor
    {
        unsafe { SetFocus(view.hwnd()) };
    }
    if let Some(view) = view {
        unsafe { ShowWindow(view.hwnd(), if shown { SW_SHOWNA } else { SW_HIDE }) };
    }
    if let Some(editor) = editor_hwnd
        && host_window::tab_count(hwnd) > 0
    {
        unsafe { ShowWindow(editor, if hide_editor { SW_HIDE } else { SW_SHOWNA }) };
    }
    host_window::layout_editor_and_find_bar(hwnd);
    host_window::invalidate_title_strip(hwnd);
}

pub(crate) fn load_active_document(hwnd: HWND, force: bool) {
    let (Some(view), Some(editor), Some((id, _, folder))) =
        (view(hwnd), editor(hwnd), active_document(hwnd))
    else {
        return;
    };
    let started = Instant::now();
    unsafe { KillTimer(hwnd, PREVIEW_TIMER_ID) };
    with_host(hwnd, |host| {
        host.document = Some(id);
        host.edits = EditLog::default();
    });
    let length = editor.length().unwrap_or(0);
    let top_line = editor
        .first_visible_line()
        .and_then(|line| editor.doc_line_from_visible(line))
        .unwrap_or(0);
    if length > LIVE_UPDATE_LIMIT && !force {
        view.set_paused(true);
        view.replace_document(PreviewDocument::default(), folder, started);
    } else if length > WORKER_PARSE_THRESHOLD {
        view.set_paused(length > LIVE_UPDATE_LIMIT);
        view.set_document_dir(folder);
        spawn_parse(hwnd, &editor, id, started);
    } else {
        view.set_paused(false);
        view.replace_document(PreviewDocument::default(), folder, started);
        view.reparse(&ScintillaSource(&editor), started);
        view.scroll_to_line(top_line);
    }
}

/// Task 15 replaces this body with the worker parse; until then large documents parse inline.
fn spawn_parse(hwnd: HWND, editor: &Editor, _document: DocumentId, started: Instant) {
    if let Some(view) = view(hwnd) {
        view.reparse(&ScintillaSource(editor), started);
    }
}

/// Positions the preview for `area` and returns where the editor goes.
pub(crate) fn layout(hwnd: HWND, area: RECT, dpi: u32) -> ContentRects {
    let (mode, ratio) =
        with_host(hwnd, |host| (host.mode, host.ratio)).unwrap_or((PreviewMode::Off, 0.5));
    let rects = content_rects(area, mode, preview_shown(hwnd), ratio, dpi);
    with_host(hwnd, |host| {
        host.area = Some(area);
        host.divider = rects.divider;
    });
    if let (Some(view), Some(rect)) = (view(hwnd), rects.preview) {
        unsafe {
            MoveWindow(
                view.hwnd(),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                1,
            )
        };
    }
    rects
}

pub(crate) fn divider_rect(hwnd: HWND) -> Option<RECT> {
    with_host(hwnd, |host| host.divider).flatten()
}

fn over_divider(hwnd: HWND, x: i32, y: i32) -> bool {
    divider_rect(hwnd)
        .is_some_and(|rect| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom)
}

pub(crate) fn begin_divider_drag(hwnd: HWND, x: i32, y: i32) -> bool {
    if !over_divider(hwnd, x, y) {
        return false;
    }
    let now = unsafe { GetMessageTime() } as u32;
    let double_click_time = unsafe { GetDoubleClickTime() };
    let double = with_host(hwnd, |host| {
        let double = host
            .last_divider_click
            .is_some_and(|last| now.wrapping_sub(last) <= double_click_time);
        host.last_divider_click = if double { None } else { Some(now) };
        if double {
            host.ratio = 0.5;
        } else {
            host.dragging = true;
        }
        double
    })
    .unwrap_or(false);
    if double {
        host_window::layout_editor_and_find_bar(hwnd);
        host_window::invalidate_title_strip(hwnd);
        return true;
    }
    if let Some(view) = view(hwnd) {
        view.set_live_resize(true);
    }
    unsafe { SetCapture(hwnd) };
    true
}

pub(crate) fn drag_divider(hwnd: HWND, x: i32) -> bool {
    let Some(area) = with_host(hwnd, |host| host.dragging.then_some(host.area).flatten()).flatten()
    else {
        return false;
    };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    with_host(hwnd, |host| host.ratio = ratio_for_x(area, x, dpi));
    host_window::layout_editor_and_find_bar(hwnd);
    host_window::invalidate_title_strip(hwnd);
    true
}

pub(crate) fn end_divider_drag(hwnd: HWND) -> bool {
    if !with_host(hwnd, |host| std::mem::replace(&mut host.dragging, false)).unwrap_or(false) {
        return false;
    }
    unsafe { ReleaseCapture() };
    if let Some(view) = view(hwnd) {
        view.set_live_resize(false);
    }
    true
}

pub(crate) fn cursor_over_divider(hwnd: HWND) -> bool {
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) } == 0 || unsafe { ScreenToClient(hwnd, &mut point) } == 0
    {
        return false;
    }
    with_host(hwnd, |host| host.dragging).unwrap_or(false) || over_divider(hwnd, point.x, point.y)
}

pub(crate) fn button_hover(hwnd: HWND, target: Option<HitTarget>) {
    let hint = match target {
        Some(HitTarget::PreviewSide) => {
            Some("Open Preview to the Side (Ctrl+Shift+V cycles preview modes)")
        }
        Some(HitTarget::PreviewFull) => Some("Open Preview (Ctrl+Shift+V cycles preview modes)"),
        _ => None,
    };
    let changed = with_host(hwnd, |host| {
        std::mem::replace(&mut host.button_hint, hint) != hint
    })
    .unwrap_or(false);
    if changed {
        host_window::invalidate_status_bar(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: RECT = RECT {
        left: 0,
        top: 40,
        right: 1004,
        bottom: 700,
    };

    #[test]
    fn off_or_hidden_previews_give_the_editor_the_whole_area() {
        let whole = ContentRects {
            editor: Some(AREA),
            divider: None,
            preview: None,
        };
        assert_eq!(content_rects(AREA, PreviewMode::Off, true, 0.5, 96), whole);
        assert_eq!(
            content_rects(AREA, PreviewMode::Split, false, 0.5, 96),
            whole
        );
    }

    #[test]
    fn split_places_a_divider_between_editor_and_preview() {
        let rects = content_rects(AREA, PreviewMode::Split, true, 0.5, 96);
        let expected = ContentRects {
            editor: Some(RECT { right: 500, ..AREA }),
            divider: Some(RECT {
                left: 500,
                right: 504,
                ..AREA
            }),
            preview: Some(RECT { left: 504, ..AREA }),
        };
        assert_eq!(rects, expected);
    }

    #[test]
    fn split_ratio_is_clamped() {
        let wide = content_rects(AREA, PreviewMode::Split, true, 0.95, 96);
        assert_eq!(wide.editor.unwrap().right, 800);
        let narrow = content_rects(AREA, PreviewMode::Split, true, 0.01, 96);
        assert_eq!(narrow.editor.unwrap().right, 200);
    }

    #[test]
    fn full_mode_hides_the_editor() {
        let rects = content_rects(AREA, PreviewMode::Full, true, 0.5, 96);
        assert_eq!(
            rects,
            ContentRects {
                editor: None,
                divider: None,
                preview: Some(AREA)
            }
        );
    }

    #[test]
    fn dragging_maps_the_pointer_to_a_clamped_ratio() {
        assert_eq!(ratio_for_x(AREA, 502, 96), 0.5);
        assert_eq!(ratio_for_x(AREA, 0, 96), 0.2);
        assert_eq!(ratio_for_x(AREA, 5000, 96), 0.8);
    }
}
