pub const BENCHMARK_VERSION: u32 = 1;
pub const BENCHMARK_FRAME_LEN: usize = 4 + 2 * size_of::<u32>() + 10 * size_of::<u64>();

const BENCHMARK_MAGIC: &[u8; 4] = b"FPB1";
pub const MAPPING_HANDLE_ENV: &str = "FASTPAD_BENCH_MAPPING_HANDLE";
pub const EVENT_HANDLE_ENV: &str = "FASTPAD_BENCH_EVENT_HANDLE";
pub const QPC_ORIGIN_ENV: &str = "FASTPAD_BENCH_QPC_ORIGIN";
pub const BENCHMARK_INPUT_CHAR: usize = 0xE000;

#[derive(Default)]
pub struct BenchmarkInputState {
    accepted: bool,
    awaiting_modification: bool,
    paint_pending: bool,
    rendered: bool,
}

impl BenchmarkInputState {
    pub fn accept_char(&mut self, character: usize) -> bool {
        if character != BENCHMARK_INPUT_CHAR || self.accepted {
            return false;
        }
        self.accepted = true;
        self.awaiting_modification = true;
        true
    }

    pub fn note_text_modified(&mut self) {
        if self.awaiting_modification {
            self.awaiting_modification = false;
            self.paint_pending = true;
        }
    }

    pub fn finish_paint(&mut self) -> bool {
        if !self.paint_pending || self.rendered {
            return false;
        }
        self.paint_pending = false;
        self.rendered = true;
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticConfig {
    pub mapping_handle: usize,
    pub event_handle: usize,
    pub qpc_origin: i64,
}

pub fn read_diagnostic_config(
    diagnostic: bool,
    mut get: impl FnMut(&str) -> Option<String>,
) -> Result<Option<DiagnosticConfig>, &'static str> {
    if !diagnostic {
        return Ok(None);
    }

    let mapping_value = get(MAPPING_HANDLE_ENV);
    let event_value = get(EVENT_HANDLE_ENV);
    let origin_value = get(QPC_ORIGIN_ENV);
    if mapping_value.is_none() && event_value.is_none() && origin_value.is_none() {
        return Ok(None);
    }
    let mapping_handle = parse_usize(mapping_value, "missing mapping handle")?;
    let event_handle = parse_usize(event_value, "missing event handle")?;
    let qpc_origin = parse_i64(origin_value, "missing QPC origin")?;
    let config = DiagnosticConfig {
        mapping_handle,
        event_handle,
        qpc_origin,
    };
    if config.mapping_handle == 0 || config.event_handle == 0 {
        return Err("diagnostic handles must be nonzero");
    }
    if config.mapping_handle == config.event_handle {
        return Err("diagnostic handles must be distinct");
    }
    if config.qpc_origin <= 0 {
        return Err("diagnostic QPC origin must be positive");
    }
    Ok(Some(config))
}

pub fn validate_diagnostic_config(
    config: DiagnosticConfig,
    current_qpc: i64,
) -> Result<DiagnosticConfig, &'static str> {
    if current_qpc <= 0 || config.qpc_origin > current_qpc {
        return Err("diagnostic QPC origin is outside the valid range");
    }
    Ok(config)
}

