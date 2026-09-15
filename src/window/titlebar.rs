use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmDefWindowProc;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW,
    COLOR_WINDOWTEXT, DT_CENTER, DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, DrawTextW, EndPaint,
    FillRect, GetMonitorInfoW, GetSysColor, GetSysColorBrush, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromWindow, PAINTSTRUCT, ScreenToClient, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON,
    MINMAXINFO, NCCALCSIZE_PARAMS, SM_CXSIZE, SM_CYSIZE, SPI_GETHIGHCONTRAST,
    SystemParametersInfoW,
};

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
    NewTab,
    Overflow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TitleBarLayout {
    pub tabs: Rect,
    pub drag_region: Rect,
    pub minimize: Rect,
    pub maximize: Rect,
    pub close: Rect,
    pub new_tab: Rect,
    pub overflow: Rect,
    pub height: i32,
    tab_rects: Vec<Rect>,
    close_tab_rects: Vec<Rect>,
}

impl TitleBarLayout {
    pub fn calculate(client: Size, dpi: u32, tab_count: usize) -> Self {
        let width = client.width.max(0);
        let scale = |value: i32| ((i64::from(value) * i64::from(dpi.max(1)) + 48) / 96) as i32;
        let height = scale(40).max(unsafe { GetSystemMetricsForDpi(SM_CYSIZE, dpi.max(1)) });
        let caption_width = unsafe { GetSystemMetricsForDpi(SM_CXSIZE, dpi.max(1)) }
            .max(1)
            .min(width / 3);
        let close = Rect::new(width - caption_width, 0, width, height);
        let maximize = Rect::new(close.left - caption_width, 0, close.left, height);
        let minimize = Rect::new(maximize.left - caption_width, 0, maximize.left, height);

        let action_width = scale(40);
        let available_for_tabs_and_actions = minimize.left.max(0);
        let action_total = (action_width * 2).min(available_for_tabs_and_actions);
        let tabs_right = available_for_tabs_and_actions - action_total;
        let tabs = Rect::new(0, 0, tabs_right, height);

        let visible_tabs = tab_count.max(1);
        let preferred_tab_width = scale(180);
        let tab_width = if tab_count == 0 {
            0
        } else {
            (tabs_right / visible_tabs as i32).min(preferred_tab_width)
        };
        let mut tab_rects = Vec::with_capacity(tab_count);
        let mut close_tab_rects = Vec::with_capacity(tab_count);
        for index in 0..tab_count {
            let left = index as i32 * tab_width;
            let right = (left + tab_width).min(tabs.right);
            let tab = Rect::new(left, 0, right, height);
            let close_size = scale(24).min((right - left).max(0));
            tab_rects.push(tab);
            close_tab_rects.push(Rect::new(right - close_size, 0, right, height));
        }

        let occupied_tabs_right = tab_rects.last().map_or(0, |rect| rect.right);
        let new_tab = Rect::new(tabs_right, 0, tabs_right + action_total / 2, height);
        let overflow = Rect::new(new_tab.right, 0, available_for_tabs_and_actions, height);
        let drag_region = Rect::new(
            occupied_tabs_right,
            0,
            tabs_right.max(occupied_tabs_right),
            height,
        );

        Self {
            tabs,
            drag_region,
            minimize,
            maximize,
            close,
            new_tab,
            overflow,
            height,
            tab_rects,
            close_tab_rects,
        }
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
        if self.new_tab.contains(point) {
            return HitTarget::NewTab;
        }
        if self.overflow.contains(point) {
            return HitTarget::Overflow;
        }
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
        if self.drag_region.contains(point) {
            return HitTarget::Caption;
        }
        HitTarget::Client
    }

    pub fn tab(&self, index: usize) -> Rect {
        self.tab_rects[index]
    }
}

pub(crate) fn layout_for_window(hwnd: HWND, tab_count: usize) -> TitleBarLayout {
    let mut client = RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut client);
    }
    TitleBarLayout::calculate(
        Size::new(client.right - client.left, client.bottom - client.top),
        unsafe { GetDpiForWindow(hwnd) }.max(96),
        tab_count,
    )
}

