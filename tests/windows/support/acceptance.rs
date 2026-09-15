use super::process::{
    FastPadProcess, process_has_module_loaded, wait_and_dismiss_dialog, wait_for_process_exit,
};
use super::win32::{Deadline, find_child_by_class, scintilla_text};
use fastpad::perf::protocol::{
    BENCHMARK_INPUT_CHAR, BENCHMARK_SHARED_FRAME_LEN, BenchmarkRecord, EVENT_HANDLE_ENV,
    MAPPING_HANDLE_ENV, QPC_ORIGIN_ENV, read_shared_record,
};
use fastpad::platform::OwnedHandle;
use fastpad::window::commands::CommandId;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    HANDLE, HWND, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Performance::QueryPerformanceCounter;
use windows_sys::Win32::System::ProcessStatus::EnumProcesses;
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PostMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_CHAR, WM_CLOSE, WM_COMMAND,
};

const LEXILLA_DLL: &str = "Lexilla.dll";
const NETWORK_IMPORTS: [&str; 4] = ["ws2_32.dll", "winhttp.dll", "wininet.dll", "urlmon.dll"];
const BROWSER_MODULES: [&str; 10] = [
    "WebView2Loader.dll",
    "EmbeddedBrowserWebView.dll",
    "mshtml.dll",
    "ieframe.dll",
    "edgehtml.dll",
    "jscript.dll",
    "jscript9.dll",
    "jscript9Legacy.dll",
    "chakra.dll",
    "urlmon.dll",
];
const MALFORMED_JSON: &str = "{\"broken\": [1, 2";

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

/// Product acceptance scenarios. Each instance owns one temporary directory (used as the
/// LocalAppData override and for fixtures) and removes only that directory when dropped.
pub struct AcceptanceHarness {
    root: PathBuf,
}

