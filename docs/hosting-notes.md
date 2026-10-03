# Hosting notes

![Official compatible logo](../assets/compatible-logo.png)

The VST3 interface declarations follow the pinned Steinberg 3.8 MIT headers.
See [publication audit](publication-audit.md) for source and verification details.

## Lifecycle and ownership

Bundle initialization precedes the factory call. On macOS the entry point
receives a CFBundleRef; Linux ModuleEntry receives the loaded shared-library
handle. Component, processor, factory, controller, and view interface pointers
must remain valid while calls are in flight. Query interfaces through the SDK
contract; product-internal pointer arithmetic is deliberately excluded.

Initialize the component, find or create its controller, set the component
handler, connect separate peers, then synchronize component state. Unified
controllers share the component's lifecycle. Speaker arrangements and bus
activation precede processing. Main-thread dispatch preserves AppKit affinity
when an application event loop is available.

## Editors

Create a view through the controller, give it a host frame, and attach it to a
native child view. The visible parent window must exist before attachment so
GPU resources can initialize. Keep the event loop serviced while the plugin
starts asynchronous renderer work. Remove the view before destroying the
parent, and release interfaces before unloading their library.
Linux editors use X11 and host run-loop callbacks. Linux close callbacks and
Unicode text-input forwarding are not implemented. Windows editors are stubs.

## Crash boundaries

The scanner launches a helper for factory metadata rather than loading unknown
plugins in the caller. A failing or timed-out helper adds its binary path to
the persistent blacklist. Explicit option setters must run before first use.
Rendering remains in-process unless the application supplies process isolation.
Native faults are process-fatal. In-process signal jumps are not a recovery
boundary suitable for keeping a production process alive after memory faults.

## Presets and processing

Standard preset discovery is filesystem-based. The inherited parser handles
minimal state blobs; full SDK chunk-table parsing and IUnitInfo enumeration
are not implemented. Processing accepts note-on/off tuples with block sample
offsets and mutable channel slices. Use `primary_output_channels()` after setup for the negotiated channel count.
The crate rejects invalid lengths, block sizes, sample formats, and MIDI ranges
before processing audio; these checks cannot defend against a faulty plugin.