pub(crate) unsafe fn paint(hwnd: HWND, titles: &[&str], active: usize, status: Option<&str>) {
    let mut paint = PAINTSTRUCT::default();
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    if dc.is_null() {
        return;
    }

    let layout = layout_for_window(hwnd, titles.len());
    let high_contrast = high_contrast_enabled();
    let strip_color = if high_contrast {
        COLOR_WINDOW
    } else {
        COLOR_BTNFACE
    };
    let strip = RECT {
        left: 0,
        top: 0,
        right: layout.close.right,
        bottom: layout.height,
    };
    unsafe {
        FillRect(dc, &strip, GetSysColorBrush(strip_color));
        SetBkMode(dc, TRANSPARENT as i32);
    }

    for (index, title) in titles.iter().enumerate() {
        let tab = layout.tab(index);
        let selected = index == active;
        let background = if selected {
            COLOR_HIGHLIGHT
        } else {
            strip_color
        };
        unsafe {
            FillRect(dc, &native_rect(tab), GetSysColorBrush(background));
            SetTextColor(
                dc,
                GetSysColor(if selected {
                    COLOR_HIGHLIGHTTEXT
                } else if high_contrast {
                    COLOR_WINDOWTEXT
                } else {
                    COLOR_BTNTEXT
                }),
            );
        }
        let close_width =
            ((24_i64 * i64::from(unsafe { GetDpiForWindow(hwnd) }.max(96)) + 48) / 96) as i32;
        let label = Rect::new(
            tab.left + 8,
            tab.top,
            (tab.right - close_width).max(tab.left),
            tab.bottom,
        );
        unsafe {
            draw_text(
                dc,
                title,
                label,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_END_ELLIPSIS,
            );
            draw_text(
                dc,
                "×",
                Rect::new(label.right, tab.top, tab.right, tab.bottom),
                DT_SINGLELINE | DT_VCENTER | DT_CENTER,
            );
        }
    }

    unsafe {
        SetTextColor(dc, GetSysColor(COLOR_BTNTEXT));
        draw_text(
            dc,
            "+",
            layout.new_tab,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
        draw_text(
            dc,
            "⋯",
            layout.overflow,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
        draw_text(
            dc,
            "—",
            layout.minimize,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
        draw_text(
            dc,
            "□",
            layout.maximize,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
        draw_text(
            dc,
            "×",
            layout.close,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
    }

    if let Some(status) = status {
        let mut client = RECT::default();
        unsafe {
            GetClientRect(hwnd, &mut client);
        }
        let height = crate::window::status::status_height(unsafe { GetDpiForWindow(hwnd) });
        let bar = RECT {
            left: 0,
            top: client.bottom - height,
            right: client.right,
            bottom: client.bottom,
        };
        unsafe {
            FillRect(dc, &bar, GetSysColorBrush(strip_color));
            SetTextColor(
                dc,
                GetSysColor(if high_contrast {
                    COLOR_WINDOWTEXT
                } else {
                    COLOR_BTNTEXT
                }),
            );
            draw_text(
                dc,
                status,
                Rect::new(8, bar.top, (bar.right - 8).max(8), bar.bottom),
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
    }

    unsafe {
        EndPaint(hwnd, &paint);
    }
}

pub(crate) unsafe fn nonclient_hit_test(
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
    tab_count: usize,
) -> LRESULT {
    let mut dwm_result = 0;
    if unsafe {
        DwmDefWindowProc(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_NCHITTEST,
            wparam,
            lparam,
            &mut dwm_result,
        )
    } != 0
    {
        return dwm_result;
    }

    let mut point = windows_sys::Win32::Foundation::POINT {
        x: (lparam as u32 & 0xffff) as u16 as i16 as i32,
        y: ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32,
    };
    unsafe {
        ScreenToClient(hwnd, &mut point);
    }
    let layout = layout_for_window(hwnd, tab_count);
    if point.x < 0 || point.x >= layout.close.right || point.y < 0 || point.y >= layout.height {
        return unsafe {
            DefWindowProcW(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_NCHITTEST,
                wparam,
                lparam,
            )
        };
    }
    match layout.hit_test(Point::new(point.x, point.y)) {
        HitTarget::Caption => HTCAPTION as LRESULT,
        HitTarget::Minimize => HTMINBUTTON as LRESULT,
        HitTarget::Maximize => HTMAXBUTTON as LRESULT,
        HitTarget::Close => HTCLOSE as LRESULT,
        HitTarget::Client
        | HitTarget::Tab(_)
        | HitTarget::CloseTab(_)
        | HitTarget::NewTab
        | HitTarget::Overflow => HTCLIENT as LRESULT,
    }
}

pub(crate) unsafe fn reclaim_caption(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if lparam == 0 {
        return unsafe {
            DefWindowProcW(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_NCCALCSIZE,
                wparam,
                lparam,
            )
        };
    }
    let client = if wparam == 0 {
        unsafe { &mut *(lparam as *mut RECT) }
    } else {
        unsafe { &mut (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0] }
    };
    let proposed_top = client.top;
    unsafe {
        DefWindowProcW(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_NCCALCSIZE,
            wparam,
            lparam,
        );
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let border = unsafe {
        GetSystemMetricsForDpi(windows_sys::Win32::UI::WindowsAndMessaging::SM_CYFRAME, dpi)
            + GetSystemMetricsForDpi(
                windows_sys::Win32::UI::WindowsAndMessaging::SM_CXPADDEDBORDER,
                dpi,
            )
    };
    client.top = proposed_top + border.max(1);
    0
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

fn high_contrast_enabled() -> bool {
    let mut contrast = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            (&mut contrast as *mut HIGHCONTRASTW).cast(),
            0,
        ) != 0
            && contrast.dwFlags & HCF_HIGHCONTRASTON != 0
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

unsafe fn draw_text(
    dc: windows_sys::Win32::Graphics::Gdi::HDC,
    text: &str,
    rect: Rect,
    format: u32,
) {
    let wide = text.encode_utf16().collect::<Vec<_>>();
    let mut rect = native_rect(rect);
    unsafe {
        DrawTextW(dc, wide.as_ptr(), wide.len() as i32, &mut rect, format);
    }
}

#[cfg(test)]
mod tests {
    use super::{HitTarget, Size, TitleBarLayout};

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
        assert_eq!(layout.hit_test(layout.new_tab.center()), HitTarget::NewTab);
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
}
