//! Main-thread freeze watchdog.
//!
//! Registers a CFRunLoopObserver on the main run loop that increments a heartbeat
//! counter. A background thread checks the counter periodically — if the main
//! thread hasn't advanced the heartbeat in 5 seconds, it captures diagnostics
//! (callback ring buffer + `sample` thread dump) and writes them to a temp file.

#[cfg(target_os = "macos")]
use std::os::raw::c_void;
#[cfg(target_os = "macos")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

/// Whether `run_on_main_sync` is currently waiting for the main thread.
/// Set to true before signaling the CFRunLoopSource, cleared after completion.
/// Read by the watchdog to distinguish "main thread frozen during our dispatch"
/// from "main thread frozen in plugin code".
pub static MAIN_SYNC_INFLIGHT: AtomicBool = AtomicBool::new(false);

struct WatchdogState {
    #[cfg(target_os = "macos")]
    heartbeat: AtomicU64,
    stop: AtomicBool,
}

static PREFIX: OnceLock<String> = OnceLock::new();

static STATE: OnceLock<Arc<WatchdogState>> = OnceLock::new();

/// Start the freeze watchdog. Call once from the application startup after
/// the main run loop is serviceable.
///
/// This must be called from the main thread so the CFRunLoopObserver is
/// registered on the correct run loop.
pub fn start() {
    start_with_prefix("plugin-hostkit");
}

/// Start once on the main thread, using a filename-safe diagnostic prefix.
/// Later calls have no effect. Non-macOS platforms do not install a run-loop observer.
pub fn start_with_prefix(prefix: &str) {
    PREFIX.get_or_init(|| {
        prefix
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    });
    #[cfg(not(target_os = "macos"))]
    return;
    #[cfg(target_os = "macos")]
    start_on_main();
}

#[cfg(target_os = "macos")]
fn start_on_main() {
    let state = Arc::new(WatchdogState {
        heartbeat: AtomicU64::new(0),
        stop: AtomicBool::new(false),
    });
    if STATE.set(state.clone()).is_err() {
        return;
    }

    // Register CFRunLoopObserver on the main run loop.
    // The observer increments the heartbeat counter on kCFRunLoopAllActivities,
    // which fires each time the run loop processes events.
    register_heartbeat_observer(&state);

    // Spawn the watchdog background thread.
    let watch_state = state.clone();
    std::thread::Builder::new()
        .name("freeze-watchdog".into())
        .spawn(move || watchdog_loop(watch_state))
        .expect("failed to spawn freeze watchdog thread");

    tracing::info!("freeze watchdog started");
}

/// Stop the freeze watchdog. The background thread will exit on its next wake cycle.
pub fn stop() {
    if let Some(state) = STATE.get() {
        state.stop.store(true, Ordering::Relaxed);
    }
}

/// Emit a freeze diagnostic from any context (called by run_on_main_sync timeout).
pub fn emit_freeze_diagnostic(reason: &str) {
    let ring_dump = crate::com::dump_callback_ring();
    let inflight = MAIN_SYNC_INFLIGHT.load(Ordering::Relaxed);

    let report = format!(
        "=== Plugin Hostkit Freeze Diagnostic ===\n\
         Reason: {reason}\n\
         Timestamp: {:?}\n\
         run_on_main_sync inflight: {inflight}\n\
         \n\
         --- Host Callback Ring Buffer (last 128 entries) ---\n\
         {ring_dump}\n\
         \n\
         --- Thread Sample ---\n\
         (capturing...)\n",
        std::time::SystemTime::now()
    );

    // Write diagnostic file — this runs on a background thread, no main thread needed.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let prefix = PREFIX.get().map(String::as_str).unwrap_or("plugin-hostkit");
    let path = std::env::temp_dir().join(format!("{prefix}-freeze-{ts}.txt"));

    // Capture thread sample via macOS `sample` command
    let pid = std::process::id();
    let sample_output = std::process::Command::new("sample")
        .arg(pid.to_string())
        .arg("1") // sample for 1 second
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_else(|e| format!("(sample failed: {e})"));

    let full_report = format!("{report}{sample_output}\n");

    match std::fs::write(&path, &full_report) {
        Ok(()) => {
            tracing::error!(
                path = %path.display(),
                "freeze diagnostic written — check this file for thread stacks"
            );
            eprintln!("[FREEZE] diagnostic written to {}", path.display());
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to write freeze diagnostic");
            eprintln!("[FREEZE] failed to write diagnostic: {e}");
            eprintln!("{full_report}");
        }
    }
}

#[cfg(target_os = "macos")]
fn watchdog_loop(state: Arc<WatchdogState>) {
    let mut last_heartbeat = state.heartbeat.load(Ordering::Relaxed);
    let mut last_change = Instant::now();
    let mut freeze_reported = false;
    let threshold = Duration::from_secs(5);

    loop {
        std::thread::sleep(Duration::from_secs(1));

        if state.stop.load(Ordering::Relaxed) {
            tracing::info!("freeze watchdog stopping");
            break;
        }

        let current = state.heartbeat.load(Ordering::Relaxed);
        if current != last_heartbeat {
            last_heartbeat = current;
            last_change = Instant::now();
            if freeze_reported {
                tracing::info!("main thread responsive again after freeze");
                freeze_reported = false;
            }
        } else if last_change.elapsed() > threshold && !freeze_reported {
            freeze_reported = true;
            tracing::error!(
                stall_secs = last_change.elapsed().as_secs(),
                heartbeat = current,
                "main thread freeze detected — capturing diagnostics"
            );
            emit_freeze_diagnostic("watchdog: main thread unresponsive");
        }
    }
}

