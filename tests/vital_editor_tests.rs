//! Integration tests for VST3 editor lifecycle using real Vital plugin.
//!
//! Uses `harness = false` so `main()` runs on the process's main thread.
//! Vital requires the main thread for AppKit operations
//! like `createView` and `attached`.
//!
//! Run with: `cargo test -p plugin-hostkit --test vital_editor_tests`

mod common;
use common::{find_vital_path, is_vital_installed};
use plugin_hostkit::discovery::Vst3Bundle;
use plugin_hostkit::host::ProcessConfig;
use plugin_hostkit::{EditorView, VstInstance};

use std::process::ExitCode;

/// Load Vital and create an initialized VstInstance.
fn load_vital_instance() -> Option<VstInstance> {
    let vital_path = find_vital_path()?;
    let bundle = Vst3Bundle::from_path(&vital_path)?;
    let binary = bundle.binary_path?;

    let mut instance = match VstInstance::load(&binary) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("VstInstance::load failed: {e}");
            return None;
        }
    };
    if let Err(e) = instance.initialize() {
        eprintln!("initialize failed: {e}");
        return None;
    }
    if let Err(e) = instance.setup_processing(ProcessConfig::default()) {
        eprintln!("setup_processing failed: {e}");
        return None;
    }
    Some(instance)
}

// AppKit plugin editors initialize their renderer asynchronously. Drive the
// main run loop while the view is attached, as a real host event loop does.
#[cfg(target_os = "macos")]
fn service_editor_startup() {
    use std::ffi::c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;
    }
    unsafe {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.5, 0);
    }
}
#[cfg(not(target_os = "macos"))]
fn service_editor_startup() {
    std::thread::sleep(std::time::Duration::from_millis(500));
}

// ── Test cases ──────────────────────────────────────────────────────

fn test_editor_view_created(instance: &VstInstance) -> bool {
    eprintln!("test: vital_editor_view_created_from_instance");
    // SAFETY: the harness keeps this initialized instance alive until view teardown,
    // and runs native operations on the process main thread.
    let view = match unsafe { EditorView::from_instance(instance) } {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FAIL: EditorView not available for installed Vital: {e}");
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

fn test_editor_open_and_close(instance: &VstInstance) -> bool {
    eprintln!("test: vital_editor_open_and_close");
    // SAFETY: the harness keeps this initialized instance alive until view teardown,
    // and runs native operations on the process main thread.
    let mut view = match unsafe { EditorView::from_instance(instance) } {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FAIL: EditorView not available for installed Vital: {e}");
            return false;
        }
    };
    if let Err(e) = view.open(None) {
        eprintln!("  FAIL: view.open(None) failed: {e}");
        return false;
    }
    service_editor_startup();
    eprintln!("  editor opened successfully, closing...");
    view.close();
    eprintln!("  PASS");
    true
}

fn test_editor_close_is_idempotent(instance: &VstInstance) -> bool {
    eprintln!("test: vital_editor_close_is_idempotent");
    // SAFETY: the harness keeps this initialized instance alive until view teardown,
    // and runs native operations on the process main thread.
    let mut view = match unsafe { EditorView::from_instance(instance) } {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FAIL: EditorView not available for installed Vital: {e}");
            return false;
        }
    };
    if let Err(e) = view.open(None) {
        eprintln!("  FAIL: view.open(None) failed: {e}");
        return false;
    }
    service_editor_startup();
    view.close();
    eprintln!("  first close succeeded, calling close() again...");
    view.close();
    eprintln!("  PASS");
    true
}

fn test_editor_drop_closes_cleanly(instance: &VstInstance) -> bool {
    eprintln!("test: vital_editor_drop_closes_cleanly");
    // SAFETY: the harness keeps this initialized instance alive until view teardown,
    // and runs native operations on the process main thread.
    let mut view = match unsafe { EditorView::from_instance(instance) } {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FAIL: EditorView not available for installed Vital: {e}");
            return false;
        }
    };
    if let Err(e) = view.open(None) {
        eprintln!("  FAIL: view.open(None) failed: {e}");
        return false;
    }
    service_editor_startup();
    eprintln!("  dropping open EditorView without explicit close...");
    drop(view);
    eprintln!("  PASS");
    true
}

/// get_state → set_state roundtrip across two Vital instances.
/// Currently skipped: this harness does not validate a second instance.
fn test_state_roundtrip(_instance: &VstInstance) -> bool {
    eprintln!("test: vital_state_roundtrip_before_editor");
    eprintln!("SKIPPED: state roundtrip is not validated by this harness");
    true
}

// ── Main ────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    if !is_vital_installed() {
        eprintln!("SKIPPED: Vital not installed");
        return ExitCode::SUCCESS;
    }

    let mut instance = match load_vital_instance() {
        Some(i) => i,
        None => {
            eprintln!("FAIL: could not load installed Vital instance");
            return ExitCode::FAILURE;
        }
    };

    let mut failures = 0;

    if !test_editor_view_created(&instance) {
        failures += 1;
    }
    if !test_editor_open_and_close(&instance) {
        failures += 1;
    }
    if !test_editor_close_is_idempotent(&instance) {
        failures += 1;
    }
    if !test_editor_drop_closes_cleanly(&instance) {
        failures += 1;
    }
    if !test_state_roundtrip(&instance) {
        failures += 1;
    }

    if let Err(error) = instance.terminate() {
        eprintln!("FAIL: plugin termination failed: {error}");
        failures += 1;
    }

    eprintln!("\n{failures} failure(s)");
    if failures > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
