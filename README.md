# plugin-hostkit

![Official VST Compatible logo](assets/compatible-logo.png)

Rust utilities for hosting VST3 plug-ins: discovery, crash-isolated metadata
scanning, component and controller lifecycle, state, and native editor views.
Extracted from the author's multisamples host, with application types and
product-specific preset scanners removed. MIT licensed; Rust 1.93 or newer.

VST is a registered trademark of Steinberg Media Technologies GmbH.
This project is not affiliated with or endorsed by Steinberg.

## Load and process

```sh
cargo run --example scan
cargo run --example load -- /path/to/Instrument.vst3
```

The lifecycle is `VstInstance::load` → `initialize` → `setup_processing` →
`activate` → `process` → `deactivate` → `terminate`. `ProcessConfig::new`
accepts the sample rate in hertz as `f64`. See [the load example](examples/load.rs).
`PluginDescriptor` exposes raw subcategories; `PluginKind::from_subcategories`
classifies instruments, effects, and other plugins. `PresetManager` discovers
standard preset files and stores caller-provided factory preset names; querying
factory programs is currently a stub. The preset parser preserves the original
simplified state extraction and does not implement full chunk-table parsing.

## Scanner helper

Build and install `plugin-scanner` beside your executable, or call
`scanner::set_scanner_binary(PathBuf)` before scanning. Blacklisted failures are
persisted under the platform's local data directory in
`plugin-hostkit/plugin_blacklist.json`. Call `scanner::set_blacklist_path(PathBuf)`
before the first scan to override that location. Both options are process-wide;
a late setter returns the rejected path. `clear_blacklist` permits rescanning.

Cargo does not build binaries from dependencies. A consuming workspace can
provide its own binary containing:

```rust
fn main() { plugin_hostkit::scanner_cli_main() }
```

See [the downstream wrapper](examples/custom_scanner.rs).

## Platform status

| Platform | Evidence |
| --- | --- |
| macOS | Original host used daily by the author; extracted crate tested locally with Vital. AppKit lifecycle and editor operations require the main thread and a serviced event loop. |
| Linux | Extracted crate tested locally on Linux x86_64 with the pinned SDK note-expression instrument, including finite, non-silent audio. CI repeats that mandatory fixture test. |
| Windows | Conditional implementation; not validated. Native editor hosting is not implemented. |

## Safety and limitations

Plugins process audio in your process and can crash, hang, or corrupt memory.
Metadata scanning uses a disposable helper with a timeout and persistent blacklist.
For render isolation, use a child process; see
[plugin-render-bridge](https://github.com/mattsp1290/plugin-render-bridge).
Native faults terminate the process; the host does not jump through Rust or
foreign frames to recover. `EditorView::from_instance` is unsafe: its instance
must stay alive and initialized until the editor is closed and dropped. Serialize
native UI operations on the main thread and keep its event loop serviced during
worker dispatch and native-owner cleanup. Allow asynchronous GUI startup to run before
closing a view, with the normal application event loop.

The current bus setup is instrument-oriented and accepts Float32 processing. Use
`primary_output_channels()` for the negotiated channel count; `bus_info()` reports
bus counts. Processing rejects mismatched buffers, oversized blocks, and invalid MIDI. Loading a plugin is not a claim
of full compatibility with every plugin or bus arrangement. Diagnostics remain
enabled through `tracing`; the main-thread freeze watchdog is macOS-only.
`watchdog::start_with_prefix` configures its diagnostic filenames.

## Development

```sh
cargo build --all-targets --locked
cargo test --locked -- --nocapture
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
cargo doc --no-deps
```

Plugin-dependent tests print `SKIPPED:` when their fixture is absent.
`VST3_TEST_PLUGIN` selects the instrument fixture; `REQUIRE_TEST_PLUGIN=1`
makes a missing fixture fatal. The fixture test sends a note and verifies
finite, non-silent output. Vital editor and signal tests use custom harnesses
to preserve main-thread and subprocess behavior.

See [hosting notes](docs/hosting-notes.md), [publication audit](docs/publication-audit.md),
and [third-party notices](THIRD-PARTY-NOTICES.md).