fn parse_usize(value: Option<String>, missing: &'static str) -> Result<usize, &'static str> {
    value
        .ok_or(missing)?
        .parse()
        .map_err(|_| "diagnostic handle is not an unsigned integer")
}

fn parse_i64(value: Option<String>, missing: &'static str) -> Result<i64, &'static str> {
    value
        .ok_or(missing)?
        .parse()
        .map_err(|_| "diagnostic QPC origin is not an integer")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BenchmarkRecord {
    pub version: u32,
    pub pid: u32,
    pub process_start_us: u64,
    pub window_created_us: u64,
    pub editor_created_us: u64,
    pub first_paint_us: u64,
    pub first_input_accepted_us: u64,
    pub first_input_rendered_us: u64,
    pub settings_loaded_us: u64,
    pub file_loaded_us: u64,
    pub fully_ready_us: u64,
    pub idle_private_working_set_bytes: u64,
}

#[cfg(windows)]
pub struct DiagnosticSession {
    mapping: crate::platform::OwnedHandle,
    event: crate::platform::OwnedHandle,
    view: windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS,
    qpc_origin: i64,
    frequency: i64,
    record: std::cell::RefCell<BenchmarkRecord>,
    input: std::cell::RefCell<BenchmarkInputState>,
    editor_hwnd: std::cell::Cell<windows_sys::Win32::Foundation::HWND>,
    editor_destroyed: std::cell::Cell<bool>,
}

#[cfg(windows)]
impl std::fmt::Debug for DiagnosticSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DiagnosticSession")
            .field("mapping", &self.mapping)
            .field("event", &self.event)
            .field("qpc_origin", &self.qpc_origin)
            .finish_non_exhaustive()
    }
}

#[cfg(windows)]
impl DiagnosticSession {
    pub fn attach(diagnostic: bool) -> crate::Result<Option<std::rc::Rc<Self>>> {
        use crate::{FastPadError, platform::OwnedHandle};
        use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE};
        use windows_sys::Win32::System::Memory::{FILE_MAP_WRITE, MapViewOfFile};
        use windows_sys::Win32::System::Performance::{
            QueryPerformanceCounter, QueryPerformanceFrequency,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentProcessId, ResetEvent};

        let config = read_diagnostic_config(diagnostic, |name| std::env::var(name).ok())
            .map_err(FastPadError::Invariant)?;
        let Some(config) = config else {
            return Ok(None);
        };

        let mut current_qpc = 0_i64;
        if unsafe { QueryPerformanceCounter(&mut current_qpc) } == 0 {
            return Err(crate::platform::last_error());
        }
        let config =
            validate_diagnostic_config(config, current_qpc).map_err(FastPadError::Invariant)?;
        let mapping_raw = config.mapping_handle as HANDLE;
        let event_raw = config.event_handle as HANDLE;
        let mut flags = 0_u32;
        if unsafe { GetHandleInformation(mapping_raw, &mut flags) } == 0
            || unsafe { GetHandleInformation(event_raw, &mut flags) } == 0
        {
            return Err(crate::platform::last_error());
        }

        let mapping = unsafe { OwnedHandle::from_raw_owned(mapping_raw)? };
        let event = unsafe { OwnedHandle::from_raw_owned(event_raw)? };
        let view =
            unsafe { MapViewOfFile(mapping.as_raw(), FILE_MAP_WRITE, 0, 0, BENCHMARK_FRAME_LEN) };
        if view.Value.is_null() {
            return Err(crate::platform::last_error());
        }
        if unsafe { ResetEvent(event.as_raw()) } == 0 {
            unsafe {
                windows_sys::Win32::System::Memory::UnmapViewOfFile(view);
            }
            return Err(crate::platform::last_error());
        }

        let mut frequency = 0_i64;
        if unsafe { QueryPerformanceFrequency(&mut frequency) } == 0 || frequency <= 0 {
            unsafe {
                windows_sys::Win32::System::Memory::UnmapViewOfFile(view);
            }
            return Err(crate::platform::last_error());
        }
        let session = std::rc::Rc::new(Self {
            mapping,
            event,
            view,
            qpc_origin: config.qpc_origin,
            frequency,
            record: std::cell::RefCell::new(BenchmarkRecord::empty(unsafe {
                GetCurrentProcessId()
            })),
            input: std::cell::RefCell::new(BenchmarkInputState::default()),
            editor_hwnd: std::cell::Cell::new(std::ptr::null_mut()),
            editor_destroyed: std::cell::Cell::new(false),
        });
        session.publish();
        Ok(Some(session))
    }

    pub fn record_milestone(&self, milestone: crate::perf::Milestone, tick: i64) {
        if tick < self.qpc_origin {
            return;
        }
        let micros = ((tick - self.qpc_origin) as i128 * 1_000_000 / self.frequency as i128) as u64;
        self.record.borrow_mut().set_milestone(milestone, micros);
        self.publish();
    }

    pub(crate) fn install_input_hooks(
        self: &std::rc::Rc<Self>,
        parent: windows_sys::Win32::Foundation::HWND,
        editor: windows_sys::Win32::Foundation::HWND,
    ) -> crate::Result<()> {
        use windows_sys::Win32::UI::Shell::{RemoveWindowSubclass, SetWindowSubclass};

        self.editor_hwnd.set(editor);
        let child_data = std::rc::Rc::into_raw(std::rc::Rc::clone(self)) as usize;
        if unsafe {
            SetWindowSubclass(
                editor,
                Some(diagnostic_editor_subclass_proc),
                DIAGNOSTIC_EDITOR_SUBCLASS_ID,
                child_data,
            )
        } == 0
        {
            unsafe {
                drop(std::rc::Rc::from_raw(child_data as *const Self));
            }
            return Err(crate::platform::last_error());
        }

        let parent_data = std::rc::Rc::into_raw(std::rc::Rc::clone(self)) as usize;
        if unsafe {
            SetWindowSubclass(
                parent,
                Some(diagnostic_parent_subclass_proc),
                DIAGNOSTIC_PARENT_SUBCLASS_ID,
                parent_data,
            )
        } == 0
        {
            unsafe {
                RemoveWindowSubclass(
                    editor,
                    Some(diagnostic_editor_subclass_proc),
                    DIAGNOSTIC_EDITOR_SUBCLASS_ID,
                );
                drop(std::rc::Rc::from_raw(child_data as *const Self));
                drop(std::rc::Rc::from_raw(parent_data as *const Self));
            }
            return Err(crate::platform::last_error());
        }
        Ok(())
    }

    fn record_now(&self, milestone: crate::perf::Milestone) {
        let mut tick = 0_i64;
        if unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut tick) }
            != 0
        {
            self.record_milestone(milestone, tick);
        }
    }

    fn publish(&self) {
        let bytes = self.record.borrow().encode();
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.view.Value.cast::<u8>(),
                BENCHMARK_FRAME_LEN,
            );
        }
    }
}

