//! Ctrl+Shift+P command palette: a filter field over a list of every command the window can run,
//! overlaid at the top of the editor. A small painted panel hosts a native `Edit` and an
//! owner-drawn `ListBox` in the editor's palette colors; the main window owns showing, layout,
//! and running the chosen command.

use crate::platform::{last_error, wide_null};
use crate::window::commands::CommandId;
use crate::window::menus::{AcceleratorSpec, accelerator_specs};
use crate::window::palette::Palette;
use crate::window::panel::{create_child, create_panel, fill, inset, scale, text_height};
use std::rc::Rc;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DT_CALCRECT, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT,
    DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, HBRUSH, HDC, HFONT,
    InvalidateRect, PAINTSTRUCT, RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RedrawWindow,
    SelectObject, SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, EM_SETSEL, ODS_SELECTED, SetWindowTheme};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN,
    VK_SHIFT, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, ES_AUTOHSCROLL, GetClientRect, GetWindowTextLengthW, GetWindowTextW, HWND_TOP,
    LB_ADDSTRING, LB_GETCURSEL, LB_ITEMFROMPOINT, LB_RESETCONTENT, LB_SETCURSEL, LB_SETITEMHEIGHT,
    LBS_HASSTRINGS, LBS_NOINTEGRALHEIGHT, LBS_OWNERDRAWFIXED, MoveWindow, SW_HIDE, SW_SHOWNA,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SendMessageW, SetWindowPos, SetWindowTextW, ShowWindow,
    WM_CHAR, WM_GETFONT, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_NCDESTROY,
    WM_SETFOCUS, WM_SETFONT, WS_CHILD, WS_VISIBLE, WS_VSCROLL,
};

/// One runnable row: the command and what the palette calls it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PaletteEntry {
    pub command: CommandId,
    pub label: &'static str,
}

const fn entry(label: &'static str, command: CommandId) -> PaletteEntry {
    PaletteEntry { command, label }
}

/// Every command reachable from the palette, in the order an empty query lists them. `SelectTabN`
/// is positional and the palette itself is already open, so neither is listed.
pub(crate) const ENTRIES: [PaletteEntry; 46] = [
    entry("File: New tab", CommandId::New),
    entry("File: Open...", CommandId::Open),
    entry("File: Save", CommandId::Save),
    entry("File: Save as...", CommandId::SaveAs),
    entry("File: Close tab", CommandId::CloseTab),
    entry("File: Close all tabs", CommandId::CloseAllTabs),
    entry(
        "File: Toggle session restore",
        CommandId::ToggleRestoreSession,
    ),
    entry("Edit: Undo", CommandId::Undo),
    entry("Edit: Redo", CommandId::Redo),
    entry("Edit: Cut", CommandId::Cut),
    entry("Edit: Copy", CommandId::Copy),
    entry("Edit: Paste", CommandId::Paste),
    entry("Search: Find", CommandId::Find),
    entry("Search: Replace", CommandId::Replace),
    entry("JSON: Format document", CommandId::FormatJson),
    entry("JSON: Validate document", CommandId::ValidateJson),
    entry("Language: Plain text", CommandId::LanguagePlainText),
    entry("Language: JSON", CommandId::LanguageJson),
    entry("Language: Markdown", CommandId::LanguageMarkdown),
    entry(
        "Markdown Preview: Side by Side",
        CommandId::MarkdownPreviewSide,
    ),
    entry("Markdown Preview: Full", CommandId::MarkdownPreviewFull),
    entry("Close Markdown Preview", CommandId::MarkdownPreviewClose),
    entry("View: Next tab", CommandId::NextTab),
    entry("View: Previous tab", CommandId::PreviousTab),
    entry("View: Zoom in", CommandId::ZoomIn),
    entry("View: Zoom out", CommandId::ZoomOut),
    entry("View: Reset zoom", CommandId::ZoomReset),
    entry("View: Left-to-right text", CommandId::TextLeftToRight),
    entry("View: Right-to-left text", CommandId::TextRightToLeft),
    entry("View: Toggle word wrap", CommandId::ToggleWordWrap),
    entry("View: Toggle line numbers", CommandId::ToggleLineNumbers),
    entry("View: Increase font size", CommandId::FontSizeIncrease),
    entry("View: Decrease font size", CommandId::FontSizeDecrease),
    entry("View: Reset font size", CommandId::FontSizeReset),
    entry("Theme: System", CommandId::ThemeSystem),
    entry("Theme: Light", CommandId::ThemeLight),
    entry("Theme: Dark", CommandId::ThemeDark),
    entry(
        "Theme: Catppuccin (follow system)",
        CommandId::ThemeCatppuccin,
    ),
    entry("Theme: Catppuccin Latte", CommandId::ThemeCatppuccinLatte),
    entry("Theme: Catppuccin Frappe", CommandId::ThemeCatppuccinFrappe),
    entry(
        "Theme: Catppuccin Macchiato",
        CommandId::ThemeCatppuccinMacchiato,
    ),
    entry("Theme: Catppuccin Mocha", CommandId::ThemeCatppuccinMocha),
    entry("Editor: Tab width 2", CommandId::TabWidth2),
    entry("Editor: Tab width 4", CommandId::TabWidth4),
    entry("Editor: Tab width 8", CommandId::TabWidth8),
    entry("File: Exit", CommandId::Exit),
];

