use crate::{FastPadError, Result};

#[cfg(windows)]
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

const MILESTONE_COUNT: usize = 9;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Milestone {
    ProcessStart,
    WindowCreated,
    EditorCreated,
    FirstPaint,
    FirstInputAccepted,
    FirstInputRendered,
    SettingsLoaded,
    FileLoaded,
    FullyReady,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupMetrics {
    frequency: i64,
    start: i64,
    ticks: [i64; MILESTONE_COUNT],
}

impl StartupMetrics {
    #[cfg(windows)]
    pub fn begin() -> Result<Self> {
        let mut frequency = 0_i64;
        let mut start = 0_i64;
        let frequency_ok = unsafe { QueryPerformanceFrequency(&mut frequency) };
        if frequency_ok == 0 {
            return Err(FastPadError::Win32(unsafe {
                windows_sys::Win32::Foundation::GetLastError()
            }));
        }
        let start_ok = unsafe { QueryPerformanceCounter(&mut start) };
        if start_ok == 0 {
            return Err(FastPadError::Win32(unsafe {
                windows_sys::Win32::Foundation::GetLastError()
            }));
        }

        let mut metrics = Self::with_frequency(frequency, start);
        metrics.record(Milestone::ProcessStart, start);
        Ok(metrics)
    }

    #[cfg(not(windows))]
    pub fn begin() -> Result<Self> {
        Err(FastPadError::Invariant(
            "startup metrics are only available on Windows",
        ))
    }

    pub fn with_frequency(frequency: i64, start: i64) -> Self {
        Self {
            frequency,
            start,
            ticks: [0; MILESTONE_COUNT],
        }
    }

    pub fn record(&mut self, milestone: Milestone, tick: i64) {
        let slot = &mut self.ticks[milestone as usize];
        if *slot == 0 {
            *slot = tick;
        }
    }

    #[cfg(windows)]
    pub fn record_now(&mut self, milestone: Milestone) -> Result<()> {
        let mut tick = 0_i64;
        let ok = unsafe { QueryPerformanceCounter(&mut tick) };
        if ok == 0 {
            return Err(FastPadError::Win32(unsafe {
                windows_sys::Win32::Foundation::GetLastError()
            }));
        }
        self.record(milestone, tick);
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn record_now(&mut self, _milestone: Milestone) -> Result<()> {
        Err(FastPadError::Invariant(
            "startup metrics are only available on Windows",
        ))
    }

    pub fn micros(&self, milestone: Milestone) -> Option<u64> {
        let tick = self.ticks[milestone as usize];
        (tick != 0).then(|| ((tick - self.start) * 1_000_000 / self.frequency) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::{Milestone, StartupMetrics};

    #[test]
    fn metrics_record_each_milestone_once() {
        // Break caught: overwriting an already-recorded milestone instead of preserving first tick.
        let mut metrics = StartupMetrics::with_frequency(10_000, 100);
        metrics.record(Milestone::WindowCreated, 143);
        metrics.record(Milestone::WindowCreated, 999);
        assert_eq!(metrics.micros(Milestone::WindowCreated), Some(4_300));
    }
}
