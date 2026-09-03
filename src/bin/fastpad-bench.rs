use fastpad::perf::protocol::BenchmarkRecord;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};

const BOOTSTRAP_RESAMPLES: usize = 10_000;
const BOOTSTRAP_SEED: u64 = 0xFA57_0A0D;
static ACTIVE_CHILD: AtomicIsize = AtomicIsize::new(0);

struct ActiveChildRegistration(isize);

impl ActiveChildRegistration {
    fn new(handle: isize) -> Self {
        ACTIVE_CHILD.store(handle, Ordering::Release);
        Self(handle)
    }

    fn clear(&self) {
        let _ = ACTIVE_CHILD.compare_exchange(self.0, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

impl Drop for ActiveChildRegistration {
    fn drop(&mut self) {
        self.clear();
    }
}

fn active_child_handle() -> isize {
    ACTIVE_CHILD.load(Ordering::Acquire)
}

#[derive(Debug, Eq, PartialEq)]
enum Action {
    Run {
        runs: usize,
        warmup: usize,
        output: PathBuf,
        enforce_reference: bool,
    },
    Compare {
        baseline: PathBuf,
        candidate: PathBuf,
    },
}

fn parse_args<I, S>(args: I) -> Result<Action, String>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if args.first().and_then(|arg| arg.to_str()) == Some("compare") {
        if args.len() != 3 {
            return Err("usage: fastpad-bench compare BASELINE.jsonl CANDIDATE.jsonl".to_owned());
        }
        return Ok(Action::Compare {
            baseline: PathBuf::from(&args[1]),
            candidate: PathBuf::from(&args[2]),
        });
    }

    let mut runs = 100_usize;
    let mut warmup = 10_usize;
    let mut output = PathBuf::from("benchmarks/latest.jsonl");
    let mut enforce_reference = false;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or_else(|| "benchmark options must be valid Unicode".to_owned())?;
        match flag {
            "--runs" | "--warmup" | "--output" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("missing value for {flag}"))?;
                match flag {
                    "--runs" => runs = parse_count(value, flag)?,
                    "--warmup" => warmup = parse_count(value, flag)?,
                    "--output" => output = PathBuf::from(value),
                    _ => unreachable!(),
                }
                index += 2;
            }
            "--enforce-reference" => {
                enforce_reference = true;
                index += 1;
            }
            _ => return Err(format!("unknown benchmark option: {flag}")),
        }
    }
    if runs == 0 {
        return Err("--runs must be greater than zero".to_owned());
    }
    Ok(Action::Run {
        runs,
        warmup,
        output,
        enforce_reference,
    })
}

fn parse_count(value: &OsString, flag: &str) -> Result<usize, String> {
    value
        .to_str()
        .ok_or_else(|| format!("{flag} must be valid Unicode"))?
        .parse()
        .map_err(|_| format!("{flag} must be a nonnegative integer"))
}

fn record_to_json_line(record: &BenchmarkRecord) -> String {
    serde_json::json!({
        "version": record.version,
        "pid": record.pid,
        "process_start_us": record.process_start_us,
        "window_created_us": record.window_created_us,
        "editor_created_us": record.editor_created_us,
        "first_paint_us": record.first_paint_us,
        "first_input_accepted_us": record.first_input_accepted_us,
        "first_input_rendered_us": record.first_input_rendered_us,
        "settings_loaded_us": record.settings_loaded_us,
        "file_loaded_us": record.file_loaded_us,
        "fully_ready_us": record.fully_ready_us,
        "idle_private_working_set_bytes": record.idle_private_working_set_bytes,
    })
    .to_string()
}

fn validate_record(record: &BenchmarkRecord, expected_pid: u32) -> Result<(), String> {
    if record.version != fastpad::perf::protocol::BENCHMARK_VERSION {
        return Err(format!(
            "unsupported benchmark record version {}",
            record.version
        ));
    }
    if record.pid != expected_pid {
        return Err(format!(
            "diagnostic PID {} did not match child PID {expected_pid}",
            record.pid
        ));
    }
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
    if milestones.contains(&0) {
        return Err("diagnostic frame is missing a startup milestone".to_owned());
    }
    if !(record.process_start_us <= record.window_created_us
        && record.window_created_us <= record.editor_created_us
        && record.editor_created_us <= record.first_paint_us
        && record.first_input_accepted_us <= record.first_input_rendered_us
        && record.settings_loaded_us <= record.file_loaded_us
        && record.file_loaded_us <= record.fully_ready_us)
    {
        return Err("diagnostic milestones are out of order".to_owned());
    }
    if record.idle_private_working_set_bytes == 0 {
        return Err("diagnostic record is missing idle private working set".to_owned());
    }
    Ok(())
}