impl AcceptanceHarness {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "fastpad-acceptance-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("FastPad")).unwrap();
        Self { root }
    }

    pub fn empty_launch_order(&self) {
        let mut launch = MeasuredLaunch::start(&self.root, &[]).unwrap();
        let record = launch.type_until_fully_ready().unwrap();

        assert_startup_order(&record, launch.process.id());
        assert!(
            record.first_paint_us <= record.settings_loaded_us,
            "optional settings work ran before first paint: {record:?}"
        );
        assert!(!process_has_module_loaded(launch.process.id(), LEXILLA_DLL).unwrap());
        assert!(!launch.process.has_dialog().unwrap());
        launch.close_discarding_changes();
    }

    pub fn json_launch_order(&self) {
        ensure_lexilla_beside_fastpad();
        let file = self.fixture("launch.json", "{\"ok\": true}");
        let mut launch = MeasuredLaunch::start(&self.root, &[file.as_os_str()]).unwrap();

        // The launch open waits for first input, so settings finish while the file is still unread.
        let pending = launch
            .wait_for_record(|record| record.settings_loaded_us != 0)
            .unwrap();
        assert!(pending.window_created_us != 0 && pending.first_paint_us != 0);
        assert_eq!(pending.file_loaded_us, 0, "file loaded before first input");
        assert_eq!(scintilla_text(launch.editor).unwrap(), "");
        assert!(!process_has_module_loaded(launch.process.id(), LEXILLA_DLL).unwrap());

        let record = launch.type_until_fully_ready().unwrap();
        assert_startup_order(&record, launch.process.id());
        assert!(
            record.window_created_us < record.file_loaded_us
                && record.first_paint_us < record.file_loaded_us
                && record.first_input_accepted_us < record.file_loaded_us,
            "window, paint, and input must precede the file load: {record:?}"
        );
        wait_until("Lexilla activation for the JSON file", || {
            process_has_module_loaded(launch.process.id(), LEXILLA_DLL).unwrap_or(false)
        });
        launch.close_discarding_changes();
    }

    /// Evidence: a malformed JSON file is opened by the launch and the whole deferred chain reaches
    /// FullyReady without any JSON report; the explicit Validate JSON command then reports the same
    /// document, proving the observation would have caught a startup parse.
    pub fn assert_no_startup_json_parse(&self) {
        let file = self.fixture("malformed.json", MALFORMED_JSON);
        let mut launch = MeasuredLaunch::start(&self.root, &[file.as_os_str()]).unwrap();
        let record = launch.type_until_fully_ready().unwrap();
        assert_startup_order(&record, launch.process.id());
        wait_until("the malformed JSON file in the editor", || {
            scintilla_text(launch.editor).is_ok_and(|text| text == MALFORMED_JSON)
        });
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            !launch.process.has_dialog().unwrap(),
            "startup reported a JSON parse result"
        );

        unsafe {
            PostMessageW(launch.hwnd, WM_COMMAND, CommandId::ValidateJson as usize, 0);
        }
        wait_and_dismiss_dialog(launch.process.id(), Duration::from_secs(3))
            .expect("the explicit Validate JSON command did not report the malformed document");
        assert_eq!(scintilla_text(launch.editor).unwrap(), MALFORMED_JSON);
        launch.close_discarding_changes();
    }

    pub fn assert_no_browser_module(&self) {
        ensure_lexilla_beside_fastpad();
        let file = self.fixture(
            "notes.md",
            "# Notes\n\n*hello* [link](https://example.com)\n",
        );
        let mut process = FastPadProcess::spawn_with_local_app_data(
            [std::ffi::OsStr::new("--new-window"), file.as_os_str()],
            &self.root,
        )
        .unwrap();
        let hwnd = process
            .wait_for_main_window(Duration::from_secs(5))
            .unwrap();
        let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
        post_char(editor, u16::from(b'x'));
        wait_until("the Markdown file in the editor", || {
            scintilla_text(editor).is_ok_and(|text| text.contains("hello"))
        });
        wait_until("Lexilla activation for Markdown", || {
            process_has_module_loaded(process.id(), LEXILLA_DLL).unwrap_or(false)
        });
        std::thread::sleep(Duration::from_millis(500));

        for module in BROWSER_MODULES {
            assert!(
                !process_has_module_loaded(process.id(), module).unwrap(),
                "Markdown loaded browser runtime module {module}"
            );
        }
        close_discarding_changes(process, hwnd);
    }

    pub fn corrupt_state_order(&self) {
        let data = self.root.join("FastPad");
        std::fs::write(
            data.join("fastpad.ini"),
            b"tab_width=banana\n\xFF\xFE\x00garbage\nfont_size=\n[section\n",
        )
        .unwrap();
        let recovery = data.join("Recovery");
        std::fs::create_dir_all(&recovery).unwrap();
        let malformed = recovery.join("ffffffffffffffffffffffffffffffff.fps");
        std::fs::write(&malformed, b"FPS1\x01torn").unwrap();

        let mut launch = MeasuredLaunch::start(&self.root, &[]).unwrap();
        let record = launch.type_until_fully_ready().unwrap();
        assert_startup_order(&record, launch.process.id());
        assert!(
            record.first_paint_us <= record.settings_loaded_us,
            "corrupt settings were read before first paint: {record:?}"
        );
        let mut quarantined = malformed.as_os_str().to_owned();
        quarantined.push(".invalid");
        wait_until("the malformed snapshot quarantine", || {
            !malformed.exists() && Path::new(&quarantined).exists()
        });
        assert!(
            !launch.process.has_dialog().unwrap(),
            "corrupt state produced a modal dialog"
        );
        launch.close_discarding_changes();
    }

    pub fn assert_no_network_imports(&self) {
        let dumpbin = locate_dumpbin();
        let binary = Path::new(env!("CARGO_BIN_EXE_fastpad"));
        let output = run_bounded(
            Command::new(&dumpbin)
                .arg("/nologo")
                .arg("/imports")
                .arg(binary),
            Duration::from_secs(120),
        )
        .unwrap_or_else(|error| panic!("dumpbin /imports {} failed: {error}", binary.display()));
        let imports = imported_dlls(&output);
        assert!(
            imports.iter().any(|name| name == "kernel32.dll"),
            "dumpbin output had no KERNEL32 import; parsing failed:\n{output}"
        );
        for forbidden in NETWORK_IMPORTS {
            assert!(
                !imports.iter().any(|name| name == forbidden),
                "{} imports network library {forbidden}: {imports:?}",
                binary.display()
            );
        }
    }

    pub fn assert_no_resident_process(&self) {
        let fastpad_image = Path::new(env!("CARGO_BIN_EXE_fastpad"));
        assert!(
            running_fastpad_processes(fastpad_image).is_empty(),
            "another FastPad process is already running; the default launch would forward to it"
        );

        let mut first =
            FastPadProcess::spawn_with_local_app_data(std::iter::empty::<&str>(), &self.root)
                .unwrap();
        let first_id = first.id();
        let hwnd = first.wait_for_main_window(Duration::from_secs(5)).unwrap();
        find_child_by_class(hwnd, "Scintilla").unwrap();
        first.close().unwrap();
        wait_for_process_exit(first_id, Duration::from_secs(5)).unwrap();
        wait_until("every FastPad process to exit after close", || {
            running_fastpad_processes(fastpad_image).is_empty()
        });

        let mut second =
            FastPadProcess::spawn_with_local_app_data(std::iter::empty::<&str>(), &self.root)
                .unwrap();
        assert_ne!(second.id(), first_id);
        let hwnd = second
            .wait_for_main_window(Duration::from_secs(5))
            .expect("a later default launch did not become a new primary window");
        find_child_by_class(hwnd, "Scintilla").unwrap();
        second.close().unwrap();
        wait_until("the second FastPad process to exit", || {
            running_fastpad_processes(fastpad_image).is_empty()
        });
    }

    fn fixture(&self, name: &str, text: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for AcceptanceHarness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A `--new-window --diagnostic` launch wired to the FPB1 shared-memory transport the benchmark
/// harness uses, so milestone order comes from the same frame FastPad publishes for fastpad-bench.
struct MeasuredLaunch {
    transport: DiagnosticTransport,
    process: FastPadProcess,
    hwnd: HWND,
    editor: HWND,
}

impl MeasuredLaunch {
    fn start(local_app_data: &Path, extra_args: &[&std::ffi::OsStr]) -> TestResult<Self> {
        let transport = DiagnosticTransport::create()?;
        let mut args = vec![
            std::ffi::OsStr::new("--new-window"),
            std::ffi::OsStr::new("--diagnostic"),
        ];
        args.extend_from_slice(extra_args);
        let environment = transport.environment()?;
        let mut process =
            FastPadProcess::spawn_with_environment(args, local_app_data, &environment)?;
        let hwnd = process.wait_for_main_window(Duration::from_secs(5))?;
        let editor = find_child_by_class(hwnd, "Scintilla")?;
        Ok(Self {
            transport,
            process,
            hwnd,
            editor,
        })
    }

    fn type_until_fully_ready(&mut self) -> TestResult<BenchmarkRecord> {
        let mut result = 0_usize;
        if unsafe {
            SendMessageTimeoutW(
                self.editor,
                WM_CHAR,
                BENCHMARK_INPUT_CHAR,
                1,
                SMTO_ABORTIFHUNG,
                5_000,
                &mut result,
            )
        } == 0
        {
            return Err("timed out delivering the first input character".into());
        }
        self.wait_for_rendered_input()?;
        self.wait_for_record(|record| record.fully_ready_us != 0)
    }

    fn wait_for_rendered_input(&mut self) -> TestResult<()> {
        let deadline = Deadline::after(Duration::from_secs(10));
        loop {
            match unsafe { WaitForSingleObject(self.transport.event.as_raw(), 10) } {
                WAIT_OBJECT_0 => return Ok(()),
                WAIT_TIMEOUT => {}
                _ => return Err(Box::new(fastpad::platform::last_error())),
            }
            if deadline.expired() {
                return Err("timed out waiting for the rendered-input event".into());
            }
        }
    }

    fn wait_for_record(
        &self,
        mut ready: impl FnMut(&BenchmarkRecord) -> bool,
    ) -> TestResult<BenchmarkRecord> {
        let deadline = Deadline::after(Duration::from_secs(10));
        loop {
            if let Some(record) = self.transport.read()?
                && ready(&record)
            {
                return Ok(record);
            }
            if deadline.expired() {
                return Err(format!(
                    "timed out waiting for a diagnostic milestone: {:?}",
                    self.transport.read()?
                )
                .into());
            }
            deadline.sleep_step();
        }
    }

    fn close_discarding_changes(self) {
        let Self {
            transport,
            process,
            hwnd,
            ..
        } = self;
        close_discarding_changes(process, hwnd);
        drop(transport);
    }
}

struct DiagnosticTransport {
    mapping: OwnedHandle,
    event: OwnedHandle,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl DiagnosticTransport {
    fn create() -> TestResult<Self> {
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        let mapping_raw = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                &security,
                PAGE_READWRITE,
                0,
                BENCHMARK_SHARED_FRAME_LEN as u32,
                std::ptr::null(),
            )
        };
        let mapping = unsafe { OwnedHandle::from_raw_owned(mapping_raw) }?;
        let event_raw = unsafe { CreateEventW(&security, 0, 0, std::ptr::null()) };
        let event = unsafe { OwnedHandle::from_raw_owned(event_raw) }?;
        let view = unsafe {
            MapViewOfFile(
                mapping.as_raw(),
                FILE_MAP_READ | FILE_MAP_WRITE,
                0,
                0,
                BENCHMARK_SHARED_FRAME_LEN,
            )
        };
        if view.Value.is_null() {
            return Err(Box::new(fastpad::platform::last_error()));
        }
        Ok(Self {
            mapping,
            event,
            view,
        })
    }

    /// std's `Command` inherits every inheritable handle, which delivers the mapping and event.
    fn environment(&self) -> TestResult<Vec<(&'static str, String)>> {
        let mut origin = 0_i64;
        if unsafe { QueryPerformanceCounter(&mut origin) } == 0 {
            return Err(Box::new(fastpad::platform::last_error()));
        }
        Ok(vec![
            (MAPPING_HANDLE_ENV, handle_value(self.mapping.as_raw())),
            (EVENT_HANDLE_ENV, handle_value(self.event.as_raw())),
            (QPC_ORIGIN_ENV, origin.to_string()),
        ])
    }

    fn read(&self) -> TestResult<Option<BenchmarkRecord>> {
        Ok(unsafe { read_shared_record(self.view.Value.cast()) }?)
    }
}