/// How well `query` matches `label`, lower is better; `None` when it does not match at all.
/// Case-insensitive and whitespace-insensitive in the query: a label prefix beats a word prefix,
/// which beats a substring, which beats the query's characters merely appearing in order.
fn match_rank(query: &str, label: &str) -> Option<u8> {
    let query = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if query.is_empty() {
        return Some(0);
    }
    let label = label.to_lowercase();
    let compact = label
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    if compact.starts_with(&query) {
        return Some(0);
    }
    // Each word, and the text after the category prefix, counts as a word start.
    let word_start = label
        .char_indices()
        .filter(|&(index, _)| {
            index == 0 || label[..index].ends_with(|c: char| c.is_whitespace() || c == ':')
        })
        .any(|(index, _)| {
            label[index..]
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .starts_with(&query)
        });
    if word_start {
        return Some(1);
    }
    if compact.contains(&query) {
        return Some(2);
    }
    let mut remaining = compact.chars();
    query
        .chars()
        .all(|wanted| remaining.any(|c| c == wanted))
        .then_some(3)
}

/// The commands `query` lists, best match first and in catalog order within a rank, skipping the
/// ones `available` rejects.
pub(crate) fn filter_entries(
    query: &str,
    available: impl Fn(CommandId) -> bool,
) -> Vec<PaletteEntry> {
    let mut ranked = ENTRIES
        .iter()
        .filter(|entry| available(entry.command))
        .filter_map(|entry| match_rank(query, entry.label).map(|rank| (rank, *entry)))
        .collect::<Vec<_>>();
    ranked.sort_by_key(|&(rank, _)| rank);
    ranked.into_iter().map(|(_, entry)| entry).collect()
}

/// The first keyboard shortcut bound to `command`, spelled the way the menus spell shortcuts.
pub(crate) fn shortcut_text(command: CommandId) -> Option<String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FALT, FCONTROL, FSHIFT};
    let spec: AcceleratorSpec = accelerator_specs()
        .into_iter()
        .find(|spec| spec.command == command)?;
    let mut text = String::new();
    for (flag, name) in [(FCONTROL, "Ctrl+"), (FSHIFT, "Shift+"), (FALT, "Alt+")] {
        if spec.modifiers & flag != 0 {
            text.push_str(name);
        }
    }
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_OEM_MINUS, VK_OEM_PLUS};
    match spec.key {
        VK_TAB => text.push_str("Tab"),
        VK_OEM_PLUS => text.push('+'),
        VK_OEM_MINUS => text.push('-'),
        key => text.push(char::from_u32(u32::from(key))?),
    }
    Some(text)
}

