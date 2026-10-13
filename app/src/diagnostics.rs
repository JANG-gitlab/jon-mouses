use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const LOG_LIMIT: u64 = 1024 * 1024;

const GAP_LIMITS: [Duration; 5] = [
    Duration::from_millis(1),
    Duration::from_millis(12),
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
];

/// Counts the gaps between motion events; gaps above the last limit are
/// pauses of the hand, not of the pipeline, and are left out.
#[derive(Default)]
pub struct Cadence {
    last: Option<Instant>,
    events: u32,
    gaps: [u32; 5],
}

impl Cadence {
    pub fn record(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last.replace(now) {
            let gap = now - last;
            if let Some(bucket) = GAP_LIMITS.iter().position(|limit| gap < *limit) {
                self.gaps[bucket] += 1;
            }
        }
        self.events += 1;
    }

    pub fn summary(&mut self) -> Option<String> {
        if self.events == 0 {
            return None;
        }
        let [instant, smooth, slow, late, stalled] = self.gaps;
        let line = format!(
            "{}/s gaps <1ms:{instant} 1-12:{smooth} 12-25:{slow} 25-50:{late} 50-100:{stalled}",
            self.events
        );
        *self = Self { last: self.last, ..Self::default() };
        Some(line)
    }
}

pub struct Log(Option<File>);

impl Log {
    /// Appends across restarts so a freeze that forced a relaunch still
    /// leaves its trace; past the limit the previous log is kept aside once.
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("diagnostics.log");
        if fs::metadata(&path).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
            let _ = fs::rename(&path, dir.join("diagnostics.old.log"));
        }
        Self(OpenOptions::new().create(true).append(true).open(path).ok())
    }

    pub fn write(&mut self, line: &str) {
        let Some(file) = &mut self.0 else {
            return;
        };
        let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_millis()).unwrap_or(0);
        let _ = writeln!(file, "{millis} {line}");
    }
}
