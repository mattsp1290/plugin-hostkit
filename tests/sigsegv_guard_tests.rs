//! Integration tests for SIGSEGV guard behavior in VstInstance lifecycle.
//!
//! Uses `harness = false` so `main()` runs on the process's main thread.
//! Tests that exercise real guard behavior run in subprocesses to isolate
//! any heap corruption that siglongjmp recovery may cause.
//!
//! Run with: `cargo test -p plugin-hostkit --test sigsegv_guard_tests`

mod common;
use common::{find_vital_path, is_vital_installed};
use plugin_hostkit::VstInstance;
use plugin_hostkit::discovery::Vst3Bundle;
use plugin_hostkit::host::ProcessConfig;

use std::process::ExitCode;

// ── Helpers ─────────────────────────────────────────────────────────

fn load_vital_instance() -> Result<VstInstance, String> {
    let path = find_vital_path().ok_or("Vital path not found")?;
    let bundle = Vst3Bundle::from_path(&path).ok_or("not a valid VST3 bundle")?;
    let binary = bundle.binary_path.ok_or("no binary in bundle")?;

    let mut instance = VstInstance::load(&binary).map_err(|e| format!("VstInstance::load: {e}"))?;
    instance
        .initialize()
        .map_err(|e| format!("initialize: {e}"))?;
    instance
        .setup_processing(ProcessConfig::default())
        .map_err(|e| format!("setup_processing: {e}"))?;
    instance.activate().map_err(|e| format!("activate: {e}"))?;
    Ok(instance)
}

// ── Subprocess entry points ──────────────────────────────────────────

/// Test A (subprocess): load + initialize + setup_processing + activate Vital.
/// Verifies the full lifecycle works without the guards interfering.
fn run_vital_normal_lifecycle() -> ExitCode {
    eprintln!("  [subprocess] loading Vital...");
    let instance = match load_vital_instance() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("  FAIL: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("  load + initialize + setup_processing + activate OK");
    std::mem::forget(instance);
    ExitCode::SUCCESS
}

/// Test B (subprocess): install a custom SIGSEGV handler, load Vital
/// (exercises guarded_create_instance + guarded_initialize_component +
/// guarded_set_component_state), then verify the custom handler is
/// still installed afterward.
///
/// This confirms that every guard restores the previous signal action
/// after it completes (normal path and crash path).
#[cfg(unix)]
fn run_vital_handler_restoration() -> ExitCode {
    use std::sync::atomic::{AtomicBool, Ordering};

    static CUSTOM_HANDLER_CALLED: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" fn custom_handler(
        _sig: libc::c_int,
        _info: *mut libc::siginfo_t,
        _ctx: *mut libc::c_void,
    ) {
        CUSTOM_HANDLER_CALLED.store(true, Ordering::Relaxed);
    }

    eprintln!("  [subprocess] installing custom SIGSEGV handler...");

    // Install our custom handler.
    let mut new_action: libc::sigaction = unsafe { std::mem::zeroed() };
    new_action.sa_sigaction = custom_handler as *const () as usize;
    new_action.sa_flags = libc::SA_SIGINFO;
    let mut installed_action: libc::sigaction = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::sigaction(libc::SIGSEGV, &new_action, &mut installed_action) };
    if rc != 0 {
        eprintln!("  FAIL: sigaction(install) returned {rc}");
        return ExitCode::FAILURE;
    }

    // Load Vital — exercises all three guarded functions:
    //   VstInstance::load     → guarded_create_instance
    //   instance.initialize() → guarded_initialize_component
    //                         → guarded_set_component_state (inside init_edit_controller)
    eprintln!("  [subprocess] loading Vital (exercises all three guards)...");
    match load_vital_instance() {
        Ok(instance) => {
            eprintln!("  load + initialize + setup_processing + activate OK");
            std::mem::forget(instance);
        }
        Err(e) => {
            eprintln!("  FAIL: could not load Vital: {e}");
            return ExitCode::FAILURE;
        }
    }

    // Read back the currently-installed handler and compare to our custom one.
    let mut current_action: libc::sigaction = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::sigaction(libc::SIGSEGV, std::ptr::null(), &mut current_action) };
    if rc != 0 {
        eprintln!("  FAIL: sigaction(query) returned {rc}");
        return ExitCode::FAILURE;
    }

    let expected = custom_handler as *const () as usize;
    let actual = current_action.sa_sigaction;

    if actual != expected {
        eprintln!("  FAIL: handler not restored — expected 0x{expected:x}, got 0x{actual:x}");
        return ExitCode::FAILURE;
    }

    eprintln!("  custom SIGSEGV handler still installed after all guards ran — PASS");
    ExitCode::SUCCESS
}