const WIDTH_AT_96_DPI: i32 = 560;
const PADDING_AT_96_DPI: i32 = 6;
const FIELD_HEIGHT_AT_96_DPI: i32 = 28;
const FIELD_TEXT_INSET_AT_96_DPI: i32 = 8;
const ROW_HEIGHT_AT_96_DPI: i32 = 26;
const VISIBLE_ROWS: usize = 12;
const MARGIN_AT_96_DPI: i32 = 8;

/// Where the panel's parts sit, in panel client coordinates.
#[derive(Clone, Copy)]
struct PanelLayout {
    width: i32,
    height: i32,
    /// The painted field box, border included.
    field: RECT,
    /// The borderless `Edit`, inset in the field and vertically centered on its text.
    edit: RECT,
    /// `None` while no command matches.
    list: Option<RECT>,
}

impl std::fmt::Debug for PanelLayout {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PanelLayout")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl PanelLayout {
    fn calculate(width: i32, dpi: u32, text_height: i32, rows: usize) -> Self {
        let padding = scale(PADDING_AT_96_DPI, dpi);
        let field_height = scale(FIELD_HEIGHT_AT_96_DPI, dpi);
        let inset = scale(FIELD_TEXT_INSET_AT_96_DPI, dpi);
        let field = RECT {
            left: padding,
            top: padding,
            right: (width - padding).max(padding),
            bottom: padding + field_height,
        };
        let text_height = text_height.clamp(1, field_height - 2);
        let edit_top = field.top + (field_height - text_height) / 2;
        let edit = RECT {
            left: field.left + inset,
            top: edit_top,
            right: (field.right - inset).max(field.left + inset),
            bottom: edit_top + text_height,
        };
        let rows = rows.min(VISIBLE_ROWS) as i32;
        let list = (rows > 0).then(|| RECT {
            left: 1,
            top: field.bottom + padding,
            right: (width - 1).max(1),
            bottom: field.bottom + padding + rows * scale(ROW_HEIGHT_AT_96_DPI, dpi),
        });
        // The list runs to the bottom border; the field alone keeps its padding below.
        let height = list.map_or(field.bottom + padding, |list| list.bottom + 1);
        Self {
            width,
            height,
            field,
            edit,
            list,
        }
    }
}

#[derive(Debug)]
pub(crate) struct CommandPalette {
    /// Paints the frame and field box, and owns the two controls.
    panel: HWND,
    query_edit: HWND,
    list: HWND,
    shown: Vec<PaletteEntry>,
    visible: bool,
    colors: Palette,
    layout: Option<PanelLayout>,
    field_brush: HBRUSH,
    list_brush: HBRUSH,
}