fn is_fastpad_main_window_class(class_name: &str) -> bool {
    class_name == "FastPadMainWindow"
}

fn main() {
    let exit_code = match run_main() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("fastpad-bench: {error}");
            1
        }
    };
    std::process::exit(exit_code);
}

fn run_main() -> Result<i32, String> {
    match parse_args(std::env::args_os().skip(1))? {
        Action::Run {
            runs,
            warmup,
            output,
            enforce_reference,
        } => {
            install_console_cleanup()?;
            run_distribution(runs, warmup, &output, enforce_reference)
        }
        Action::Compare {
            baseline,
            candidate,
        } => compare_distributions(&baseline, &candidate),
    }
}

#[cfg(not(windows))]
fn install_console_cleanup() -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn install_console_cleanup() -> Result<(), String> {
    if unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console_control_handler), 1)
    } == 0
    {
        Err(fastpad::platform::last_error().to_string())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
unsafe extern "system" fn console_control_handler(control: u32) -> windows_sys::core::BOOL {
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    };
    use windows_sys::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};
    if !matches!(
        control,
        CTRL_C_EVENT
            | CTRL_BREAK_EVENT
            | CTRL_CLOSE_EVENT
            | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT
    ) {
        return 0;
    }
    let raw = active_child_handle();
    if raw != 0 {
        let handle = raw as windows_sys::Win32::Foundation::HANDLE;
        unsafe {
            TerminateProcess(handle, 1);
            WaitForSingleObject(handle, 5_000);
        }
    }
    1
}

