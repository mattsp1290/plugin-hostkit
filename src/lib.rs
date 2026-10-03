//! Native plugin discovery, isolated metadata scanning, and hosting.
//!
//! Lifecycle: load → initialize → setup_processing → activate → process →
//! deactivate → terminate. Processing and native editors run in-process;
//! applications should isolate untrusted plugins in a child process.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use plugin_hostkit::{ProcessConfig, VstInstance};
//! let mut plugin = VstInstance::load(std::path::Path::new("instrument.so"))?;
//! plugin.initialize()?;
//! plugin.setup_processing(ProcessConfig::new(44_100.0, 512))?;
//! plugin.activate()?;
//! let mut channels = vec![vec![0.0; 512]; plugin.primary_output_channels()?];
//! let mut output: Vec<_> = channels.iter_mut().map(Vec::as_mut_slice).collect();
//! plugin.process(&[(0, 60, 100, 0)], &mut output)?;
//! plugin.deactivate()?;
//! plugin.terminate()?;
//! # Ok(()) }
//! ```

pub(crate) mod com;
pub mod discovery;
pub mod editor;
pub mod error;
pub mod factory;
pub mod host;
pub mod instance;
pub mod preset;
pub mod scanner;
pub mod watchdog;

pub use discovery::Vst3Bundle;
pub use editor::{EditorView, mark_as_child_process};
pub use error::Vst3Error;
pub use host::{ProcessConfig, ProcessMode};
pub use instance::VstInstance;
pub use preset::{PresetEntry, PresetOrigin};
pub use scanner::{PluginDescriptor, PluginKind, PluginScanner};
pub mod cli;
pub use cli::scanner_cli_main;

/// Execute a closure on the macOS main thread (dispatch_sync_f).
///
/// On non-macOS platforms, runs the closure directly on the current thread.
/// On macOS, falls back to direct execution if no NSApplication is running
/// (test/CLI context) or if already on the main thread.
///
/// VST3 plugins require their entire lifecycle —
/// load, initialize, editor creation — to happen on the main thread.
#[cfg(target_os = "macos")]
pub fn run_on_main_sync<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send,
    R: Send,
{
    editor::cocoa_run_on_main_sync(f)
}

/// No-op passthrough on non-macOS platforms.
#[cfg(not(target_os = "macos"))]
pub fn run_on_main_sync<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send,
    R: Send,
{
    f()
}

/// Check if we're running in an application context (NSApplication on macOS).
///
/// Returns `true` if the process has a running event loop that can service
/// AppKit operations like `IPlugView::attached()`. Returns `false` in test
/// processes and CLI tools.
#[cfg(target_os = "macos")]
pub fn is_app_context() -> bool {
    editor::cocoa_is_app_context()
}

/// Non-macOS: always false (no AppKit).
#[cfg(not(target_os = "macos"))]
pub fn is_app_context() -> bool {
    false
}