impl Drop for DiagnosticTransport {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
        }
    }
}

fn handle_value(handle: HANDLE) -> String {
    (handle as usize).to_string()
}

fn assert_startup_order(record: &BenchmarkRecord, pid: u32) {
    assert_eq!(
        record.pid, pid,
        "diagnostic frame came from another process"
    );
    let milestones = [
        record.process_start_us,
        record.window_created_us,
        record.editor_created_us,
        record.first_paint_us,
        record.first_input_accepted_us,
        record.first_input_rendered_us,
        record.settings_loaded_us,
        record.file_loaded_us,
        record.fully_ready_us,
    ];
    assert!(
        !milestones.contains(&0),
        "missing startup milestone: {record:?}"
    );
    assert!(
        record.process_start_us <= record.window_created_us
            && record.window_created_us <= record.editor_created_us
            && record.editor_created_us <= record.first_paint_us
            && record.editor_created_us <= record.first_input_accepted_us
            && record.first_input_accepted_us <= record.first_input_rendered_us
            && record.first_paint_us <= record.settings_loaded_us
            && record.settings_loaded_us <= record.file_loaded_us
            && record.file_loaded_us <= record.fully_ready_us,
        "startup milestones out of order: {record:?}"
    );
}

fn close_discarding_changes(process: FastPadProcess, hwnd: HWND) {
    unsafe {
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
    }
    let deadline = Deadline::after(Duration::from_secs(10));
    while wait_for_process_exit(process.id(), Duration::from_millis(20)).is_err() {
        assert!(!deadline.expired(), "FastPad did not exit after WM_CLOSE");
        if process.has_dialog().unwrap_or(false) {
            let _ = wait_and_dismiss_dialog(process.id(), Duration::from_secs(1));
        }
    }
    process.close().unwrap();
}