fn run_distribution(
    runs: usize,
    warmup: usize,
    output: &std::path::Path,
    enforce_reference: bool,
) -> Result<i32, String> {
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let file = std::fs::File::create(output)
        .map_err(|error| format!("could not create {}: {error}", output.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    let mut records = Vec::with_capacity(runs);

    for index in 0..warmup + runs {
        let record = run_once()?;
        if index >= warmup {
            use std::io::Write;
            writeln!(writer, "{}", record_to_json_line(&record))
                .map_err(|error| format!("could not write {}: {error}", output.display()))?;
            records.push(record);
            eprintln!("record {}/{}: pid {}", records.len(), runs, record.pid);
        }
    }
    use std::io::Write;
    writer
        .flush()
        .map_err(|error| format!("could not flush {}: {error}", output.display()))?;

    print_distribution(&records);
    let mut tti = records
        .iter()
        .map(|record| record.first_input_rendered_us)
        .collect::<Vec<_>>();
    tti.sort_unstable();
    let p50 = percentile(&tti, 0.50);
    let p95 = percentile(&tti, 0.95);
    println!("valid_records={}", records.len());
    if enforce_reference && !reference_thresholds_pass(p50, p95) {
        eprintln!("reference threshold failed: warm_tti p50={p50}us p95={p95}us");
        return Ok(2);
    }
    Ok(0)
}

fn print_distribution(records: &[BenchmarkRecord]) {
    for (name, values) in milestone_columns(records) {
        let mut values = values;
        values.sort_unstable();
        println!(
            "{name}: p50={}us p95={}us",
            percentile(&values, 0.50),
            percentile(&values, 0.95)
        );
    }
    let mut memory = records
        .iter()
        .map(|record| record.idle_private_working_set_bytes)
        .collect::<Vec<_>>();
    memory.sort_unstable();
    println!(
        "idle_private_working_set_bytes: p50={} p95={}",
        percentile(&memory, 0.50),
        percentile(&memory, 0.95)
    );
}

type MetricAccessor = fn(&BenchmarkRecord) -> u64;

fn milestone_columns(records: &[BenchmarkRecord]) -> Vec<(&'static str, Vec<u64>)> {
    let fields: [(&str, MetricAccessor); 9] = [
        ("process_start", |record| record.process_start_us),
        ("window_created", |record| record.window_created_us),
        ("editor_created", |record| record.editor_created_us),
        ("first_paint", |record| record.first_paint_us),
        ("first_input_accepted", |record| {
            record.first_input_accepted_us
        }),
        ("first_input_rendered", |record| {
            record.first_input_rendered_us
        }),
        ("settings_loaded", |record| record.settings_loaded_us),
        ("file_loaded", |record| record.file_loaded_us),
        ("fully_ready", |record| record.fully_ready_us),
    ];
    fields
        .into_iter()
        .map(|(name, field)| (name, records.iter().map(field).collect()))
        .collect()
}

fn compare_distributions(
    baseline_path: &std::path::Path,
    candidate_path: &std::path::Path,
) -> Result<i32, String> {
    let baseline = read_records(baseline_path)?;
    let candidate = read_records(candidate_path)?;
    let baseline_columns = milestone_columns(&baseline);
    let candidate_columns = milestone_columns(&candidate);
    let mut regression = false;
    for ((name, baseline_values), (_, candidate_values)) in
        baseline_columns.into_iter().zip(candidate_columns)
    {
        let mut baseline_sorted = baseline_values.clone();
        let mut candidate_sorted = candidate_values.clone();
        baseline_sorted.sort_unstable();
        candidate_sorted.sort_unstable();
        let baseline_p95 = percentile(&baseline_sorted, 0.95);
        let candidate_p95 = percentile(&candidate_sorted, 0.95);
        let delta = candidate_p95 as i64 - baseline_p95 as i64;
        let (lower, upper) = bootstrap_p95_delta_ci(&baseline_values, &candidate_values);
        let regressed = is_regression(&baseline_values, &candidate_values);
        println!(
            "{name}: baseline_p95={baseline_p95}us candidate_p95={candidate_p95}us delta={delta}us bootstrap95=[{lower},{upper}]{}",
            if regressed { " REGRESSION" } else { "" }
        );
        regression |= regressed;
    }
    Ok(if regression { 2 } else { 0 })
}

fn read_records(path: &std::path::Path) -> Result<Vec<BenchmarkRecord>, String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let mut records = Vec::new();
    for (index, line) in std::io::BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|error| format!("could not read {}: {error}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line).map_err(|error| {
            format!(
                "{} line {} is not valid JSON: {error}",
                path.display(),
                index + 1
            )
        })?;
        let record = record_from_json(&value)
            .map_err(|error| format!("{} line {}: {error}", path.display(), index + 1))?;
        validate_record(&record, record.pid)?;
        records.push(record);
    }
    if records.is_empty() {
        return Err(format!("{} contains no records", path.display()));
    }
    Ok(records)
}

fn record_from_json(value: &serde_json::Value) -> Result<BenchmarkRecord, String> {
    let u64_field = |name| {
        value
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("missing unsigned integer field {name}"))
    };
    let version = u64_field("version")?;
    let pid = u64_field("pid")?;
    Ok(BenchmarkRecord {
        version: version
            .try_into()
            .map_err(|_| "version exceeds u32".to_owned())?,
        pid: pid.try_into().map_err(|_| "pid exceeds u32".to_owned())?,
        process_start_us: u64_field("process_start_us")?,
        window_created_us: u64_field("window_created_us")?,
        editor_created_us: u64_field("editor_created_us")?,
        first_paint_us: u64_field("first_paint_us")?,
        first_input_accepted_us: u64_field("first_input_accepted_us")?,
        first_input_rendered_us: u64_field("first_input_rendered_us")?,
        settings_loaded_us: u64_field("settings_loaded_us")?,
        file_loaded_us: u64_field("file_loaded_us")?,
        fully_ready_us: u64_field("fully_ready_us")?,
        idle_private_working_set_bytes: u64_field("idle_private_working_set_bytes")?,
    })
}

#[cfg(not(windows))]
fn run_once() -> Result<BenchmarkRecord, String> {
    Err("the startup benchmark requires Windows".to_owned())
}