impl CommandPalette {
    pub(crate) fn create(parent: HWND) -> crate::Result<Self> {
        let panel = create_panel(parent)?;
        let controls = (|| {
            let query_edit = create_child(
                panel,
                &wide_null("Edit"),
                WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
            )?;
            let list = create_child(
                panel,
                &wide_null("ListBox"),
                WS_CHILD
                    | WS_VSCROLL
                    | (LBS_OWNERDRAWFIXED | LBS_HASSTRINGS | LBS_NOINTEGRALHEIGHT) as u32,
            )?;
            install_hook(query_edit, parent, PaletteControl::Query)?;
            install_hook(list, parent, PaletteControl::List)?;
            Ok((query_edit, list))
        })();
        let (query_edit, list) = match controls {
            Ok(controls) => controls,
            Err(error) => {
                unsafe {
                    DestroyWindow(panel);
                }
                return Err(error);
            }
        };
        let colors = Palette::neutral();
        Ok(Self {
            panel,
            query_edit,
            list,
            shown: Vec::new(),
            visible: false,
            colors,
            layout: None,
            field_brush: unsafe { CreateSolidBrush(colors.editor_background) },
            list_brush: unsafe { CreateSolidBrush(colors.strip_background) },
        })
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn owns(&self, hwnd: HWND) -> bool {
        hwnd == self.query_edit || hwnd == self.list || hwnd == self.panel
    }

    pub(crate) fn query_text(&self) -> String {
        let length = unsafe { GetWindowTextLengthW(self.query_edit) };
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied =
            unsafe { GetWindowTextW(self.query_edit, buffer.as_mut_ptr(), buffer.len() as i32) };
        buffer.truncate(copied.max(0) as usize);
        String::from_utf16_lossy(&buffer)
    }

    /// Marks the palette shown in `colors`; returns whether it was hidden before. Makes no calls
    /// that re-enter the window procedure, so the caller may hold the App borrow across it.
    pub(crate) fn mark_shown(&mut self, colors: Palette) -> bool {
        self.set_colors(colors);
        !std::mem::replace(&mut self.visible, true)
    }

    /// Recolors for a theme change; the caller repaints with `invalidate`.
    pub(crate) fn set_colors(&mut self, colors: Palette) {
        if colors == self.colors {
            return;
        }
        unsafe {
            DeleteObject(self.field_brush);
            DeleteObject(self.list_brush);
            self.field_brush = CreateSolidBrush(colors.editor_background);
            self.list_brush = CreateSolidBrush(colors.strip_background);
        }
        self.colors = colors;
    }

    pub(crate) fn invalidate(&self) {
        unsafe {
            RedrawWindow(
                self.panel,
                std::ptr::null(),
                std::ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
        }
    }

    /// Returns whether it was visible; `hide_controls` then removes it from the screen.
    pub(crate) fn mark_hidden(&mut self) -> bool {
        std::mem::take(&mut self.visible)
    }

    /// Sends `EN_CHANGE` to the parent.
    pub(crate) fn clear_query(&self) {
        let empty = wide_null("");
        unsafe {
            SetWindowTextW(self.query_edit, empty.as_ptr());
        }
    }

    pub(crate) fn hide_controls(&self) {
        unsafe {
            ShowWindow(self.panel, SW_HIDE);
        }
    }

    /// Records the rows to list; `fill_list` then puts them in the list box.
    pub(crate) fn set_entries(&mut self, entries: Vec<PaletteEntry>) {
        self.shown = entries;
    }

    /// Refills the list box from the recorded rows and selects the best match.
    pub(crate) fn fill_list(&self) {
        unsafe {
            SendMessageW(self.list, LB_RESETCONTENT, 0, 0);
        }
        for entry in &self.shown {
            let label = wide_null(entry.label);
            unsafe {
                SendMessageW(self.list, LB_ADDSTRING, 0, label.as_ptr() as LPARAM);
            }
        }
        if !self.shown.is_empty() {
            unsafe {
                SendMessageW(self.list, LB_SETCURSEL, 0, 0);
            }
        }
    }

    /// Records the panel geometry for `width` of parent client area, so painting and `apply_layout`
    /// agree on it.
    pub(crate) fn measure(&mut self, width: i32, dpi: u32, font: HFONT) {
        let margin = scale(MARGIN_AT_96_DPI, dpi);
        let width = scale(WIDTH_AT_96_DPI, dpi).min(width - 2 * margin).max(0);
        let text_height = text_height(self.query_edit, font);
        self.layout = Some(PanelLayout::calculate(
            width,
            dpi,
            text_height,
            self.shown.len(),
        ));
    }

    /// Centers the measured panel horizontally in `parent_width`, starting at `top`, and places
    /// the field and list inside it.
    pub(crate) fn apply_layout(&self, parent_width: i32, top: i32, dpi: u32, font: HFONT) {
        let Some(layout) = self.layout.filter(|_| self.visible) else {
            return;
        };
        let left = ((parent_width - layout.width) / 2).max(0);
        let top = top + scale(MARGIN_AT_96_DPI, dpi);
        let move_to = |hwnd, rect: RECT| unsafe {
            MoveWindow(
                hwnd,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                1,
            );
        };
        unsafe {
            if !font.is_null() {
                SendMessageW(self.query_edit, WM_SETFONT, font as WPARAM, 0);
                SendMessageW(self.list, WM_SETFONT, font as WPARAM, 0);
            }
            SendMessageW(
                self.list,
                LB_SETITEMHEIGHT,
                0,
                scale(ROW_HEIGHT_AT_96_DPI, dpi) as LPARAM,
            );
            // Dark scrollbar under the dark palettes, the system one otherwise.
            let dark_theme = wide_null("DarkMode_Explorer");
            SetWindowTheme(
                self.list,
                if self.colors.dark_frame {
                    dark_theme.as_ptr()
                } else {
                    std::ptr::null()
                },
                std::ptr::null(),
            );
            move_to(self.query_edit, layout.edit);
            match layout.list {
                Some(list) => {
                    move_to(self.list, list);
                    ShowWindow(self.list, SW_SHOWNA);
                }
                None => {
                    ShowWindow(self.list, SW_HIDE);
                }
            }
            // Above the editor in z-order; the editor clips itself against this sibling.
            SetWindowPos(
                self.panel,
                HWND_TOP,
                left,
                top,
                layout.width,
                layout.height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            InvalidateRect(self.panel, std::ptr::null(), 0);
        }
    }

    pub(crate) fn focus_query(&self) {
        unsafe {
            SetFocus(self.query_edit);
            SendMessageW(self.query_edit, EM_SETSEL, 0, -1);
        }
    }

    /// Moves the selection by `delta` rows, clamped to the list.
    pub(crate) fn move_selection(&self, delta: isize) {
        let Some(last) = self.shown.len().checked_sub(1) else {
            return;
        };
        let current = unsafe { SendMessageW(self.list, LB_GETCURSEL, 0, 0) }.max(0);
        let next = current.saturating_add(delta).clamp(0, last as isize);
        unsafe {
            SendMessageW(self.list, LB_SETCURSEL, next as WPARAM, 0);
        }
    }

    pub(crate) fn selected_command(&self) -> Option<CommandId> {
        let index = unsafe { SendMessageW(self.list, LB_GETCURSEL, 0, 0) };
        usize::try_from(index)
            .ok()
            .and_then(|index| self.shown.get(index))
            .map(|entry| entry.command)
    }

    /// Selects the row under a list-client point from `WM_LBUTTONDOWN`'s `lparam`.
    pub(crate) fn select_row_at(&self, lparam: LPARAM) -> bool {
        let hit = unsafe { SendMessageW(self.list, LB_ITEMFROMPOINT, 0, lparam) } as usize;
        // The high word is nonzero when the point lies outside every item.
        if (hit >> 16) & 0xffff != 0 || (hit & 0xffff) >= self.shown.len() {
            return false;
        }
        unsafe {
            SendMessageW(self.list, LB_SETCURSEL, hit & 0xffff, 0);
        }
        true
    }

    /// `WM_CTLCOLOREDIT`/`WM_CTLCOLORLISTBOX` for one of the palette's controls.
    pub(crate) fn control_color(&self, dc: HDC, control: HWND) -> HBRUSH {
        let (background, brush) = if control == self.query_edit {
            (self.colors.editor_background, self.field_brush)
        } else {
            (self.colors.strip_background, self.list_brush)
        };
        unsafe {
            SetTextColor(dc, self.colors.editor_foreground);
            SetBkColor(dc, background);
        }
        brush
    }

    /// `WM_PAINT` for the panel: the strip-colored card with a hairline border, and the field box
    /// in the editor's colors with an accent outline around the borderless `Edit`.
    pub(crate) fn paint_panel(&self, panel: HWND) {
        let mut paint = PAINTSTRUCT::default();
        let dc = unsafe { BeginPaint(panel, &mut paint) };
        if dc.is_null() {
            return;
        }
        let mut client = RECT::default();
        unsafe {
            GetClientRect(panel, &mut client);
        }
        let colors = self.colors;
        if let Some(layout) = self.layout {
            unsafe {
                fill(dc, client, colors.pressed_background);
                fill(dc, inset(client, 1), colors.strip_background);
                fill(dc, layout.field, colors.selection_background);
                fill(dc, inset(layout.field, 1), colors.editor_background);
            }
        }
        unsafe {
            EndPaint(panel, &paint);
        }
    }

    /// `WM_DRAWITEM` for the list: label on the left, shortcut right-aligned in the muted color.
    pub(crate) fn draw_item(&self, item: &DRAWITEMSTRUCT) {
        let Some(entry) = usize::try_from(item.itemID)
            .ok()
            .and_then(|index| self.shown.get(index))
        else {
            return;
        };
        let selected = item.itemState & ODS_SELECTED != 0;
        let colors = self.colors;
        let (background, foreground, muted) = if selected {
            (
                colors.hover_background,
                colors.hover_foreground,
                colors.hover_foreground,
            )
        } else {
            (
                colors.strip_background,
                colors.editor_foreground,
                colors.muted_foreground,
            )
        };
        let dc = item.hDC;
        // Row text lines up with the query text above it; the list starts one pixel in.
        let padding = self.layout.map_or(8, |layout| layout.edit.left - 1);
        let mut text = RECT {
            left: item.rcItem.left + padding,
            right: item.rcItem.right - padding,
            ..item.rcItem
        };
        unsafe {
            fill(dc, item.rcItem, background);
            SetBkMode(dc, TRANSPARENT as i32);
            let font = SendMessageW(self.list, WM_GETFONT, 0, 0);
            let previous = (font != 0).then(|| SelectObject(dc, font as _));
            let flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;
            if let Some(shortcut) = shortcut_text(entry.command) {
                let mut shortcut = shortcut.encode_utf16().collect::<Vec<_>>();
                SetTextColor(dc, muted);
                let mut measured = text;
                DrawTextW(
                    dc,
                    shortcut.as_mut_ptr(),
                    shortcut.len() as i32,
                    &mut measured,
                    flags | DT_RIGHT | DT_CALCRECT,
                );
                DrawTextW(
                    dc,
                    shortcut.as_mut_ptr(),
                    shortcut.len() as i32,
                    &mut text,
                    flags | DT_RIGHT,
                );
                // Keep the label clear of the shortcut column.
                text.right -= (measured.right - measured.left) + padding;
            }
            let mut label = entry.label.encode_utf16().collect::<Vec<_>>();
            SetTextColor(dc, foreground);
            DrawTextW(
                dc,
                label.as_mut_ptr(),
                label.len() as i32,
                &mut text,
                flags | DT_LEFT | DT_END_ELLIPSIS,
            );
            if let Some(previous) = previous {
                SelectObject(dc, previous);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn query_hwnd(&self) -> HWND {
        self.query_edit
    }

    #[cfg(test)]
    pub(crate) fn shown(&self) -> &[PaletteEntry] {
        &self.shown
    }

    #[cfg(test)]
    pub(crate) fn panel_hwnd(&self) -> HWND {
        self.panel
    }
}

impl Drop for CommandPalette {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.field_brush);
            DeleteObject(self.list_brush);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaletteControl {
    Query,
    List,
}

struct PaletteHook {
    parent: HWND,
    control: PaletteControl,
}

const PALETTE_HOOK_ID: usize = 0x4650_4350;

fn install_hook(hwnd: HWND, parent: HWND, control: PaletteControl) -> crate::Result<()> {
    let data = Rc::into_raw(Rc::new(PaletteHook { parent, control })) as usize;
    if unsafe { SetWindowSubclass(hwnd, Some(palette_control_proc), PALETTE_HOOK_ID, data) } == 0 {
        unsafe {
            drop(Rc::from_raw(data as *const PaletteHook));
        }
        return Err(last_error());
    }
    Ok(())
}

unsafe extern "system" fn palette_control_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> isize {
    let raw = ref_data as *const PaletteHook;
    unsafe {
        Rc::increment_strong_count(raw);
    }
    let hook = unsafe { Rc::from_raw(raw) };
    let parent = hook.parent;
    match (hook.control, message) {
        (_, WM_NCDESTROY) => unsafe {
            RemoveWindowSubclass(hwnd, Some(palette_control_proc), PALETTE_HOOK_ID);
            Rc::decrement_strong_count(raw);
        },
        (PaletteControl::Query, WM_KEYDOWN) => {
            let key = wparam as u16;
            let modified = [VK_CONTROL, VK_MENU, VK_SHIFT]
                .into_iter()
                .any(|modifier| unsafe { GetKeyState(i32::from(modifier)) } < 0);
            let step = match key {
                VK_UP => Some(-1),
                VK_DOWN => Some(1),
                VK_PRIOR => Some(-(VISIBLE_ROWS as isize)),
                VK_NEXT => Some(VISIBLE_ROWS as isize),
                _ => None,
            };
            if let Some(step) = step.filter(|_| !modified) {
                super::main_window::move_command_palette_selection(parent, step);
                return 0;
            }
            if key == VK_RETURN {
                super::main_window::run_command_palette_selection(parent);
                return 0;
            }
            if key == VK_ESCAPE {
                super::main_window::close_command_palette(parent, true);
                return 0;
            }
        }
        // A single-line Edit beeps at Enter and Escape characters; both were handled on key down.
        (PaletteControl::Query, WM_CHAR) if matches!(wparam as u16, 0x0d | 0x1b) => return 0,
        (PaletteControl::Query, WM_KILLFOCUS) => {
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            if !super::main_window::command_palette_owns(parent, wparam as HWND) {
                super::main_window::close_command_palette(parent, false);
            }
            return result;
        }
        // The list never keeps the focus: typing stays in the field while rows are clicked.
        (PaletteControl::List, WM_SETFOCUS) => {
            super::main_window::focus_command_palette(parent);
            return 0;
        }
        (PaletteControl::List, WM_LBUTTONDOWN | WM_LBUTTONDBLCLK) => {
            if super::main_window::select_command_palette_row(parent, lparam) {
                super::main_window::run_command_palette_selection(parent);
            }
            return 0;
        }
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::{ENTRIES, PanelLayout, filter_entries, match_rank, shortcut_text};
    use crate::window::commands::CommandId;

    fn labels(query: &str) -> Vec<&'static str> {
        filter_entries(query, |_| true)
            .into_iter()
            .map(|entry| entry.label)
            .collect()
    }

    #[test]
    fn every_command_except_tab_positions_and_the_palette_is_listed_once() {
        // Break caught: a command added to the menus and shortcuts that the palette never offers.
        for value in 100..200u16 {
            let Ok(command) = CommandId::try_from(value) else {
                continue;
            };
            let listed = ENTRIES
                .iter()
                .filter(|entry| entry.command == command)
                .count();
            let expected = usize::from(
                command.tab_index().is_none()
                    && command != CommandId::CommandPalette
                    && command != CommandId::MarkdownPreviewCycle,
            );
            assert_eq!(listed, expected, "{command:?}");
        }
    }

    #[test]
    fn an_empty_query_lists_every_available_command_in_catalog_order() {
        assert_eq!(labels("").len(), ENTRIES.len());
        assert_eq!(labels("   ")[0], ENTRIES[0].label);
        let without_documents = filter_entries("", |command| !command.needs_document());
        assert!(
            without_documents
                .iter()
                .all(|entry| !entry.command.needs_document())
        );
        assert!(
            without_documents
                .iter()
                .any(|entry| entry.command == CommandId::Open)
        );
        assert!(
            !without_documents
                .iter()
                .any(|entry| entry.command == CommandId::Save)
        );
    }

    #[test]
    fn matches_are_case_insensitive_and_ranked_prefix_word_substring_then_scattered() {
        assert_eq!(match_rank("FILE: s", "File: Save"), Some(0));
        assert_eq!(match_rank("zoom in", "View: Zoom in"), Some(1));
        assert_eq!(match_rank("oom", "View: Zoom in"), Some(2));
        assert_eq!(match_rank("vzi", "View: Zoom in"), Some(3));
        assert_eq!(match_rank("xyz", "View: Zoom in"), None);
        // Break caught: a scattered match outranking the command whose word the user typed.
        let found = labels("save");
        assert_eq!(found[..2], ["File: Save", "File: Save as..."]);
        assert_eq!(labels("json")[0], "JSON: Format document");
        assert_eq!(labels("zoom")[0], "View: Zoom in");
    }

    #[test]
    fn the_panel_centers_the_query_text_and_ends_with_the_list_on_its_bottom_border() {
        // Break caught: query text stuck to the top of its box, or a list that overhangs (or stops
        // short of) the panel's border.
        let layout = PanelLayout::calculate(560, 96, 16, 3);
        let field_middle = (layout.field.top + layout.field.bottom) / 2;
        let edit_middle = (layout.edit.top + layout.edit.bottom) / 2;
        assert!((field_middle - edit_middle).abs() <= 1);
        assert!(layout.edit.left > layout.field.left && layout.edit.right < layout.field.right);
        let list = layout.list.unwrap();
        assert_eq!(list.bottom - list.top, 3 * 26);
        assert_eq!((list.left, list.right), (1, 559));
        assert_eq!(layout.height, list.bottom + 1);

        let many = PanelLayout::calculate(560, 144, 24, 40);
        assert_eq!(many.list.map(|list| list.bottom - list.top), Some(12 * 39));

        let empty = PanelLayout::calculate(560, 96, 16, 0);
        assert!(empty.list.is_none());
        assert_eq!(empty.height, empty.field.bottom + 6);
    }

    #[test]
    fn shortcuts_are_spelled_from_the_accelerator_table() {
        assert_eq!(shortcut_text(CommandId::Save).as_deref(), Some("Ctrl+S"));
        assert_eq!(
            shortcut_text(CommandId::SaveAs).as_deref(),
            Some("Ctrl+Shift+S")
        );
        assert_eq!(
            shortcut_text(CommandId::NextTab).as_deref(),
            Some("Ctrl+Tab")
        );
        assert_eq!(shortcut_text(CommandId::ZoomIn).as_deref(), Some("Ctrl++"));
        assert_eq!(shortcut_text(CommandId::ZoomOut).as_deref(), Some("Ctrl+-"));
        assert_eq!(
            shortcut_text(CommandId::CommandPalette).as_deref(),
            Some("Ctrl+Shift+P")
        );
        assert_eq!(shortcut_text(CommandId::Copy), None);
    }

    #[test]
    fn markdown_preview_cycles_with_ctrl_shift_v_and_lists_three_palette_entries() {
        assert_eq!(
            shortcut_text(CommandId::MarkdownPreviewCycle).as_deref(),
            Some("Ctrl+Shift+V")
        );
        let labels = filter_entries("markdown preview", |_| true)
            .into_iter()
            .map(|entry| entry.command)
            .collect::<Vec<_>>();
        for command in [
            CommandId::MarkdownPreviewSide,
            CommandId::MarkdownPreviewFull,
            CommandId::MarkdownPreviewClose,
        ] {
            assert!(labels.contains(&command), "{command:?}");
        }
        assert!(
            filter_entries("markdown preview", |command| !command.is_markdown_preview()).is_empty()
        );
    }
}
