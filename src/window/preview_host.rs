//! The main window's half of the Markdown preview: the mode, the split layout and its divider,
//! loading documents into the preview window, and (in later tasks) the debounced update pipeline,
//! scroll sync, and link actions. Every entry point takes the main window handle. No function holds
//! an `App` borrow while calling Win32 APIs that can send messages back to the main window
//! (`SetFocus`, `ShowWindow`, `MoveWindow`, `ShellExecuteW`).

use crate::Result;
use crate::document::{DocumentId, Language};
use crate::editor::scintilla_constants::SC_MOD_INSERTTEXT;
use crate::editor::{Editor, ScintillaNotification};
use crate::platform::wide_null;
use crate::preview::colors::{PreviewColors, preview_colors};
use crate::preview::dwrite::Graphics;
use crate::preview::incremental::{Edit, EditLog, Pending, PreviewDocument, SourceText};
use crate::preview::layout::PreviewFonts;
use crate::preview::links::{LinkAction, classify_link};
use crate::preview::view::PreviewView;
use crate::preview::{
    LIVE_UPDATE_LIMIT, PREVIEW_UPDATE_DELAY_MS, PreviewMode, WORKER_PARSE_THRESHOLD,
};
use crate::window::WM_FASTPAD_PREVIEW_PARSED;
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
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetFocus, ReleaseCapture, SetCapture, SetFocus,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetMessageTime, KillTimer, MoveWindow, PostMessageW, SW_HIDE, SW_SHOWNA,
    SW_SHOWNORMAL, SetTimer, ShowWindow,
};

pub(crate) const PREVIEW_TIMER_ID: usize = 0x4650_5056;
const DIVIDER_WIDTH_AT_96_DPI: i32 = 4;
const MIN_RATIO: f32 = 0.2;
const MAX_RATIO: f32 = 0.8;
const NOT_MARKDOWN_NOTICE: &str = "Markdown preview is available for Markdown documents. Choose \
                                   View > Markdown to treat this tab as Markdown.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    /// Set while a worker parse for `parse_generation` is outstanding. The view's block model is
    /// stale until it lands, so edits must not be applied incrementally on top of it.
    pub(crate) full_parse_pending: bool,
    dragging: bool,
    last_divider_click: Option<u32>,
    area: Option<RECT>,
    divider: Option<RECT>,
    pub(crate) scroll_origin: ScrollOrigin,
    pub(crate) sync_count: u64,
    pub(crate) hover_text: Option<String>,
    button_hint: Option<&'static str>,
}

/// Written by hand: windows-sys `RECT` implements no `Debug`.
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
            .field("full_parse_pending", &self.full_parse_pending)
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
            full_parse_pending: false,
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
        host.full_parse_pending = false;
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
                host.full_parse_pending = false;
                host.hover_text = None;
            });
            unsafe { KillTimer(hwnd, PREVIEW_TIMER_ID) };
            // A hidden preview keeps only its window and the factories; the document (up to
            // LIVE_UPDATE_LIMIT of text) is parsed again when the preview shows.
            view.release();
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
    let (previous, mode) = with_host(hwnd, |host| {
        let previous = host.document.replace(id);
        host.edits = EditLog::default();
        // Any worker parse still running describes older text.
        host.parse_generation += 1;
        host.full_parse_pending = false;
        (previous, host.mode)
    })
    .unwrap_or((None, PreviewMode::Off));
    let length = editor.length().unwrap_or(0);
    // Full mode hides the editor, so its top line is stale: reloading the document the preview
    // already shows keeps the preview's own position there.
    let top_line = if mode == PreviewMode::Full && previous == Some(id) {
        view.top_line()
    } else {
        editor_top_line(&editor)
    };
    if length > LIVE_UPDATE_LIMIT && !force {
        view.set_paused(true);
        view.replace_document(PreviewDocument::default(), folder, started);
    } else if length > WORKER_PARSE_THRESHOLD {
        view.set_paused(length > LIVE_UPDATE_LIMIT);
        // Until the worker lands, show nothing rather than the previous document: its blocks,
        // links, and accessibility snapshot must never stand in for this one.
        view.replace_document(PreviewDocument::default(), folder, started);
        spawn_parse(hwnd, &editor, id, started, Some(top_line));
    } else {
        view.set_paused(false);
        view.replace_document(PreviewDocument::default(), folder, started);
        view.reparse(&ScintillaSource(&editor), started);
        view.scroll_to_line(top_line);
    }
}

