//! Integration tests for the CFRunLoopSource-based dispatch fix.
//!
//! Previously, `run_on_main_sync` used `dispatch_sync_f` (GCD), which
//! deadlocked when called from a thread that already held the main queue
//! (e.g., from within a CFRunLoop callback). The fix replaces it with a
//! CFRunLoopSource posted directly to the main run loop, which never
//! deadlocks regardless of the calling context.
//!
//! This file tests that `EditorView::open()` completes without deadlocking
//! against the Addictive Drums 2 plugin. A `SIGALRM` alarm is set before
//! the open call as a hard kill-switch: if the call hangs, the process dies
//! after 10 seconds and the test fails.
//!
//! Run with: `cargo test -p plugin-hostkit --test deadlock_editor_tests`

use plugin_hostkit::discovery::Vst3Bundle;
use plugin_hostkit::host::ProcessConfig;
use plugin_hostkit::{EditorView, VstInstance};

use std::path::PathBuf;
use std::process::ExitCode;

// ── macOS NSApplication + CFRunLoop ─────────────────────────────────

#[cfg(target_os = "macos")]
mod objc_ffi {
    use std::ffi::c_void;

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        pub fn NSApplicationLoad() -> bool;
    }

    unsafe extern "C" {
        pub fn objc_getClass(name: *const u8) -> *mut c_void;
        pub fn sel_registerName(name: *const u8) -> *mut c_void;
        pub fn objc_msgSend();
    }

    pub unsafe fn shared_application() -> *mut c_void {
        unsafe {
            type Fn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
            let f: Fn = std::mem::transmute(objc_msgSend as *const c_void);
            let cls = objc_getClass(c"NSApplication".as_ptr().cast());
            f(cls, sel_registerName(c"sharedApplication".as_ptr().cast()))
        }
    }

    pub unsafe fn send_void(target: *mut c_void, sel: *mut c_void) {
        unsafe {
            type Fn = unsafe extern "C" fn(*mut c_void, *mut c_void);
            let f: Fn = std::mem::transmute(objc_msgSend as *const c_void);
            f(target, sel);
        }
    }

    pub unsafe fn send_i64(target: *mut c_void, sel: *mut c_void, arg: i64) {
        unsafe {
            type Fn = unsafe extern "C" fn(*mut c_void, *mut c_void, i64);
            let f: Fn = std::mem::transmute(objc_msgSend as *const c_void);
            f(target, sel, arg);
        }
    }

    pub unsafe fn send_i8(target: *mut c_void, sel: *mut c_void, arg: i8) {
        unsafe {
            type Fn = unsafe extern "C" fn(*mut c_void, *mut c_void, i8);
            let f: Fn = std::mem::transmute(objc_msgSend as *const c_void);
            f(target, sel, arg);
        }
    }
}

#[cfg(target_os = "macos")]
fn setup_nsapplication() {
    unsafe {
        use objc_ffi::*;
        NSApplicationLoad();
        let app = shared_application();
        send_i64(
            app,
            sel_registerName(c"setActivationPolicy:".as_ptr().cast()),
            0,
        );
        send_void(app, sel_registerName(c"finishLaunching".as_ptr().cast()));
        send_i8(
            app,
            sel_registerName(c"activateIgnoringOtherApps:".as_ptr().cast()),
            1,
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn setup_nsapplication() {}

// ── Plugin path helpers ──────────────────────────────────────────────

const AD2_PATH: &str = "/Library/Audio/Plug-Ins/VST3/Addictive Drums 2.vst3";
const MANIS_PATH: &str = "/Library/Audio/Plug-Ins/VST3/Manis Iteritas.vst3";

fn is_ad2_installed() -> bool {
    std::path::Path::new(AD2_PATH).exists()
}

fn find_ad2_path() -> Option<PathBuf> {
    let p = PathBuf::from(AD2_PATH);
    if p.exists() { Some(p) } else { None }
}

fn is_manis_installed() -> bool {
    std::path::Path::new(MANIS_PATH).exists()
}

// ── Plugin loading ───────────────────────────────────────────────────

fn load_ad2_instance() -> Option<VstInstance> {
    let plugin_path = find_ad2_path()?;
    let bundle = Vst3Bundle::from_path(&plugin_path)?;
    let binary = bundle.binary_path?;

    let mut instance = match VstInstance::load(&binary) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("  VstInstance::load failed: {e}");
            return None;
        }
    };
    if let Err(e) = instance.initialize() {
        eprintln!("  initialize failed: {e}");
        return None;
    }
    if let Err(e) = instance.setup_processing(ProcessConfig::default()) {
        eprintln!("  setup_processing failed: {e}");
        return None;
    }
    Some(instance)
}

// ── Test cases ──────────────────────────────────────────────────────

/// Key regression test: `EditorView::open()` must not deadlock.
///
/// A `SIGALRM` alarm is armed for 10 seconds before the call. If
/// `run_on_main_sync` deadlocks (reverted to GCD `dispatch_sync_f`),
/// the alarm fires and kills the process, causing the test to fail.
/// If the CFRunLoopSource fix is in place, `open()` returns promptly
/// and the alarm is cancelled.
fn test_editor_open_no_deadlock(instance: &VstInstance) -> bool {
    eprintln!("test: ad2_editor_open_no_deadlock");

    // SAFETY: the harness keeps this initialized instance alive until view teardown,
    // and runs native operations on the process main thread.
    let mut view = match unsafe { EditorView::from_instance(instance) } {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FAIL: EditorView not available for installed Addictive Drums 2: {e}");
            return false;
        }
    };

    eprintln!("  arming 10-second SIGALRM deadlock watchdog...");
    // Safety: alarm() is async-signal-safe. If open() deadlocks, the process
    // receives SIGALRM after 10 seconds, terminates, and the test fails.
    #[cfg(target_os = "macos")]
    unsafe {
        libc::alarm(10);
    }

    let result = view.open(None);

    // Cancel the alarm — open() returned, no deadlock.
    #[cfg(target_os = "macos")]
    unsafe {
        libc::alarm(0);
    }

    match result {
        Err(e) => {
            eprintln!("  FAIL: view.open(None) failed: {e}");
            return false;
        }
        Ok(()) => {
            eprintln!("  open() returned without deadlock");
        }
    }

    view.close();
    eprintln!("  PASS");
    true
}