// ── CFRunLoopObserver registration (dynamic loading) ───────────────

// CoreFoundation types (opaque pointers)
#[cfg(target_os = "macos")]
type CFRunLoopRef = *mut c_void;
#[cfg(target_os = "macos")]
type CFRunLoopObserverRef = *mut c_void;
#[cfg(target_os = "macos")]
type CFIndex = isize;
#[cfg(target_os = "macos")]
type CFOptionFlags = u64;
#[cfg(target_os = "macos")]
type CFAllocatorRef = *const c_void;

// kCFRunLoopAllActivities = 0x0FFFFFFF — fires on every run loop activity
#[cfg(target_os = "macos")]
const K_CF_RUN_LOOP_ALL_ACTIVITIES: CFOptionFlags = 0x0FFF_FFFF;

#[cfg(target_os = "macos")]
type CFRunLoopObserverCallBack = unsafe extern "C" fn(
    observer: CFRunLoopObserverRef,
    activity: CFOptionFlags,
    info: *mut c_void,
);

/// Matches `CFRunLoopObserverContext` from `<CoreFoundation/CFRunLoop.h>`:
///
/// ```c
/// typedef struct {
///     CFIndex        version;
///     void          *info;
///     const void   *(*retain)(const void *info);
///     void          (*release)(const void *info);
///     CFStringRef   (*copyDescription)(const void *info);
/// } CFRunLoopObserverContext;
/// ```
#[cfg(target_os = "macos")]
#[repr(C)]
struct CFRunLoopObserverContext {
    version: CFIndex,
    info: *mut c_void,
    retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<unsafe extern "C" fn(*const c_void)>,
    copy_description: Option<unsafe extern "C" fn(*const c_void) -> *mut c_void>,
}

#[cfg(target_os = "macos")]
type CFRunLoopGetMain = unsafe extern "C" fn() -> CFRunLoopRef;
#[cfg(target_os = "macos")]
type CFRunLoopObserverCreate = unsafe extern "C" fn(
    allocator: CFAllocatorRef,
    activities: CFOptionFlags,
    repeats: u8, // Boolean
    order: CFIndex,
    callout: CFRunLoopObserverCallBack,
    context: *mut CFRunLoopObserverContext,
) -> CFRunLoopObserverRef;
#[cfg(target_os = "macos")]
type CFRunLoopAddObserver =
    unsafe extern "C" fn(rl: CFRunLoopRef, observer: CFRunLoopObserverRef, mode: *const c_void);

#[cfg(target_os = "macos")]
fn register_heartbeat_observer(state: &Arc<WatchdogState>) {
    // Leak a clone of the Arc into the observer context so the callback can
    // reach WatchdogState for the process lifetime.
    let state_ptr = Arc::into_raw(state.clone()) as *mut c_void;

    // Build the observer context struct on the stack. We set only `version`
    // and `info`; the retain/release/copyDescription callbacks are null because
    // the Arc was already leaked — CF need not manage its lifetime.
    let mut ctx = CFRunLoopObserverContext {
        version: 0,
        info: state_ptr,
        retain: None,
        release: None,
        copy_description: None,
    };

    unsafe {
        let cf = libloading::Library::new(
            "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation",
        )
        .expect("failed to load CoreFoundation");

        let get_main: CFRunLoopGetMain = *cf.get(b"CFRunLoopGetMain\0").expect("CFRunLoopGetMain");
        let create_observer: CFRunLoopObserverCreate = *cf
            .get(b"CFRunLoopObserverCreate\0")
            .expect("CFRunLoopObserverCreate");
        let add_observer: CFRunLoopAddObserver = *cf
            .get(b"CFRunLoopAddObserver\0")
            .expect("CFRunLoopAddObserver");

        // Use kCFRunLoopCommonModes so the heartbeat fires during all common
        // run loop modes (default, modal, tracking). kCFRunLoopDefaultMode would
        // miss iterations during modal panels and window resizing, causing
        // false-positive freeze reports.
        let mode_var_addr = *cf
            .get::<*const c_void>(b"kCFRunLoopCommonModes\0")
            .expect("kCFRunLoopCommonModes");
        let cf_run_loop_common_modes = *(mode_var_addr as *const *const c_void);

        let run_loop = get_main();
        let observer = create_observer(
            std::ptr::null(), // kCFAllocatorDefault
            K_CF_RUN_LOOP_ALL_ACTIVITIES,
            1, // repeats = YES
            0, // order
            heartbeat_callback,
            &mut ctx,
        );
        add_observer(run_loop, observer, cf_run_loop_common_modes);

        // Keep CoreFoundation loaded for process lifetime.
        std::mem::forget(cf);

        tracing::debug!("CFRunLoopObserver registered for heartbeat");
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn heartbeat_callback(
    _observer: CFRunLoopObserverRef,
    _activity: CFOptionFlags,
    info: *mut c_void,
) {
    // SAFETY: `info` is the Arc<WatchdogState> leaked via Arc::into_raw in
    // register_heartbeat_observer. We only read from it (fetch_add), never drop
    // — the Arc lives for the process lifetime.
    let state = unsafe { &*(info as *const WatchdogState) };
    state.heartbeat.fetch_add(1, Ordering::Relaxed);
}