fn editor_top_line(editor: &Editor) -> usize {
    editor
        .first_visible_line()
        .and_then(|line| editor.doc_line_from_visible(line))
        .unwrap_or(0)
}

/// A finished worker parse, posted to the main window as `WM_FASTPAD_PREVIEW_PARSED`.
pub(crate) struct ParsedPreview {
    document: DocumentId,
    generation: u64,
    parsed: PreviewDocument,
    started: Instant,
    /// Where a Full-mode preview goes when this parse lands; `None` keeps its current position.
    full_mode_line: Option<usize>,
}

/// Copies the text (the worker must not touch Scintilla's buffer) and parses it on a new thread.
/// Bumping the generation retires any parse still running.
fn spawn_parse(
    hwnd: HWND,
    editor: &Editor,
    document: DocumentId,
    started: Instant,
    full_mode_line: Option<usize>,
) -> bool {
    let Ok(text) = editor.text() else {
        return false;
    };
    let generation = with_host(hwnd, |host| {
        host.parse_generation += 1;
        host.full_parse_pending = true;
        host.parse_generation
    })
    .unwrap_or(0);
    let target = hwnd as isize;
    std::thread::spawn(move || {
        let parsed = PreviewDocument::parse(&text);
        drop(text);
        let payload = Box::into_raw(Box::new(ParsedPreview {
            document,
            generation,
            parsed,
            started,
            full_mode_line,
        }));
        if unsafe {
            PostMessageW(
                target as HWND,
                WM_FASTPAD_PREVIEW_PARSED,
                0,
                payload as isize,
            )
        } == 0
        {
            drop(unsafe { Box::from_raw(payload) });
        }
    });
    true
}

/// `WM_FASTPAD_PREVIEW_PARSED`: installs a worker parse unless newer text superseded it.
pub(crate) fn parsed(hwnd: HWND, lparam: LPARAM) {
    if lparam == 0 {
        return;
    }
    let payload = unsafe { Box::from_raw(lparam as *mut ParsedPreview) };
    let (current, edits_waiting) = with_host(hwnd, |host| {
        let current = host.view.is_some()
            && host.document == Some(payload.document)
            && host.parse_generation == payload.generation;
        if current {
            host.full_parse_pending = false;
        }
        (current, !host.edits.is_empty())
    })
    .unwrap_or((false, false));
    let (Some(view), true) = (view(hwnd), current) else {
        return;
    };
    // Edits made while the worker ran were deferred; they apply on top of the parsed snapshot.
    if edits_waiting {
        unsafe { SetTimer(hwnd, PREVIEW_TIMER_ID, PREVIEW_UPDATE_DELAY_MS, None) };
    }
    let folder = active_document(hwnd).and_then(|(_, _, folder)| folder);
    let ParsedPreview {
        parsed,
        started,
        full_mode_line,
        ..
    } = *payload;
    // Split follows the editor; Full mode hides the editor, so the preview keeps its own position.
    let line = if mode(hwnd) == PreviewMode::Full {
        Some(full_mode_line.unwrap_or_else(|| view.top_line()))
    } else {
        editor(hwnd).map(|editor| editor_top_line(&editor))
    };
    view.replace_document(parsed, folder, started);
    if let Some(line) = line {
        view.scroll_to_line(line);
    }
}

/// `SCN_MODIFIED`: O(1) bookkeeping only; the timer does the work.
pub(crate) fn record_edit(hwnd: HWND, notification: &ScintillaNotification) {
    let inserted = notification.modification_type as u32 & SC_MOD_INSERTTEXT != 0;
    let length = notification.length.max(0) as usize;
    let edit = Edit {
        position: notification.position.max(0) as usize,
        removed: if inserted { 0 } else { length },
        inserted: if inserted { length } else { 0 },
        lines_delta: notification.lines_added,
    };
    let recorded = with_host(hwnd, |host| {
        if host.view.is_none() || host.document.is_none() {
            return false;
        }
        host.edits.record(edit);
        true
    })
    .unwrap_or(false);
    if recorded {
        unsafe { SetTimer(hwnd, PREVIEW_TIMER_ID, PREVIEW_UPDATE_DELAY_MS, None) };
    }
}