#[cfg(windows)]
impl Drop for DiagnosticSession {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Memory::UnmapViewOfFile(self.view);
        }
    }
}

#[cfg(windows)]
const DIAGNOSTIC_EDITOR_SUBCLASS_ID: usize = 0x4650_4245;
#[cfg(windows)]
const DIAGNOSTIC_PARENT_SUBCLASS_ID: usize = 0x4650_4250;

#[cfg(windows)]
#[repr(C)]
struct ScintillaNotificationPrefix {
    header: windows_sys::Win32::UI::Controls::NMHDR,
    position: isize,
    character: i32,
    modifiers: i32,
    modification_type: i32,
}

#[cfg(windows)]
unsafe extern "system" fn diagnostic_editor_subclass_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> isize {
    use crate::perf::Milestone;
    use std::rc::Rc;
    use windows_sys::Win32::System::Threading::SetEvent;
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_CHAR, WM_NCDESTROY, WM_PAINT};

    let raw = ref_data as *const DiagnosticSession;
    unsafe { Rc::increment_strong_count(raw) };
    let session = unsafe { Rc::from_raw(raw) };

    if message == WM_NCDESTROY {
        session.editor_destroyed.set(true);
        unsafe {
            RemoveWindowSubclass(
                hwnd,
                Some(diagnostic_editor_subclass_proc),
                DIAGNOSTIC_EDITOR_SUBCLASS_ID,
            );
            Rc::decrement_strong_count(raw);
        }
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }

    if message == WM_CHAR && session.input.borrow_mut().accept_char(wparam) {
        session.record_now(Milestone::FirstInputAccepted);
    }
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if message == WM_PAINT
        && !session.editor_destroyed.get()
        && session.input.borrow_mut().finish_paint()
    {
        session.record_now(Milestone::FirstInputRendered);
        unsafe {
            SetEvent(session.event.as_raw());
        }
    }
    result
}