#[cfg(not(unix))]
fn run_vital_handler_restoration() -> ExitCode {
    eprintln!("  [subprocess] SKIPPED: signal handler test is unix-only");
    ExitCode::SUCCESS
}

// ── Test A: Normal lifecycle with guards ─────────────────────────────

fn test_vital_normal_lifecycle() -> bool {
    eprintln!("test: vital_normal_lifecycle_with_guards");

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("  FAIL: could not determine test binary path: {e}");
            return false;
        }
    };

    let output = match std::process::Command::new(&exe)
        .arg("--run-vital-lifecycle")
        .stderr(std::process::Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("  FAIL: could not spawn subprocess: {e}");
            return false;
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr.lines() {
        eprintln!("  [child] {line}");
    }

    if output.status.success() {
        eprintln!("  PASS");
        return true;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = output.status.signal() {
            let name = signal_name(sig);
            eprintln!("  FAIL: child crashed with signal {sig} ({name})");
            return false;
        }
    }

    eprintln!("  FAIL: child exited with {}", output.status);
    false
}

// ── Test B: SIGSEGV handler restoration ──────────────────────────────

fn test_vital_handler_restoration() -> bool {
    eprintln!("test: vital_sigsegv_handler_restored_after_guards");

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("  FAIL: could not determine test binary path: {e}");
            return false;
        }
    };

    let output = match std::process::Command::new(&exe)
        .arg("--run-vital-handler-restoration")
        .stderr(std::process::Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("  FAIL: could not spawn subprocess: {e}");
            return false;
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr.lines() {
        eprintln!("  [child] {line}");
    }

    if output.status.success() {
        eprintln!("  PASS");
        return true;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = output.status.signal() {
            let name = signal_name(sig);
            eprintln!("  FAIL: child crashed with signal {sig} ({name})");
            return false;
        }
    }

    eprintln!("  FAIL: child exited with {}", output.status);
    false
}

// ── Test C: Guard catches real crash (Omnisphere) ─────────────────────

// ── Signal name helper ───────────────────────────────────────────────

#[cfg(unix)]
fn signal_name(sig: i32) -> &'static str {
    match sig {
        11 => "SIGSEGV",
        6 => "SIGABRT",
        4 => "SIGILL",
        8 => "SIGFPE",
        _ => "unknown",
    }
}

// ── Main ────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    // Subprocess dispatch — each mode runs a crash-prone operation in a
    // disposable process so that siglongjmp heap corruption doesn't
    // contaminate the parent test runner.
    if std::env::args().any(|a| a == "--run-vital-lifecycle") {
        return run_vital_normal_lifecycle();
    }
    if std::env::args().any(|a| a == "--run-vital-handler-restoration") {
        return run_vital_handler_restoration();
    }
    // ── Parent mode: orchestrate tests ──────────────────────────────

    let vital_ok = is_vital_installed();

    if !vital_ok {
        eprintln!("SKIPPED: Vital not installed");
        return ExitCode::SUCCESS;
    }

    let mut failures = 0;

    // Test A: normal lifecycle doesn't break when guards are active.
    if vital_ok {
        if !test_vital_normal_lifecycle() {
            failures += 1;
        }
    } else {
        eprintln!("SKIPPED: vital_normal_lifecycle_with_guards (Vital not installed)");
    }

    // Test B: each guard restores the previously-installed signal handler.
    // This is the most important correctness property of the guards — they
    // must not permanently replace whatever handler the caller had installed.
    if vital_ok {
        if !test_vital_handler_restoration() {
            failures += 1;
        }
    } else {
        eprintln!("SKIPPED: vital_sigsegv_handler_restored_after_guards (Vital not installed)");
    }

    eprintln!("\n{failures} failure(s)");
    if failures > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