/// What a debounced flush does with the pending edits.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FlushPlan {
    Nothing,
    /// A worker parse is still outstanding: keep these edits for when it lands. Spawning another
    /// would copy the whole text again on every typing pause.
    Defer(Pending),
    /// The document is over `LIVE_UPDATE_LIMIT`: show the refresh bar and drop the edits.
    Pause,
    /// Reparse everything on the UI thread.
    Reparse,
    /// Reparse everything on a worker.
    WorkerParse,
    /// Reparse around the edits; `allow_full_parse` lets that fall back to a full parse inline.
    Incremental {
        edits: Vec<Edit>,
        allow_full_parse: bool,
    },
}

/// A paused preview needs the whole document parsed again. While a worker parse is outstanding the
/// block model is stale, so edits wait for it instead of being applied on top.
pub(crate) fn plan_flush(
    pending: Pending,
    length: usize,
    paused: bool,
    full_parse_pending: bool,
) -> FlushPlan {
    if length > LIVE_UPDATE_LIMIT {
        return FlushPlan::Pause;
    }
    let pending = match pending {
        Pending::Nothing if !paused || full_parse_pending => return FlushPlan::Nothing,
        pending if full_parse_pending => return FlushPlan::Defer(pending),
        _ if paused => Pending::Full,
        pending => pending,
    };
    let large = length > WORKER_PARSE_THRESHOLD;
    match pending {
        Pending::Nothing => FlushPlan::Nothing,
        Pending::Full if large => FlushPlan::WorkerParse,
        Pending::Full => FlushPlan::Reparse,
        Pending::Edits(edits) => FlushPlan::Incremental {
            edits,
            allow_full_parse: !large,
        },
    }
}

/// `WM_TIMER` for `PREVIEW_TIMER_ID`: the typing pause has elapsed.
pub(crate) fn flush(hwnd: HWND) {
    unsafe { KillTimer(hwnd, PREVIEW_TIMER_ID) };
    if host_window::input_pending() {
        unsafe { SetTimer(hwnd, PREVIEW_TIMER_ID, PREVIEW_UPDATE_DELAY_MS, None) };
        return;
    }
    let (Some(view), Some(editor), Some((id, ..))) =
        (view(hwnd), editor(hwnd), active_document(hwnd))
    else {
        return;
    };
    let Some((pending, full_parse_pending)) = with_host(hwnd, |host| {
        (host.document == Some(id)).then(|| (host.edits.take(), host.full_parse_pending))
    })
    .flatten() else {
        return;
    };
    let length = editor.length().unwrap_or(0);
    let started = Instant::now();
    match plan_flush(pending, length, view.is_paused(), full_parse_pending) {
        FlushPlan::Nothing => {}
        FlushPlan::Defer(pending) => {
            with_host(hwnd, |host| host.edits.restore(pending));
            unsafe { SetTimer(hwnd, PREVIEW_TIMER_ID, PREVIEW_UPDATE_DELAY_MS, None) };
        }
        FlushPlan::Pause => view.set_paused(true),
        FlushPlan::Reparse => {
            view.set_paused(false);
            // Retire any worker parse still running: this parse is newer.
            with_host(hwnd, |host| {
                host.parse_generation += 1;
                host.full_parse_pending = false;
            });
            view.reparse(&ScintillaSource(&editor), started);
        }
        FlushPlan::WorkerParse => {
            view.set_paused(false);
            if !spawn_parse(hwnd, &editor, id, started, None) {
                // Keep the work for the next flush instead of dropping it.
                with_host(hwnd, |host| host.edits.request_full());
            }
        }
        FlushPlan::Incremental {
            edits,
            allow_full_parse,
        } => {
            let applied =
                view.apply_edits(&ScintillaSource(&editor), &edits, started, allow_full_parse);
            // `try_apply` declining has already shifted the model's ranges for the edits without
            // reparsing, so nothing may be applied incrementally on top of it: a full parse must
            // come first. The worker parse marks one pending (later edits wait for it); if it
            // cannot start, the edits survive as a full reparse request.
            if applied.is_none() && !spawn_parse(hwnd, &editor, id, started, None) {
                with_host(hwnd, |host| host.edits.request_full());
            }
        }
    }
}

/// `WM_FASTPAD_PREVIEW_REFRESH`: the paused bar was clicked.
pub(crate) fn refresh(hwnd: HWND) {
    load_active_document(hwnd, true);
}

/// A file finished loading into the active tab: its text replaced whatever the preview showed.
pub(crate) fn document_reloaded(hwnd: HWND) {
    with_host(hwnd, |host| host.document = None);
    sync_visibility(hwnd);
}