#[cfg(windows)]
fn run_once() -> Result<BenchmarkRecord, String> {
    use fastpad::perf::protocol::{
        BENCHMARK_FRAME_LEN, BENCHMARK_INPUT_CHAR, EVENT_HANDLE_ENV, MAPPING_HANDLE_ENV,
        QPC_ORIGIN_ENV,
    };
    use fastpad::platform::{OwnedHandle, last_error, wide_null};
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE,
        UnmapViewOfFile,
    };
    use windows_sys::Win32::System::Performance::QueryPerformanceCounter;
    use windows_sys::Win32::System::Threading::{
        CREATE_UNICODE_ENVIRONMENT, CreateEventW, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
    };

    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate benchmark executable: {error}"))?
        .with_file_name("fastpad.exe");
    if !executable.is_file() {
        return Err(format!(
            "{} is missing; build the release fastpad binary first",
            executable.display()
        ));
    }

    let mut name_counter = 0_i64;
    if unsafe { QueryPerformanceCounter(&mut name_counter) } == 0 {
        return Err(last_error().to_string());
    }
    let unique = format!("{}-{name_counter}", std::process::id());
    let mapping_name = wide_null(&format!("Local\\FastPadBenchMapping-{unique}"));
    let event_name = wide_null(&format!("Local\\FastPadBenchEvent-{unique}"));
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mapping_raw = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            &security,
            PAGE_READWRITE,
            0,
            BENCHMARK_FRAME_LEN as u32,
            mapping_name.as_ptr(),
        )
    };
    let mapping =
        unsafe { OwnedHandle::from_raw_owned(mapping_raw) }.map_err(|error| error.to_string())?;
    let event_raw = unsafe { CreateEventW(&security, 0, 0, event_name.as_ptr()) };
    let event =
        unsafe { OwnedHandle::from_raw_owned(event_raw) }.map_err(|error| error.to_string())?;
    let view = unsafe {
        MapViewOfFile(
            mapping.as_raw(),
            FILE_MAP_READ | FILE_MAP_WRITE,
            0,
            0,
            BENCHMARK_FRAME_LEN,
        )
    };
    if view.Value.is_null() {
        return Err(last_error().to_string());
    }
    struct ViewGuard(windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS);
    impl Drop for ViewGuard {
        fn drop(&mut self) {
            unsafe {
                UnmapViewOfFile(self.0);
            }
        }
    }
    let _view_guard = ViewGuard(view);

    let origin_width = 20;
    let origin_placeholder = "0".repeat(origin_width);
    let mut environment = diagnostic_environment_block(
        mapping.as_raw(),
        event.as_raw(),
        &origin_placeholder,
        MAPPING_HANDLE_ENV,
        EVENT_HANDLE_ENV,
        QPC_ORIGIN_ENV,
    )?;
    let origin_marker = format!("{QPC_ORIGIN_ENV}={origin_placeholder}")
        .encode_utf16()
        .collect::<Vec<_>>();
    let origin_start = environment
        .windows(origin_marker.len())
        .position(|window| window == origin_marker)
        .ok_or_else(|| "could not locate QPC origin in environment block".to_owned())?
        + QPC_ORIGIN_ENV.encode_utf16().count()
        + 1;

    let application = executable
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let mut command_line = format!("\"{}\" --diagnostic", executable.display())
        .encode_utf16()
        .chain([0])
        .collect::<Vec<_>>();
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process_info = PROCESS_INFORMATION::default();
    let mut origin = 0_i64;
    if unsafe { QueryPerformanceCounter(&mut origin) } == 0 {
        return Err(last_error().to_string());
    }
    write_fixed_decimal(
        &mut environment[origin_start..origin_start + origin_width],
        origin,
    )?;
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr().cast(),
            std::ptr::null(),
            &startup,
            &mut process_info,
        )
    };
    if created == 0 {
        return Err(last_error().to_string());
    }
    let thread = unsafe { OwnedHandle::from_raw_owned(process_info.hThread) }
        .map_err(|error| error.to_string())?;
    drop(thread);
    let process = unsafe { OwnedHandle::from_raw_owned(process_info.hProcess) }
        .map_err(|error| error.to_string())?;
    let child = ChildGuard::new(process, process_info.dwProcessId);

    let main_hwnd = wait_for_main_window(&child)?;
    let scintilla = wait_for_scintilla(main_hwnd, &child)?;
    send_benchmark_char(scintilla, BENCHMARK_INPUT_CHAR)?;
    wait_for_event(event.as_raw(), &child)?;
    verify_benchmark_char(scintilla)?;
    let mut record = wait_for_fully_ready(view.Value.cast(), &child)?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    record.idle_private_working_set_bytes = private_working_set(child.process.as_raw())?;
    validate_record(&record, child.pid)?;
    child.close(main_hwnd)?;
    Ok(record)
}

