//! Opt-in per-frame phase timing. Enabled by `OUTRIDER_PROFILE=1`; appends
//! one line per frame to `%TEMP%/outrider-profile.log` with the cost of
//! each named phase in microseconds, plus the frame total. Zero cost when
//! disabled beyond one atomic load per `phase()` call.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the first profiled frame (wall clock, for pacing).
fn epoch_ms() -> u128 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis()
}

static ENABLED: AtomicU8 = AtomicU8::new(2); // 2 = unknown, 1 = on, 0 = off

fn enabled() -> bool {
    match ENABLED.load(Ordering::Relaxed) {
        1 => true,
        0 => false,
        _ => {
            let on = std::env::var_os("OUTRIDER_PROFILE").is_some();
            ENABLED.store(u8::from(on), Ordering::Relaxed);
            on
        }
    }
}

/// One frame's phase timings.
pub(crate) struct FrameProfile {
    start: Instant,
    last: Instant,
    phases: Vec<(&'static str, u128)>,
}

impl FrameProfile {
    pub(crate) fn begin() -> Option<FrameProfile> {
        if !enabled() {
            return None;
        }
        let now = Instant::now();
        Some(FrameProfile {
            start: now,
            last: now,
            phases: Vec::with_capacity(16),
        })
    }

    /// Close the current phase under `name`.
    pub(crate) fn phase(&mut self, name: &'static str) {
        let now = Instant::now();
        self.phases
            .push((name, now.duration_since(self.last).as_micros()));
        self.last = now;
    }

    /// Write the line. `extra` is free-form context (item counts etc.).
    pub(crate) fn finish(self, extra: &str) {
        use std::io::Write;
        let total = self.start.elapsed().as_micros();
        let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("outrider-profile.log"))
        else {
            return;
        };
        let mut line = format!("t={} total={total}us", epoch_ms());
        for (name, us) in &self.phases {
            line.push_str(&format!(" {name}={us}"));
        }
        if !extra.is_empty() {
            line.push(' ');
            line.push_str(extra);
        }
        let _ = writeln!(f, "{line}");
    }
}

/// Append a diagnostic line to `%TEMP%/outrider-debug.log` when profiling
/// is enabled. The message closure only runs when enabled.
pub(crate) fn debug_log(msg: impl FnOnce() -> String) {
    use std::io::Write;
    if !enabled() {
        return;
    }
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("outrider-debug.log"))
    else {
        return;
    };
    let _ = writeln!(f, "t={} {}", epoch_ms(), msg());
}

/// Helper: time a phase on an optional profile.
macro_rules! profile_phase {
    ($p:expr, $name:literal) => {
        if let Some(p) = $p.as_mut() {
            p.phase($name);
        }
    };
}
pub(crate) use profile_phase;