fn post_char(editor: HWND, unit: u16) {
    unsafe {
        PostMessageW(editor, WM_CHAR, usize::from(unit), 0);
    }
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Deadline::after(Duration::from_secs(5));
    while !condition() {
        assert!(!deadline.expired(), "timed out waiting for {what}");
        deadline.sleep_step();
    }
}

/// Mirrors the portable layout, where Lexilla.dll sits beside FastPad.exe. Shared with
/// `tests/windows/highlighting.rs`, so it is left in place rather than treated as owned data.
fn ensure_lexilla_beside_fastpad() {
    let target = Path::new(env!("CARGO_BIN_EXE_fastpad")).with_file_name(LEXILLA_DLL);
    if !target.exists() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("native")
            .join("out")
            .join("x64")
            .join(LEXILLA_DLL);
        std::fs::copy(&source, &target).unwrap_or_else(|error| {
            panic!(
                "could not stage {} beside FastPad: {error}",
                source.display()
            )
        });
    }
}

/// Uses the same vswhere lookup as the release tools; never skips when Visual Studio is absent.
fn locate_dumpbin() -> PathBuf {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tools")
        .join("msvc.ps1");
    let output = run_bounded(
        Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&script)
            .args(["-Find", "dumpbin.exe"]),
        Duration::from_secs(120),
    )
    .unwrap_or_else(|error| {
        panic!(
            "dumpbin.exe is required for the network-import gate and could not be located via \
             {} (needs pwsh and Visual Studio 2022 C++ x64 tools): {error}",
            script.display()
        )
    });
    let path = PathBuf::from(output.lines().last().unwrap_or_default().trim());
    assert!(
        path.is_file(),
        "tools/msvc.ps1 returned a dumpbin path that does not exist: {output}"
    );
    path
}