#[cfg(windows)]
fn diagnostic_environment_block(
    mapping: windows_sys::Win32::Foundation::HANDLE,
    event: windows_sys::Win32::Foundation::HANDLE,
    origin: &str,
    mapping_name: &str,
    event_name: &str,
    origin_name: &str,
) -> Result<Vec<u16>, String> {
    let mut values = std::env::vars_os().collect::<Vec<_>>();
    values.retain(|(name, _)| {
        let name = name.to_string_lossy();
        !name.eq_ignore_ascii_case(mapping_name)
            && !name.eq_ignore_ascii_case(event_name)
            && !name.eq_ignore_ascii_case(origin_name)
    });
    values.push((mapping_name.into(), (mapping as usize).to_string().into()));
    values.push((event_name.into(), (event as usize).to_string().into()));
    values.push((origin_name.into(), origin.into()));
    values.sort_by(|(left, _), (right, _)| {
        left.to_string_lossy()
            .to_ascii_uppercase()
            .cmp(&right.to_string_lossy().to_ascii_uppercase())
    });
    let mut block = Vec::new();
    for (name, value) in values {
        use std::os::windows::ffi::OsStrExt;
        block.extend(name.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

#[cfg(windows)]
fn write_fixed_decimal(target: &mut [u16], value: i64) -> Result<(), String> {
    if value <= 0 {
        return Err("QPC origin must be positive".to_owned());
    }
    let text = format!("{value:0width$}", width = target.len());
    if text.len() != target.len() {
        return Err("QPC origin exceeded environment field width".to_owned());
    }
    for (slot, byte) in target.iter_mut().zip(text.bytes()) {
        *slot = u16::from(byte);
    }
    Ok(())
}

#[cfg(windows)]
struct ChildGuard {
    active: ActiveChildRegistration,
    process: fastpad::platform::OwnedHandle,
    pid: u32,
    closed: std::cell::Cell<bool>,
}

#[cfg(windows)]
impl ChildGuard {
    fn new(process: fastpad::platform::OwnedHandle, pid: u32) -> Self {
        let active = ActiveChildRegistration::new(process.as_raw() as isize);
        Self {
            active,
            process,
            pid,
            closed: std::cell::Cell::new(false),
        }
    }

    fn close(&self, hwnd: windows_sys::Win32::Foundation::HWND) -> Result<(), String> {
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_CLOSE,
        };
        let mut result = 0_usize;
        if unsafe {
            SendMessageTimeoutW(hwnd, WM_CLOSE, 0, 0, SMTO_ABORTIFHUNG, 5_000, &mut result)
        } == 0
        {
            return Err(fastpad::platform::last_error().to_string());
        }
        if unsafe { WaitForSingleObject(self.process.as_raw(), 5_000) } != WAIT_OBJECT_0 {
            return Err("FastPad did not exit after WM_CLOSE".to_owned());
        }
        self.active.clear();
        self.closed.set(true);
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};
        if !self.closed.get()
            && unsafe { WaitForSingleObject(self.process.as_raw(), 0) } != WAIT_OBJECT_0
        {
            unsafe {
                TerminateProcess(self.process.as_raw(), 1);
                WaitForSingleObject(self.process.as_raw(), 5_000);
            }
        }
        self.active.clear();
    }
}

#[cfg(windows)]
fn wait_for_main_window(
    guard: &ChildGuard,
) -> Result<windows_sys::Win32::Foundation::HWND, String> {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId};

    struct Search {
        pid: u32,
        hwnd: HWND,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut pid = 0_u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        let mut class = [0_u16; 64];
        let class_len = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW(
                hwnd,
                class.as_mut_ptr(),
                class.len() as i32,
            )
        };
        let is_main = class_len > 0
            && is_fastpad_main_window_class(&String::from_utf16_lossy(
                &class[..class_len as usize],
            ));
        if pid == search.pid && is_main {
            search.hwnd = hwnd;
            0
        } else {
            1
        }
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let mut search = Search {
            pid: guard.pid,
            hwnd: std::ptr::null_mut(),
        };
        unsafe {
            EnumWindows(Some(visit), (&mut search as *mut Search) as LPARAM);
        }
        if !search.hwnd.is_null() {
            return Ok(search.hwnd);
        }
        if unsafe { WaitForSingleObject(guard.process.as_raw(), 0) } == WAIT_OBJECT_0 {
            return Err("FastPad exited before creating its main window".to_owned());
        }
        if std::time::Instant::now() >= deadline {
            return Err("timed out waiting for FastPad main window".to_owned());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(windows)]
fn wait_for_scintilla(
    parent: windows_sys::Win32::Foundation::HWND,
    guard: &ChildGuard,
) -> Result<windows_sys::Win32::Foundation::HWND, String> {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let class = fastpad::platform::wide_null("Scintilla");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let hwnd = unsafe {
            FindWindowExW(
                parent,
                std::ptr::null_mut(),
                class.as_ptr(),
                std::ptr::null(),
            )
        };
        if !hwnd.is_null() {
            return Ok(hwnd);
        }
        if unsafe { WaitForSingleObject(guard.process.as_raw(), 0) } == WAIT_OBJECT_0 {
            return Err("FastPad exited before creating Scintilla".to_owned());
        }
        if std::time::Instant::now() >= deadline {
            return Err("timed out waiting for Scintilla".to_owned());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(windows)]
fn send_benchmark_char(
    editor: windows_sys::Win32::Foundation::HWND,
    character: usize,
) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_CHAR,
    };
    let mut result = 0_usize;
    if unsafe {
        SendMessageTimeoutW(
            editor,
            WM_CHAR,
            character,
            1,
            SMTO_ABORTIFHUNG,
            5_000,
            &mut result,
        )
    } == 0
    {
        Err(fastpad::platform::last_error().to_string())
    } else {
        Ok(())
    }
}

