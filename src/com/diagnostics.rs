use super::*;

// ── Host callback ring buffer (freeze diagnostics) ─────────────────
//
// Bounded nonblocking ring that records host COM callbacks with a
// timestamp. Survives main-thread freezes because entries are written
// from whatever thread the callback fires on and can be read from the
// watchdog background thread.

/// Callback type tag for the ring buffer.
#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum HostCbKind {
    BeginEdit = 1,
    PerformEdit = 2,
    EndEdit = 3,
    RestartComponent = 4,
    SetDirty = 5,
    RequestOpenEditor = 6,
    StartGroupEdit = 7,
    FinishGroupEdit = 8,
    CreateContextMenu = 9,
    UnitSelection = 10,
    ProgramListChange = 11,
    HostQiUnknown = 12,
}

/// A single ring buffer entry — 24 bytes, no heap allocation.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct HostCbEntry {
    pub mono_ns: u64,
    pub kind: HostCbKind,
    pub _pad: [u8; 3],
    pub param_id: u32,
    pub extra: u64, // f64::to_bits() for PerformEdit, flags for RestartComponent, IID prefix for QI
}

const RING_LEN: usize = 128;
static RING_HEAD: AtomicUsize = AtomicUsize::new(0);

// Both readers and writers use nonblocking ownership of each slot. Busy
// entries are skipped: diagnostics must not delay callbacks or the watchdog.
struct RingSlot {
    data: Mutex<HostCbEntry>,
}

const EMPTY_ENTRY: HostCbEntry = HostCbEntry {
    mono_ns: 0,
    kind: HostCbKind::BeginEdit, // placeholder
    _pad: [0; 3],
    param_id: 0,
    extra: 0,
};
#[allow(clippy::declare_interior_mutable_const)] // Repeated initializer creates distinct atomics.
const EMPTY_SLOT: RingSlot = RingSlot {
    data: Mutex::new(EMPTY_ENTRY),
};

static RING: [RingSlot; RING_LEN] = [EMPTY_SLOT; RING_LEN];

fn mono_ns() -> u64 {
    // mach_absolute_time is async-signal-safe and allocation-free on macOS.
    // On Linux, use clock_gettime(CLOCK_MONOTONIC).
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn mach_absolute_time() -> u64;
        }
        unsafe { mach_absolute_time() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::time::Instant;
        static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let epoch = EPOCH.get_or_init(Instant::now);
        epoch.elapsed().as_nanos() as u64
    }
}

pub(super) fn ring_push(kind: HostCbKind, param_id: u32, extra: u64) {
    let idx = RING_HEAD.fetch_add(1, Ordering::Relaxed) % RING_LEN;
    let slot = &RING[idx];
    let entry = HostCbEntry {
        mono_ns: mono_ns(),
        kind,
        _pad: [0; 3],
        param_id,
        extra,
    };
    if let Ok(mut data) = slot.data.try_lock() {
        *data = entry;
    }
}

/// Dump the last RING_LEN host callback entries as a human-readable string.
/// Called from the watchdog thread when a freeze is detected.
pub fn dump_callback_ring() -> String {
    let head = RING_HEAD.load(Ordering::Relaxed);
    let start = head.saturating_sub(RING_LEN);
    let mut lines = Vec::with_capacity(RING_LEN);
    let mut prev_ns: u64 = 0;
    for i in start..head {
        let slot = &RING[i % RING_LEN];
        let entry = match slot.data.try_lock() {
            Ok(data) => *data,
            Err(_) => continue,
        };
        if entry.mono_ns == 0 {
            continue;
        }
        let delta = if prev_ns > 0 {
            format!(
                "+{:.3}ms",
                (entry.mono_ns.saturating_sub(prev_ns)) as f64 / 1_000_000.0
            )
        } else {
            "       ".to_string()
        };
        prev_ns = entry.mono_ns;
        lines.push(format!(
            "  [{:>12}ns {delta}] {:?} param_id={} extra=0x{:016x}",
            entry.mono_ns, entry.kind, entry.param_id, entry.extra
        ));
    }
    if lines.is_empty() {
        "  (no host callbacks recorded)".to_string()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_ring_supports_concurrent_wraparound_and_nonblocking_reads() {
        std::thread::scope(|scope| {
            for producer in 0..4 {
                scope.spawn(move || {
                    for n in 0..2048 {
                        ring_push(HostCbKind::PerformEdit, producer, n);
                    }
                });
            }
            scope.spawn(|| {
                for _ in 0..128 {
                    let _ = dump_callback_ring();
                }
            });
        });
        assert!(dump_callback_ring().contains("PerformEdit"));
        let _held = RING[0].data.lock().unwrap();
        // A dump must skip a busy slot rather than wait for its writer.
        let _ = dump_callback_ring();
    }
}