fn run_bounded(command: &mut Command, timeout: Duration) -> TestResult<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take().ok_or("missing stdout pipe")?;
    let mut stderr = child.stderr.take().ok_or("missing stderr pipe")?;
    let stdout_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let deadline = Deadline::after(timeout);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if deadline.expired() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("command timed out after {timeout:?}").into());
        }
        deadline.sleep_step();
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("exit status {status}: {stderr}{stdout}").into());
    }
    Ok(stdout)
}

fn imported_dlls(dumpbin_output: &str) -> Vec<String> {
    dumpbin_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.contains(' ') && line.to_ascii_lowercase().ends_with(".dll"))
        .map(str::to_ascii_lowercase)
        .collect()
}

fn running_fastpad_processes(image: &Path) -> Vec<u32> {
    let image_name = image
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut ids = vec![0_u32; 4096];
    let mut bytes = 0_u32;
    if unsafe { EnumProcesses(ids.as_mut_ptr(), (ids.len() * 4) as u32, &mut bytes) } == 0 {
        panic!("EnumProcesses failed: {}", fastpad::platform::last_error());
    }
    ids.truncate(bytes as usize / 4);
    ids.into_iter()
        .filter(|&pid| pid != 0 && process_image_file_name(pid).is_some_and(|n| n == image_name))
        .collect()
}

fn process_image_file_name(pid: u32) -> Option<String> {
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return None;
    }
    let handle = unsafe { OwnedHandle::from_raw_owned(raw) }.ok()?;
    let mut buffer = [0_u16; 1024];
    let mut length = buffer.len() as u32;
    if unsafe {
        QueryFullProcessImageNameW(
            handle.as_raw(),
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &mut length,
        )
    } == 0
    {
        return None;
    }
    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]));
    path.file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
}