fn verify_benchmark_utf8(
    length: usize,
    mut get_byte: impl FnMut(usize) -> u8,
) -> Result<(), String> {
    let expected = "\u{E000}".as_bytes();
    if length != expected.len() {
        return Err(format!(
            "Scintilla benchmark text length was {length}, expected {}",
            expected.len()
        ));
    }
    for (index, expected_byte) in expected.iter().copied().enumerate() {
        let actual = get_byte(index);
        if actual != expected_byte {
            return Err(format!(
                "Scintilla benchmark byte {index} was {actual:#04x}, expected {expected_byte:#04x}"
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn wait_for_event(
    event: windows_sys::Win32::Foundation::HANDLE,
    guard: &ChildGuard,
) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match unsafe { WaitForSingleObject(event, 10) } {
            WAIT_OBJECT_0 => return Ok(()),
            WAIT_TIMEOUT => {}
            _ => return Err(fastpad::platform::last_error().to_string()),
        }
        if unsafe { WaitForSingleObject(guard.process.as_raw(), 0) } == WAIT_OBJECT_0 {
            return Err("FastPad exited before signaling rendered input".to_owned());
        }
        if std::time::Instant::now() >= deadline {
            return Err("timed out waiting for rendered input".to_owned());
        }
    }
}

#[cfg(windows)]
fn verify_benchmark_char(editor: windows_sys::Win32::Foundation::HWND) -> Result<(), String> {
    use fastpad::editor::scintilla_constants::SCI_GETLENGTH;
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;
    const SCI_GETCHARAT: u32 = 2007;
    let length = unsafe { SendMessageW(editor, SCI_GETLENGTH, 0, 0) };
    if length < 0 {
        return Err("Scintilla did not retain the benchmark character".to_owned());
    }
    verify_benchmark_utf8(length as usize, |index| unsafe {
        SendMessageW(editor, SCI_GETCHARAT, index, 0) as u8
    })
}

#[cfg(windows)]
fn wait_for_fully_ready(view: *const u8, guard: &ChildGuard) -> Result<BenchmarkRecord, String> {
    use fastpad::perf::protocol::BENCHMARK_FRAME_LEN;
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let mut bytes = [0_u8; BENCHMARK_FRAME_LEN];
        unsafe {
            std::ptr::copy_nonoverlapping(view, bytes.as_mut_ptr(), bytes.len());
        }
        if let Ok(record) = BenchmarkRecord::decode(&bytes)
            && record.fully_ready_us != 0
        {
            return Ok(record);
        }
        if unsafe { WaitForSingleObject(guard.process.as_raw(), 0) } == WAIT_OBJECT_0 {
            return Err("FastPad exited before reaching FullyReady".to_owned());
        }
        if std::time::Instant::now() >= deadline {
            return Err("timed out waiting for FullyReady".to_owned());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(windows)]
fn private_working_set(process: windows_sys::Win32::Foundation::HANDLE) -> Result<u64, String> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    if unsafe {
        GetProcessMemoryInfo(
            process,
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
    } == 0
    {
        return Err(fastpad::platform::last_error().to_string());
    }
    Ok(counters.PrivateUsage as u64)
}

fn percentile(sorted: &[u64], percentile: f64) -> u64 {
    let index = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted[index]
}

fn reference_thresholds_pass(tti_p50_us: u64, tti_p95_us: u64) -> bool {
    tti_p50_us < 25_000 && tti_p95_us < 40_000
}

fn is_regression(baseline: &[u64], candidate: &[u64]) -> bool {
    if baseline.is_empty() || candidate.is_empty() {
        return false;
    }
    let mut baseline_sorted = baseline.to_vec();
    let mut candidate_sorted = candidate.to_vec();
    baseline_sorted.sort_unstable();
    candidate_sorted.sort_unstable();
    let baseline_p95 = percentile(&baseline_sorted, 0.95);
    let candidate_p95 = percentile(&candidate_sorted, 0.95);
    let delta = candidate_p95.saturating_sub(baseline_p95);
    let material_delta = 2_000_u64.max(baseline_p95.div_ceil(10));
    let (lower, _) = bootstrap_p95_delta_ci(baseline, candidate);
    delta >= material_delta && lower > 0
}

fn bootstrap_p95_delta_ci(baseline: &[u64], candidate: &[u64]) -> (i64, i64) {
    assert!(!baseline.is_empty());
    assert!(!candidate.is_empty());
    let mut rng = DeterministicRng::new(BOOTSTRAP_SEED);
    let mut baseline_sample = vec![0; baseline.len()];
    let mut candidate_sample = vec![0; candidate.len()];
    let mut deltas = Vec::with_capacity(BOOTSTRAP_RESAMPLES);

    for _ in 0..BOOTSTRAP_RESAMPLES {
        for value in &mut baseline_sample {
            *value = baseline[rng.index(baseline.len())];
        }
        for value in &mut candidate_sample {
            *value = candidate[rng.index(candidate.len())];
        }
        baseline_sample.sort_unstable();
        candidate_sample.sort_unstable();
        deltas.push(
            percentile(&candidate_sample, 0.95) as i64 - percentile(&baseline_sample, 0.95) as i64,
        );
    }

    deltas.sort_unstable();
    (
        signed_percentile(&deltas, 0.025),
        signed_percentile(&deltas, 0.975),
    )
}

fn signed_percentile(sorted: &[i64], percentile: f64) -> i64 {
    let index = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted[index]
}

struct DeterministicRng(u64);

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn index(&mut self, upper: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)) % upper as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Action, ActiveChildRegistration, active_child_handle, bootstrap_p95_delta_ci,
        is_fastpad_main_window_class, is_regression, parse_args, percentile, record_to_json_line,
        reference_thresholds_pass, validate_record, verify_benchmark_utf8,
    };
    use fastpad::perf::protocol::BenchmarkRecord;
    use std::path::PathBuf;

    #[test]
    fn percentile_uses_the_required_ceiling_rank() {
        // Break caught: rounding or flooring the rank understates p50/p95 for small distributions.
        assert_eq!(percentile(&[10, 20, 30, 40], 0.50), 30);
        assert_eq!(percentile(&[10, 20, 30, 40], 0.95), 40);
    }

    #[test]
    fn reference_thresholds_reject_values_at_the_limit() {
        // Break caught: using strict-greater threshold checks accepts the explicitly disallowed
        // 25 ms p50 or 40 ms p95 warm TTI boundary.
        assert!(!reference_thresholds_pass(25_000, 39_999));
        assert!(!reference_thresholds_pass(24_999, 40_000));
        assert!(reference_thresholds_pass(24_999, 39_999));
    }

    #[test]
    fn comparison_requires_material_delta_and_positive_bootstrap_interval() {
        // Break caught: reporting noise below the absolute/relative gate, or a p95 increase whose
        // confidence interval still includes zero, as a benchmark regression.
        let baseline = vec![10_000; 20];
        let material_candidate = vec![12_000; 20];
        let small_candidate = vec![11_999; 20];

        assert_eq!(
            bootstrap_p95_delta_ci(&baseline, &material_candidate),
            (2_000, 2_000)
        );
        assert!(is_regression(&baseline, &material_candidate));
        assert!(!is_regression(&baseline, &small_candidate));

        let high_baseline = vec![30_000; 20];
        let ten_percent_candidate = vec![33_000; 20];
        assert!(is_regression(&high_baseline, &ten_percent_candidate));
    }

    #[test]
    fn command_line_supports_run_and_compare_modes() {
        // Break caught: interpreting compare paths as run options, or silently ignoring explicit
        // warmup/output/reference settings, runs the wrong benchmark workload.
        assert_eq!(
            parse_args([
                "--runs",
                "20",
                "--warmup",
                "5",
                "--output",
                "sample.jsonl",
                "--enforce-reference"
            ])
            .unwrap(),
            Action::Run {
                runs: 20,
                warmup: 5,
                output: PathBuf::from("sample.jsonl"),
                enforce_reference: true,
            }
        );
        assert_eq!(
            parse_args(["compare", "baseline.jsonl", "candidate.jsonl"]).unwrap(),
            Action::Compare {
                baseline: PathBuf::from("baseline.jsonl"),
                candidate: PathBuf::from("candidate.jsonl"),
            }
        );
    }

    #[test]
    fn json_line_contains_every_fixed_record_field() {
        // Break caught: omitting or renaming a protocol field makes persisted distributions
        // impossible to compare with the fixed diagnostic frame.
        let line = record_to_json_line(&BenchmarkRecord {
            version: 1,
            pid: 42,
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
        });
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 12);
        assert_eq!(value["first_input_rendered_us"], 6);
        assert_eq!(value["idle_private_working_set_bytes"], 10);
    }

    #[test]
    fn record_validation_rejects_missing_or_misordered_milestones() {
        // Break caught: counting a partially written or internally inconsistent shared-memory
        // frame as a valid benchmark sample corrupts percentile distributions.
        let mut record = BenchmarkRecord {
            version: 1,
            pid: 42,
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
        };
        assert!(validate_record(&record, 42).is_ok());
        record.first_input_rendered_us = 0;
        assert!(validate_record(&record, 42).is_err());
        record.first_input_rendered_us = 4;
        assert!(validate_record(&record, 42).is_err());
        record.first_input_rendered_us = 6;
        assert!(validate_record(&record, 99).is_err());
        record.pid = 42;
        record.version = 2;
        assert!(validate_record(&record, 42).is_err());
    }

    #[test]
    fn main_window_discovery_rejects_process_owned_ime_helpers() {
        // Break caught: accepting the first top-level HWND for the child PID can select its IME
        // helper window, beneath which no Scintilla child exists.
        assert!(!is_fastpad_main_window_class("IME"));
        assert!(is_fastpad_main_window_class("FastPadMainWindow"));
    }

    #[test]
    fn benchmark_character_verification_uses_scalar_scintilla_reads() {
        // Break caught: passing a harness-process buffer pointer to child-process SCI_GETTEXT
        // cannot retrieve the inserted UTF-8 bytes across the process boundary.
        let expected = "\u{E000}".as_bytes();
        assert!(verify_benchmark_utf8(expected.len(), |index| expected[index]).is_ok());
        assert!(verify_benchmark_utf8(expected.len(), |_| 0).is_err());
    }

    #[test]
    fn active_child_registration_clears_the_console_interrupt_target() {
        // Break caught: relying only on ChildGuard::drop lets Ctrl+C terminate the harness before
        // Rust destructors run, leaving the currently benchmarked FastPad process orphaned.
        let registration = ActiveChildRegistration::new(123);
        assert_eq!(active_child_handle(), 123);
        drop(registration);
        assert_eq!(active_child_handle(), 0);
    }
}