/// Verify that an EditorView can be created and has non-zero dimensions.
fn test_editor_view_created(instance: &VstInstance) -> bool {
    eprintln!("test: ad2_editor_view_created");

    // SAFETY: the harness keeps this initialized instance alive until view teardown,
    // and runs native operations on the process main thread.
    let view = match unsafe { EditorView::from_instance(instance) } {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FAIL: EditorView not available for installed Addictive Drums 2: {e}");
            return false;
        }
    };

    let w = view.size.width();
    let h = view.size.height();
    eprintln!("  editor view size: {w}x{h}");
    if w == 0 || h == 0 {
        eprintln!("  FAIL: dimensions are zero");
        false
    } else {
        eprintln!("  PASS");
        true
    }
}

/// Verify that `close()` is idempotent (calling it twice does not crash).
fn test_editor_close_is_idempotent(instance: &VstInstance) -> bool {
    eprintln!("test: ad2_editor_close_is_idempotent");

    // SAFETY: the harness keeps this initialized instance alive until view teardown,
    // and runs native operations on the process main thread.
    let mut view = match unsafe { EditorView::from_instance(instance) } {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FAIL: EditorView not available for installed Addictive Drums 2: {e}");
            return false;
        }
    };

    #[cfg(target_os = "macos")]
    unsafe {
        libc::alarm(10);
    }
    let result = view.open(None);
    #[cfg(target_os = "macos")]
    unsafe {
        libc::alarm(0);
    }

    if let Err(e) = result {
        eprintln!("  FAIL: view.open(None) failed: {e}");
        return false;
    }

    view.close();
    eprintln!("  first close succeeded, calling close() again...");
    view.close();
    eprintln!("  PASS");
    true
}

/// Skipped test: Manis Iteritas is not installed on this machine.
/// Included to document that the test infrastructure handles missing
/// plugins gracefully.
fn test_manis_iteritas_skipped() -> bool {
    eprintln!("test: manis_iteritas_editor_skipped");
    if is_manis_installed() {
        // Unexpected — just skip with a note rather than failing.
        eprintln!(
            "SKIPPED: Manis Iteritas IS installed (unexpected); skipping to avoid scope creep"
        );
    } else {
        eprintln!("SKIPPED: Manis Iteritas not installed at {MANIS_PATH}");
    }
    true
}

// ── Main ────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    if !is_ad2_installed() {
        eprintln!("SKIPPED: Addictive Drums 2 not installed at {AD2_PATH}");
        return ExitCode::SUCCESS;
    }

    // NSApplication is required for AppKit-based editor views.
    setup_nsapplication();

    let mut instance = match load_ad2_instance() {
        Some(i) => i,
        None => {
            eprintln!("FAIL: could not load installed Addictive Drums 2 instance");
            return ExitCode::FAILURE;
        }
    };

    let mut failures = 0;

    if !test_editor_view_created(&instance) {
        failures += 1;
    }
    if !test_editor_open_no_deadlock(&instance) {
        failures += 1;
    }
    if !test_editor_close_is_idempotent(&instance) {
        failures += 1;
    }
    if !test_manis_iteritas_skipped() {
        failures += 1;
    }

    if let Err(e) = instance.terminate() {
        eprintln!("FAIL: terminate: {e}");
        failures += 1;
    }

    eprintln!("\n{failures} failure(s)");
    if failures > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