/// `WM_FASTPAD_PREVIEW_LINK`: a link was clicked or activated with Enter.
pub(crate) fn follow_link(hwnd: HWND, lparam: LPARAM) {
    if lparam == 0 {
        return;
    }
    let dest = *unsafe { Box::from_raw(lparam as *mut String) };
    let folder = active_document(hwnd).and_then(|(_, _, folder)| folder);
    match classify_link(&dest, folder.as_deref()) {
        LinkAction::External(url) => {
            if !shell_open(hwnd, &url) {
                host_window::push_notice(hwnd, format!("FastPad could not open {url}."));
            }
        }
        LinkAction::Anchor(anchor) => {
            if !view(hwnd).is_some_and(|view| view.scroll_to_anchor(&anchor)) {
                host_window::push_notice(
                    hwnd,
                    format!("No heading in this document matches #{anchor}."),
                );
            }
        }
        LinkAction::LocalFile(path) => {
            if let Err(error) = crate::window::open_path(hwnd, &path) {
                host_window::report_open_failure(hwnd, &path, &error);
            }
        }
        LinkAction::Ignored => {
            host_window::push_notice(
                hwnd,
                format!("FastPad does not open this kind of link: {dest}"),
            );
        }
    }
}

type ShellExecuteFn = unsafe extern "system" fn(
    HWND,
    *const u16,
    *const u16,
    *const u16,
    *const u16,
    i32,
) -> *mut core::ffi::c_void;

/// `ShellExecuteW`, resolved from shell32.dll on the first external link. A static import would
/// load shell32 into every launch. The module stays loaded for the life of the process, since
/// ShellExecute can leave work running inside it after returning.
fn shell_execute() -> Option<ShellExecuteFn> {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
    };
    static RESOLVED: OnceLock<Option<usize>> = OnceLock::new();
    let address = RESOLVED.get_or_init(|| {
        let name = wide_null("shell32.dll");
        let module = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if module.is_null() {
            return None;
        }
        unsafe { GetProcAddress(module, c"ShellExecuteW".as_ptr().cast()) }
            .map(|proc| proc as usize)
    });
    // SAFETY: the address is the `ShellExecuteW` export, whose signature `ShellExecuteFn` matches.
    address.map(|address| unsafe { std::mem::transmute::<usize, ShellExecuteFn>(address) })
}