#[cfg(windows)]
unsafe extern "system" fn diagnostic_parent_subclass_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> isize {
    use std::rc::Rc;
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_NCDESTROY, WM_NOTIFY};

    let raw = ref_data as *const DiagnosticSession;
    unsafe { Rc::increment_strong_count(raw) };
    let session = unsafe { Rc::from_raw(raw) };
    if message == WM_NCDESTROY {
        unsafe {
            RemoveWindowSubclass(
                hwnd,
                Some(diagnostic_parent_subclass_proc),
                DIAGNOSTIC_PARENT_SUBCLASS_ID,
            );
            Rc::decrement_strong_count(raw);
        }
    } else if message == WM_NOTIFY && lparam != 0 {
        let notification = unsafe { &*(lparam as *const ScintillaNotificationPrefix) };
        let text_change = notification.modification_type
            & (crate::editor::scintilla_constants::SC_MOD_INSERTTEXT
                | crate::editor::scintilla_constants::SC_MOD_DELETETEXT) as i32
            != 0;
        if notification.header.hwndFrom == session.editor_hwnd.get()
            && notification.header.code == crate::editor::scintilla_constants::SCN_MODIFIED
            && text_change
        {
            session.input.borrow_mut().note_text_modified();
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

impl BenchmarkRecord {
    pub fn empty(pid: u32) -> Self {
        Self {
            version: BENCHMARK_VERSION,
            pid,
            process_start_us: 0,
            window_created_us: 0,
            editor_created_us: 0,
            first_paint_us: 0,
            first_input_accepted_us: 0,
            first_input_rendered_us: 0,
            settings_loaded_us: 0,
            file_loaded_us: 0,
            fully_ready_us: 0,
            idle_private_working_set_bytes: 0,
        }
    }

    pub fn set_milestone(&mut self, milestone: crate::perf::Milestone, micros: u64) {
        let slot = match milestone {
            crate::perf::Milestone::ProcessStart => &mut self.process_start_us,
            crate::perf::Milestone::WindowCreated => &mut self.window_created_us,
            crate::perf::Milestone::EditorCreated => &mut self.editor_created_us,
            crate::perf::Milestone::FirstPaint => &mut self.first_paint_us,
            crate::perf::Milestone::FirstInputAccepted => &mut self.first_input_accepted_us,
            crate::perf::Milestone::FirstInputRendered => &mut self.first_input_rendered_us,
            crate::perf::Milestone::SettingsLoaded => &mut self.settings_loaded_us,
            crate::perf::Milestone::FileLoaded => &mut self.file_loaded_us,
            crate::perf::Milestone::FullyReady => &mut self.fully_ready_us,
        };
        if *slot == 0 {
            *slot = micros;
        }
    }

    pub fn encode(self) -> [u8; BENCHMARK_FRAME_LEN] {
        let mut bytes = [0_u8; BENCHMARK_FRAME_LEN];
        bytes[..4].copy_from_slice(BENCHMARK_MAGIC);
        let mut offset = 4;
        write_u32(&mut bytes, &mut offset, self.version);
        write_u32(&mut bytes, &mut offset, self.pid);
        for value in [
            self.process_start_us,
            self.window_created_us,
            self.editor_created_us,
            self.first_paint_us,
            self.first_input_accepted_us,
            self.first_input_rendered_us,
            self.settings_loaded_us,
            self.file_loaded_us,
            self.fully_ready_us,
            self.idle_private_working_set_bytes,
        ] {
            write_u64(&mut bytes, &mut offset, value);
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() != BENCHMARK_FRAME_LEN {
            return Err("benchmark frame has the wrong length");
        }
        if &bytes[..4] != BENCHMARK_MAGIC {
            return Err("benchmark frame has the wrong magic");
        }

        let mut offset = 4;
        let version = read_u32(bytes, &mut offset);
        if version != BENCHMARK_VERSION {
            return Err("benchmark frame has an unsupported version");
        }
        Ok(Self {
            version,
            pid: read_u32(bytes, &mut offset),
            process_start_us: read_u64(bytes, &mut offset),
            window_created_us: read_u64(bytes, &mut offset),
            editor_created_us: read_u64(bytes, &mut offset),
            first_paint_us: read_u64(bytes, &mut offset),
            first_input_accepted_us: read_u64(bytes, &mut offset),
            first_input_rendered_us: read_u64(bytes, &mut offset),
            settings_loaded_us: read_u64(bytes, &mut offset),
            file_loaded_us: read_u64(bytes, &mut offset),
            fully_ready_us: read_u64(bytes, &mut offset),
            idle_private_working_set_bytes: read_u64(bytes, &mut offset),
        })
    }

    #[cfg(test)]
    fn sample() -> Self {
        Self {
            version: BENCHMARK_VERSION,
            pid: 4_242,
            process_start_us: 1,
            window_created_us: 2,
            editor_created_us: 3,
            first_paint_us: 4,
            first_input_accepted_us: 5,
            first_input_rendered_us: 6,
            settings_loaded_us: 7,
            file_loaded_us: 8,
            fully_ready_us: 9,
            idle_private_working_set_bytes: 10,
        }
    }
}

fn write_u32(bytes: &mut [u8], offset: &mut usize, value: u32) {
    bytes[*offset..*offset + size_of::<u32>()].copy_from_slice(&value.to_le_bytes());
    *offset += size_of::<u32>();
}

fn write_u64(bytes: &mut [u8], offset: &mut usize, value: u64) {
    bytes[*offset..*offset + size_of::<u64>()].copy_from_slice(&value.to_le_bytes());
    *offset += size_of::<u64>();
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> u32 {
    let value = u32::from_le_bytes(
        bytes[*offset..*offset + size_of::<u32>()]
            .try_into()
            .unwrap(),
    );
    *offset += size_of::<u32>();
    value
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> u64 {
    let value = u64::from_le_bytes(
        bytes[*offset..*offset + size_of::<u64>()]
            .try_into()
            .unwrap(),
    );
    *offset += size_of::<u64>();
    value
}

#[cfg(test)]
mod tests {
    use super::{
        BENCHMARK_INPUT_CHAR, BenchmarkInputState, BenchmarkRecord, read_diagnostic_config,
    };
    use std::cell::Cell;

    #[test]
    fn metric_frame_has_fixed_versioned_layout() {
        // Break caught: changing the diagnostic frame magic or field ordering makes the harness
        // decode child-process metrics incorrectly.
        let record = BenchmarkRecord::sample();
        let bytes = record.encode();
        assert_eq!(&bytes[..4], b"FPB1");
        assert_eq!(BenchmarkRecord::decode(&bytes).unwrap(), record);
    }

    #[test]
    fn truncated_frame_is_rejected() {
        // Break caught: accepting a partial inherited mapping can turn absent milestone bytes into
        // plausible benchmark measurements.
        assert!(BenchmarkRecord::decode(b"FPB1").is_err());
    }

    #[test]
    fn normal_launch_does_not_read_diagnostic_environment() {
        // Break caught: consulting benchmark environment variables during ordinary startup adds
        // diagnostic work and lets stale inherited values affect a normal launch.
        let reads = Cell::new(0);
        let config = read_diagnostic_config(false, |_| {
            reads.set(reads.get() + 1);
            None
        })
        .unwrap();

        assert_eq!(config, None);
        assert_eq!(reads.get(), 0);
    }

    #[test]
    fn diagnostic_launch_without_benchmark_transport_remains_available() {
        // Break caught: making absent harness variables fatal breaks ordinary --diagnostic
        // launches and the existing editable-startup smoke contract.
        assert_eq!(read_diagnostic_config(true, |_| None).unwrap(), None);
    }

    #[test]
    fn diagnostic_environment_rejects_zero_handles_and_future_origins() {
        // Break caught: attaching unchecked inherited values can map an invalid handle or produce
        // wrapped milestone durations from an origin later than the child counter.
        let value = |name: &str| match name {
            "FASTPAD_BENCH_MAPPING_HANDLE" => Some("0".to_owned()),
            "FASTPAD_BENCH_EVENT_HANDLE" => Some("12".to_owned()),
            "FASTPAD_BENCH_QPC_ORIGIN" => Some("999".to_owned()),
            _ => None,
        };
        assert!(read_diagnostic_config(true, value).is_err());

        let value = |name: &str| match name {
            "FASTPAD_BENCH_MAPPING_HANDLE" => Some("11".to_owned()),
            "FASTPAD_BENCH_EVENT_HANDLE" => Some("12".to_owned()),
            "FASTPAD_BENCH_QPC_ORIGIN" => Some("999".to_owned()),
            _ => None,
        };
        assert!(
            super::validate_diagnostic_config(
                read_diagnostic_config(true, value).unwrap().unwrap(),
                998,
            )
            .is_err()
        );
    }

    #[test]
    fn rendered_input_requires_the_benchmark_char_modification_and_following_paint() {
        // Break caught: treating an unrelated character or a paint before Scintilla's text-change
        // notification as rendered input understates interactive latency.
        let mut state = BenchmarkInputState::default();

        assert!(!state.accept_char('X' as usize));
        assert!(!state.finish_paint());
        assert!(state.accept_char(BENCHMARK_INPUT_CHAR));
        assert!(!state.finish_paint());
        state.note_text_modified();
        assert!(state.finish_paint());
        assert!(!state.accept_char(BENCHMARK_INPUT_CHAR));
        assert!(!state.finish_paint());
    }
}
