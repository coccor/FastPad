//! The Alt/F10 menu: a painted band of File/Edit/Search/View headings below the title strip. It
//! replaces a native menu bar, which Windows would draw unthemed over the reclaimed caption. The
//! band exists only while menu mode is active and pushes the find bar and editor down like any
//! other band; each heading opens its native dropdown from `menus::MenuBar`.

use crate::platform::wide_null;
use crate::window::palette::Palette;
use crate::window::panel::{fill, scale};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    DT_CALCRECT, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DrawTextW, GetDC, HDC, HFONT, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};

/// Headings in dropdown order; `&` marks each mnemonic, as in a native menu bar.
pub(crate) const MENU_TITLES: [&str; 4] = ["&File", "&Edit", "&Search", "&View"];
/// The index of the View heading, whose dropdown holds the Markdown preview commands.
pub(crate) const VIEW_MENU_INDEX: usize = 3;

const BAND_HEIGHT_AT_96_DPI: i32 = 28;
const HEADING_PADDING_AT_96_DPI: i32 = 10;
const BAND_MARGIN_AT_96_DPI: i32 = 4;

/// Menu mode: which heading is highlighted, and whether its dropdown is open.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct MenuMode {
    pub hot: usize,
    pub open: bool,
}

pub(crate) const fn band_height(dpi: u32) -> i32 {
    scale(BAND_HEIGHT_AT_96_DPI, dpi)
}

/// Heading rectangles for label widths `widths`, laid out left to right in a band at `top`.
pub(crate) fn heading_rects(widths: &[i32], top: i32, dpi: u32) -> Vec<RECT> {
    let padding = scale(HEADING_PADDING_AT_96_DPI, dpi);
    let mut left = scale(BAND_MARGIN_AT_96_DPI, dpi);
    widths
        .iter()
        .map(|width| {
            let right = left + width + 2 * padding;
            let rect = RECT {
                left,
                top,
                right,
                bottom: top + band_height(dpi),
            };
            left = right;
            rect
        })
        .collect()
}

pub(crate) fn heading_at(headings: &[RECT], x: i32, y: i32) -> Option<usize> {
    headings
        .iter()
        .position(|rect| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom)
}

/// The heading whose `&` mnemonic is `key` (an uppercase virtual-key letter or a typed character).
pub(crate) fn mnemonic_heading(key: u32) -> Option<usize> {
    let key = char::from_u32(key)?.to_ascii_uppercase();
    MENU_TITLES.iter().position(|title| {
        title
            .split_once('&')
            .and_then(|(_, rest)| rest.chars().next())
            .is_some_and(|mnemonic| mnemonic.to_ascii_uppercase() == key)
    })
}

/// The heading beside `current`, wrapping around both ends.
pub(crate) const fn neighbor(current: usize, forward: bool) -> usize {
    let count = MENU_TITLES.len();
    if forward {
        (current + 1) % count
    } else {
        (current + count - 1) % count
    }
}

/// Measures each title in `font` for `heading_rects`.
pub(crate) fn measure_titles(hwnd: HWND, font: HFONT) -> Vec<i32> {
    unsafe {
        let dc = GetDC(hwnd);
        if dc.is_null() {
            return vec![0; MENU_TITLES.len()];
        }
        let previous = (!font.is_null()).then(|| SelectObject(dc, font as _));
        let widths = MENU_TITLES
            .iter()
            .map(|title| {
                let mut text = wide_null(title);
                let mut rect = RECT::default();
                DrawTextW(
                    dc,
                    text.as_mut_ptr(),
                    -1,
                    &mut rect,
                    DT_CALCRECT | DT_SINGLELINE,
                );
                rect.right - rect.left
            })
            .collect();
        if let Some(previous) = previous {
            SelectObject(dc, previous);
        }
        ReleaseDC(hwnd, dc);
        widths
    }
}

/// Paints the band across `width` with `mode.hot` highlighted (pressed while its dropdown is open).
pub(crate) unsafe fn paint(
    dc: HDC,
    width: i32,
    headings: &[RECT],
    mode: MenuMode,
    palette: Palette,
    font: HFONT,
) {
    let Some(first) = headings.first() else {
        return;
    };
    let band = RECT {
        left: 0,
        top: first.top,
        right: width,
        bottom: first.bottom,
    };
    unsafe {
        fill(dc, band, palette.strip_background);
        SetBkMode(dc, TRANSPARENT as i32);
        let previous = (!font.is_null()).then(|| SelectObject(dc, font as _));
        for (index, (title, rect)) in MENU_TITLES.iter().zip(headings).enumerate() {
            let foreground = if index == mode.hot {
                let background = if mode.open {
                    palette.pressed_background
                } else {
                    palette.hover_background
                };
                fill(dc, *rect, background);
                palette.hover_foreground
            } else {
                palette.strip_foreground
            };
            SetTextColor(dc, foreground);
            let mut text = wide_null(title);
            let mut rect = *rect;
            // No DT_NOPREFIX: the mnemonic letters are underlined, as keyboard menu mode shows them.
            DrawTextW(
                dc,
                text.as_mut_ptr(),
                -1,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        }
        if let Some(previous) = previous {
            SelectObject(dc, previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_sit_side_by_side_inside_the_band() {
        let headings = heading_rects(&[20, 30, 40, 25], 32, 96);
        assert_eq!(headings.len(), 4);
        assert_eq!(headings[0].left, 4);
        assert_eq!(headings[0].right, 4 + 20 + 20);
        for pair in headings.windows(2) {
            assert_eq!(pair[0].right, pair[1].left);
        }
        assert!(
            headings
                .iter()
                .all(|rect| rect.top == 32 && rect.bottom == 32 + band_height(96))
        );
        assert_eq!(heading_rects(&[20], 0, 192)[0].right, 8 + 20 + 40);
    }

    #[test]
    fn hit_testing_finds_the_heading_under_the_pointer() {
        let headings = heading_rects(&[20, 30, 40, 25], 32, 96);
        assert_eq!(heading_at(&headings, 5, 33), Some(0));
        assert_eq!(heading_at(&headings, headings[1].left, 40), Some(1));
        assert_eq!(heading_at(&headings, 5, 31), None);
        assert_eq!(heading_at(&headings, headings[3].right, 40), None);
    }

    #[test]
    fn mnemonics_and_arrow_navigation_address_the_headings() {
        assert_eq!(mnemonic_heading(u32::from(b'F')), Some(0));
        assert_eq!(mnemonic_heading(u32::from(b'e')), Some(1));
        assert_eq!(mnemonic_heading(u32::from(b'S')), Some(2));
        assert_eq!(mnemonic_heading(u32::from(b'V')), Some(3));
        assert_eq!(mnemonic_heading(u32::from(b'Q')), None);
        assert_eq!(neighbor(0, false), 3);
        assert_eq!(neighbor(3, true), 0);
        assert_eq!(neighbor(1, true), 2);
    }
}