fn shell_open(hwnd: HWND, url: &str) -> bool {
    let Some(execute) = shell_execute() else {
        return false;
    };
    let operation = wide_null("open");
    let target = wide_null(url);
    let result = unsafe {
        execute(
            hwnd,
            operation.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    result as isize > 32
}

/// `WM_FASTPAD_PREVIEW_HOVER`: the link under the pointer changed.
pub(crate) fn hover_link(hwnd: HWND, lparam: LPARAM) {
    if lparam == 0 {
        return;
    }
    let dest = *unsafe { Box::from_raw(lparam as *mut Option<String>) };
    with_host(hwnd, |host| host.hover_text = dest);
    host_window::invalidate_status_bar(hwnd);
}

/// `SCN_UPDATEUI` with a vertical scroll: move the preview to the editor's top line, unless this
/// scroll is the echo of a preview-initiated one.
pub(crate) fn editor_scrolled(hwnd: HWND) {
    // Taken before the mode check so a guard can never outlive the scroll that set it.
    let echo = with_host(hwnd, |host| {
        std::mem::take(&mut host.scroll_origin) == ScrollOrigin::Preview
    })
    .unwrap_or(false);
    if echo || mode(hwnd) != PreviewMode::Split || !preview_shown(hwnd) {
        return;
    }
    let (Some(view), Some(editor)) = (view(hwnd), editor(hwnd)) else {
        return;
    };
    if let Ok(line) = editor
        .first_visible_line()
        .and_then(|line| editor.doc_line_from_visible(line))
    {
        view.scroll_to_line(line);
        with_host(hwnd, |host| host.sync_count += 1);
    }
}

/// `WM_FASTPAD_PREVIEW_SCROLLED`: the user scrolled the preview; move the editor.
pub(crate) fn preview_scrolled(hwnd: HWND, line: usize) {
    if mode(hwnd) != PreviewMode::Split || !preview_shown(hwnd) {
        return;
    }
    let Some(editor) = editor(hwnd) else {
        return;
    };
    let Ok(target) = editor.visible_from_doc_line(line) else {
        return;
    };
    let before = editor.first_visible_line().ok();
    if before == Some(target) {
        return;
    }
    with_host(hwnd, |host| host.scroll_origin = ScrollOrigin::Preview);
    let _ = editor.set_first_visible_line(target);
    // Scintilla clamps near the end of the document; with no scroll there is no echo to clear the
    // guard, and it would swallow the next real editor scroll.
    let scrolled = editor.first_visible_line().ok() != before;
    with_host(hwnd, |host| {
        if scrolled {
            host.sync_count += 1;
        } else {
            host.scroll_origin = ScrollOrigin::None;
        }
    });
}

/// `WM_FASTPAD_DIAGNOSTIC_PREVIEW` (only under `--diagnostic`).
pub(crate) fn diagnostic(hwnd: HWND, selector: usize) -> isize {
    let view = view(hwnd);
    let stats = view.map(|view| view.stats()).unwrap_or_default();
    match selector {
        0 => match mode(hwnd) {
            PreviewMode::Off => 0,
            PreviewMode::Split => 1,
            PreviewMode::Full => 2,
        },
        1 => stats.block_count as isize,
        2 => stats.revision as isize,
        3 => stats.first_frame_micros as isize,
        4 => stats.last_update_micros as isize,
        5 => view.map_or(0, |view| view.top_line() as isize),
        6 => with_host(hwnd, |host| host.sync_count as isize).unwrap_or(0),
        7 => isize::from(preview_shown(hwnd)),
        _ => -1,
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
    if host_window::menu_mode(hwnd).is_some() || !over_divider(hwnd, x, y) {
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

/// `WM_CAPTURECHANGED`: capture was taken away mid-drag (task switch, a dialog), so no button-up
/// will arrive.
pub(crate) fn cancel_divider_drag(hwnd: HWND) {
    if with_host(hwnd, |host| std::mem::replace(&mut host.dragging, false)).unwrap_or(false)
        && let Some(view) = view(hwnd)
    {
        view.set_live_resize(false);
    }
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

    fn edit() -> Edit {
        Edit {
            position: 0,
            removed: 0,
            inserted: 1,
            lines_delta: 0,
        }
    }

    #[test]
    fn edits_wait_for_an_outstanding_worker_parse() {
        let small = 1_000;
        let large = WORKER_PARSE_THRESHOLD + 1;
        assert_eq!(
            plan_flush(Pending::Edits(vec![edit()]), large, false, true),
            FlushPlan::Defer(Pending::Edits(vec![edit()]))
        );
        assert_eq!(
            plan_flush(Pending::Full, large, false, true),
            FlushPlan::Defer(Pending::Full)
        );
        assert_eq!(
            plan_flush(Pending::Edits(vec![edit()]), small, true, true),
            FlushPlan::Defer(Pending::Edits(vec![edit()]))
        );
        assert_eq!(
            plan_flush(Pending::Nothing, large, false, true),
            FlushPlan::Nothing
        );
        assert_eq!(
            plan_flush(Pending::Edits(vec![edit()]), large, false, false),
            FlushPlan::Incremental {
                edits: vec![edit()],
                allow_full_parse: false
            }
        );
        assert_eq!(
            plan_flush(Pending::Edits(vec![edit()]), small, false, false),
            FlushPlan::Incremental {
                edits: vec![edit()],
                allow_full_parse: true
            }
        );
    }

    #[test]
    fn huge_documents_pause_and_a_paused_preview_parses_everything_again() {
        assert_eq!(
            plan_flush(
                Pending::Edits(vec![edit()]),
                LIVE_UPDATE_LIMIT + 1,
                false,
                false
            ),
            FlushPlan::Pause
        );
        assert_eq!(
            plan_flush(Pending::Nothing, 10, true, false),
            FlushPlan::Reparse
        );
        assert_eq!(
            plan_flush(
                Pending::Edits(vec![edit()]),
                WORKER_PARSE_THRESHOLD + 1,
                true,
                false
            ),
            FlushPlan::WorkerParse
        );
    }

    #[test]
    fn dragging_maps_the_pointer_to_a_clamped_ratio() {
        assert_eq!(ratio_for_x(AREA, 502, 96), 0.5);
        assert_eq!(ratio_for_x(AREA, 0, 96), 0.2);
        assert_eq!(ratio_for_x(AREA, 5000, 96), 0.8);
    }
}
