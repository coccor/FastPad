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
