# Publication audit

![Official compatible logo](../assets/compatible-logo.png)

Audit date: 2026-10-03. Scope: plugin-hostkit H1–H6 only.
Source snapshot: author's `multisamples` repository at
`a2207788282cf943d9d12cfeab63b4897af808b2`.
Decision owner: Matt Spurlin. Local L1–L5 checks passed as recorded below.
The first imported-code push still requires the owner's review and approval
of this document. Remote CI and tag evidence are recorded in the H6 Beans issue.

## L1 — SDK reference and ABI

Reference: [v3.8.0_build_66](https://github.com/steinbergmedia/vst3sdk/tree/9fad9770f2ae8542ab1a548a68c1ad1ac690abe0),
root commit `9fad9770f2ae8542ab1a548a68c1ad1ac690abe0`.
Both root `LICENSE.txt` and `pluginterfaces/LICENSE.txt` are MIT,
Copyright (c) 2025, Steinberg Media Technologies GmbH. The MIT permission
notice requires attribution and does not require signing an agreement.
The pinned README retains an earlier signed-agreement paragraph alongside
its MIT license section. This is a documentation discrepancy; this extraction
uses the explicit MIT license files as its license basis.
[Full notice](../THIRD-PARTY-NOTICES.md) is included.

Pinned submodules used as evidence:

| Reference | Commit |
| --- | --- |
| pluginterfaces | `31d6eeba6daaa3e2a8bfbe3e7a90ca0b7fbfbc1c` |
| public.sdk | `a3911a4615dabbfdfd9d181ee26b05c70c289a95` |
| doc / official logo | `6d4737c9e70750056e731d88d49aa06eefc8a1a4` |

The historical hosting code references a 3.8.0 SDK. The owner's reply,
“All sounds good,” confirms release permission and the copyright holder;
it does not unambiguously identify every historical SDK version. Earlier
reference versions remain unknown. Complete reverification against the pinned
MIT headers establishes the present interface basis independently of that history.

`python3 tools/verify_sdk_abi.py "$VST3_SDK_DIR"` passed on macOS arm64 and
Linux x86_64: 36 identifier occurrences, 32 vtable declarations, 233 method
ABI signatures, and 90 POD fields. It compiles both the Rust declarations and
C++ SDK declarations, compares sizes, alignments and every field offset,
and compares inherited method order and argument/return machine representations.
Opaque pointer target types were also inspected against the headers. The script
covers Unix 64-bit bindings; it does not establish Windows COM compatibility.
Existing HostContextObj sub-interface offset assertions still compile.

Interfaces checked (including duplicate declarations and the deliberate
headless IEditController prefix): FUnknown; IPluginFactory/2/3; IPluginBase;
IComponent; IAudioProcessor; IEditController/2; IEventList; IParamValueQueue;
IParameterChanges; IPlugView; IPlugFrame; IPlugViewContentScaleSupport;
IHostApplication; IPlugInterfaceSupport; IComponentHandler/2/3; IUnitHandler;
ChannelContext::IInfoListener; Linux::IRunLoop/IEventHandler/ITimerHandler;
IConnectionPoint; IMessage; IAttributeList; IBStream. Queried identifier-only
interfaces IUnitInfo, IUnitData, IProgramListData, IMidiMapping and
INoteExpressionController were verified too.

PODs checked: PFactoryInfo, both PClassInfo declarations, PClassInfo2,
ProcessSetup, AudioBusBuffers, ProcessData, ProcessContext, Chord, FrameRate,
NoteOnEvent, NoteOffEvent, Event (including its union), ParameterInfo and ViewRect.

The audit exposed inherited defects, so the plan's preservation invariant was
narrowly adjusted to preserve the SDK contract: IInfoListener, Linux IRunLoop,
INoteExpressionController identifiers; a supported-interface entry labeled
IConnectionPoint but containing IPluginBase; uint8 TBool declarations;
IComponentHandler3's ParamID pointer; and float32 content-scale factor.
No proprietary interfaces or object offsets are used. Linux ModuleEntry now
receives a loaded module handle, as specified by the SDK, rather than a filename.

## L2 — Provenance

The owner confirmed that tooling contributions identified as Infrastructure
Control Center and AI-assisted hosting code may be included in the MIT release,
and that Matt Spurlin is the MIT copyright holder (reply: “All sounds good”).
No JUCE, Ardour, Carla or VSTGUI implementation files were imported.

Comparison references: JUCE `501c07674e1ad693085a7e7c398f205c2677f5da`
and Ardour `7db8f45bf2042efdca43fe427b616603874c4b54`, inspected locally.
JUCE's relevant functions are in
`modules/juce_audio_processors_headless/format_types/juce_VST3PluginFormatImpl.h`;
Ardour's are in `libs/ardour/vst3_plugin.cc`. Public SDK comparisons use the
pinned MIT submodule above. Source line numbers below refer to the original
snapshot, not the subsequently reformatted public files.

| Original passages | Compared contract / function | Classification and action |
| --- | --- | --- |
| com:412; instance:12,13,16,17,26,211 | SDK processing structures, enums, factory context | SDK-derived declarations/constants; verified against MIT headers and covered by notices. |
| com:866,1741; instance:246,247,1364,1365,1380 | JUCE getFUnknown, controller initialise; SDK host-interface QI | behavior-only: interface identity and supported queries. Own Rust sub-interface offsets, refcount and callbacks. Neutral descriptions replace host comparisons. |
| com:1848 | SDK ConnectionProxy connect/disconnect/notify; JUCE interconnectComponentAndController | behavior-only: message forwarding. Rust proxy retains a parent for QI forwarding, manages targets/refcounts and diagnostics; it does not reproduce the SDK's source/destination/threadChecker implementation. The historical JUCE ConnectionProxy label is not present as a class in the inspected current host; label removed. |
| com:2495 | SDK MemoryStream::read | SDK-derived behavior, MIT: partial reads return success and report actual bytes. Rust Vec/saturating read implementation inspected; full SDK notice retained. |
| editor:145,147,192 | SDK IPlugFrame/FUnknown and host-interface delegation; JUCE host QI | behavior-only: frame queries forward to a host context. Own Rust frame object and field-offset recovery. Product names and comparison labels removed. |
| editor:367 | JUCE tryCreatingView; SDK createView/QI | behavior-only: three public-interface fallback calls. Own Rust error checks and diagnostics; neutral API description retained. |
| editor:522; lib:31 | SDK main-thread requirements | behavior-only: thread affinity. Framework-name comparison removed. |
| editor:593,744,775,2733; instance:145,1815 | SDK content scale, state and bundle lifecycle; observed native-view behavior | behavior-only: toolkit/resource timing. No framework source copied. Product-specific UI parameter manipulation removed; generic SDK state sync and native window attachment retained. |
| editor:918 | Generic framework-state comment in product-specific parameter manipulation | behavior-only comment; entire product-specific parameter manipulation removed under L3. No third-party implementation retained. |
| editor:2682,2827,2829 | JUCE GUI hosting, native parent/child NSView ownership | behavior-only: child view attachment. Own runtime Objective-C calls, class registration and event forwarding; comparison labels replaced by native-view explanation. |
| instance:333,335 | JUCE setBusArrangements call; Ardour request_bus_layout; SDK arrangements | behavior-only: input/output arrangement arrays. Quoted warning removed. Rust stereo-first/per-bus fallback implementation is distinct from both hosts' routing implementations. |
| instance:775 | JUCE destructor clearing component handler; SDK lifecycle | behavior-only: avoid callbacks to a released handler. Neutral ownership comment. |
| instance:981 | SDK ProcessContext; JUCE ProcessData | behavior-only: non-null timing data. Own transport data construction and flags; comparison removed. |
| instance:1387 | Ardour knob-mode switch; SDK IEditController2::setKnobMode | behavior-only: request linear mode through standard interface. No Ardour preference switch, configuration or parameter enumeration copied. File/line citation removed. |
| instance:1448,1461,1469 | JUCE grabInformationObjects; SDK queryInterface | behavior-only: component-first/controller fallback. Rust loops over standard IIDs, immediately releasing queried availability references, unlike JUCE's stored smart pointers. Comparison removed. |
| instance:1486,1487,1517 | JUCE interconnectComponentAndController; SDK connection contract | behavior-only: standard peer connection before optional proxy interposition. Own Rust branching, diagnostics, rollback and proxy ownership. Host labels removed. |
| instance:1648,1663,1667 | JUCE initialiseParameterList/synchroniseStates; SDK parameter/state methods | behavior-only: enumerate before state sync. Rust captures id/value tuples for first-block changes; it does not recreate JUCE's hosted parameter tree, maps or dispatcher. Neutral diagnostics. |
| instance:1841,1899 | JUCE module loader; native CFBundle API | behavior-only: use native bundle loading and supply its reference to entry. Own dynamically resolved CoreFoundation API calls and ownership; host labels removed. |
| scanner:199 | General crash blacklist behavior | behavior-only: skip known failed binaries. Own JSON persistence and configurable path; host comparison removed. |

All 49 original search hits are covered by these rows. There are zero retained
passages classified as derived from a non-MIT source. Comparing call order is
not an ownership claim for another host's expression. SDK-derived definitions
and stream behavior are specifically attributed. Remaining VSTGUI mentions
identify a toolkit behavior, with no toolkit implementation copied.

The original and final search ledgers follow. The original output is included
for provenance, not as a recommendation to use its historical claims.

Original allowlisted source: `rg -ni "juce|ardour|carla|vstgui|steinberg|hostclasses|public\.sdk"` (49 hits).

```text
com.rs:412:/// Layout must exactly match the C++ `Steinberg::Vst::ProcessContext` struct
com.rs:866:// JUCE/Ardour pattern where the same object is passed to both
com.rs:1741:    /// `IEditController::initialize` (JUCE passes IComponentHandler).
com.rs:1848:// Matches JUCE's ConnectionProxy pattern.
com.rs:2495:        // Steinberg's reference MemoryStream always returns kResultTrue from
editor.rs:145:/// Holds a pointer to the HostContextObj so that plugins (especially VSTGUI-based
editor.rs:147:/// host interfaces via `frame->queryInterface()`. This matches the JUCE/Ardour
editor.rs:192:        // IHostApplication, IPlugInterfaceSupport, etc. VSTGUI-based plugins
editor.rs:367:/// Try to create an IPlugView from the controller using the JUCE fallback chain:
editor.rs:522:        // main thread. Plugins using VSTGUI, JUCE, or other frameworks create AppKit
editor.rs:593:                        // scale factor. On Retina displays this is 2.0. VSTGUI
editor.rs:744:        // Some VSTGUI-based plugins (e.g. Serum 2) construct view-switch
editor.rs:775:            // Show the window BEFORE attached(). VSTGUI-based plugins (e.g. Serum 2)
editor.rs:918:            // The UI panel is driven by internal VSTGUI sub-controller state
editor.rs:2682:    /// rather than the window's contentView directly. This matches JUCE's hosting
editor.rs:2733:                        // for VSTGUI-based plugins during IPlugView::attached().
editor.rs:2827:                // Following JUCE's pattern: the plugin view attaches to a child
editor.rs:2829:                // proper view hierarchy behavior for VSTGUI-based plugins.
instance.rs:12:const K_AUDIO: i32 = 0; // Steinberg::Vst::MediaTypes::kAudio
instance.rs:13:const K_EVENT: i32 = 1; // Steinberg::Vst::MediaTypes::kEvent
instance.rs:16:const K_INPUT: i32 = 0;  // Steinberg::Vst::BusDirections::kInput
instance.rs:17:const K_OUTPUT: i32 = 1; // Steinberg::Vst::BusDirections::kOutput
instance.rs:26:const K_INFINITE_TAIL: u32 = 0xFFFF_FFFF; // Steinberg::Vst::kInfiniteTail
instance.rs:145:        // table. VSTGUI-based plugins (e.g. Serum 2) cache resource paths during
instance.rs:211:        // Try IPluginFactory3::setHostContext. Steinberg SDK plugins (e.g. Serum 2)
instance.rs:246:        // Pass IComponentHandler as FUnknown context (JUCE behavior).
instance.rs:247:        // JUCE's getFUnknown() returns static_cast<IComponentHandler*>(this),
instance.rs:333:    /// Both JUCE and Ardour follow this pattern.
instance.rs:335:    /// JUCE warns: "Some plug-ins will crash if you pass a nullptr to
instance.rs:775:                    // Null the handler first (JUCE pattern) so the plugin can't
instance.rs:981:        // Always provide ProcessContext — JUCE never passes null here.
instance.rs:1364:                // Pass IComponentHandler pointer as FUnknown context (JUCE behavior).
instance.rs:1365:                // JUCE's getFUnknown() returns static_cast<IComponentHandler*>(this),
instance.rs:1380:            // IPlugInterfaceSupport via queryInterface — matching JUCE/Ardour behavior.
instance.rs:1387:            // Matches Ardour's behavior (vst3_plugin.cc:1327-1342).
instance.rs:1448:            // Query standard interfaces from component and controller (JUCE's
instance.rs:1461:                // Query component first (JUCE pattern)
instance.rs:1469:                // Fallback to controller (JUCE pattern)
instance.rs:1486:            // Strategy (JUCE-style, direct-first):
instance.rs:1487:            // 1. Try direct component↔controller connection (like JUCE)
instance.rs:1517:                    // Step 1: Direct connection (JUCE pattern)
instance.rs:1648:            // Enumerate parameters BEFORE syncing state (matches JUCE's
instance.rs:1663:                    "pre-sync parameter enumeration (JUCE order)"
instance.rs:1667:            // Sync component state → controller (JUCE's synchroniseStates).
instance.rs:1815:/// methods run. VSTGUI-based plugins (e.g. Serum 2) look up their bundle during
instance.rs:1841:    // Use CFBundleLoadExecutableAndReturnError (preferred API, matches JUCE's approach)
instance.rs:1899:        // Load the bundle's executable using the preferred API (matches JUCE).
scanner.rs:199:// paths are skipped entirely. This matches JUCE/Ardour behavior.
lib.rs:31:/// VST3 plugins (especially JUCE-based ones) require their entire lifecycle —
```

Final `rg -ni "juce|ardour|carla|vstgui|steinberg|hostclasses|public\.sdk" src`:

```text
src/com.rs:400:/// Layout must exactly match the C++ `Steinberg::Vst::ProcessContext` struct
src/com.rs:2648:        // Steinberg's reference MemoryStream always returns kResultTrue from
src/editor.rs:144:/// Holds a pointer to the HostContextObj so that plugins (especially VSTGUI-based
src/editor.rs:190:        // IHostApplication, IPlugInterfaceSupport, etc. VSTGUI-based plugins
src/editor.rs:597:                        // scale factor. On Retina displays this is 2.0. VSTGUI
src/editor.rs:796:            // Show the window BEFORE attached(). VSTGUI-based plugins
src/editor.rs:2713:                        // for VSTGUI-based plugins during IPlugView::attached().
src/editor.rs:2814:                // proper view hierarchy behavior for VSTGUI-based plugins.
src/instance.rs:24:const K_AUDIO: i32 = 0; // Steinberg::Vst::MediaTypes::kAudio
src/instance.rs:25:const K_EVENT: i32 = 1; // Steinberg::Vst::MediaTypes::kEvent
src/instance.rs:28:const K_INPUT: i32 = 0; // Steinberg::Vst::BusDirections::kInput
src/instance.rs:29:const K_OUTPUT: i32 = 1; // Steinberg::Vst::BusDirections::kOutput
src/instance.rs:38:const K_INFINITE_TAIL: u32 = 0xFFFF_FFFF; // Steinberg::Vst::kInfiniteTail
src/instance.rs:157:        // table. VSTGUI-based plugins cache resource paths during
src/instance.rs:223:        // Try IPluginFactory3::setHostContext. Steinberg SDK plugins
src/instance.rs:1910:/// methods run. VSTGUI-based plugins look up their bundle during
```

## L3 — Allowlist and proprietary content

Imports were limited to `crates/ms-vst3/src/{com,discovery,editor,error,factory,
host,instance,preset,scanner,watchdog,lib}.rs` and the scanner binary body
(now `src/cli.rs` with a one-line `src/bin/plugin_scanner.rs`). The three
product preset modules and all source scripts, application code, resources,
binaries and presets were excluded. Tests follow H4's explicit subsets;
`tests/common` contains only its four requested helpers and their private path
helpers. Fixtures are synthesized by test code. The SDK instrument is built
outside the repository and is never packaged.

The original product-name ledger below records locations and matched product
names without reproducing removed internal/disassembly detail. Public source
comments were neutralized; product-specific pointer adjustment, object-memory
patching, a custom private message helper, diagnostic object-offset probes, and
UI parameter manipulation were removed. No proprietary internal class names,
resource extraction or disassembly narratives remain. Source unit-test bundle
names were changed to generic fixture names.

Retained product names in tests identify optional installed fixtures, search
paths, test labels or public metadata assertions: Vital, Addictive Drums 2 and
Manis Iteritas. These are interoperability test facts. Manis is explicitly
skipped even if installed; its editor lifecycle is not claimed as coverage.
Vital state roundtrip is also explicitly skipped. No commercial fixtures are
copied. `docs/hosting-notes.md` was rewritten from relevant lifecycle and safety
notes; the private document's product-specific, application and reverse
engineering sections were excluded.

Original product-name locations (67; whole-word matching removes false positives such as “invariant”):

```text
com.rs:1882: addictive, chipsynth
com.rs:1951: addictive, chipsynth
com.rs:2360: aria
com.rs:2362: aria
com.rs:2364: aria
com.rs:2371: aria
com.rs:2498: omnisphere
editor.rs:146: serum
editor.rs:193: serum
editor.rs:229: serum
editor.rs:498: vital
editor.rs:744: serum
editor.rs:775: serum
editor.rs:781: serum
editor.rs:794: serum
editor.rs:859: serum
editor.rs:860: serum
editor.rs:912: serum
editor.rs:916: serum
editor.rs:922: serum
editor.rs:945: serum
editor.rs:946: serum
editor.rs:1183: chipsynth
editor.rs:2712: serum
instance.rs:145: serum
instance.rs:211: serum
instance.rs:274: serum
instance.rs:640: serum
instance.rs:982: serum
instance.rs:998: serum
instance.rs:1119: omnisphere
instance.rs:1140: omnisphere
instance.rs:1210: omnisphere
instance.rs:1367: omnisphere
instance.rs:1491: aria, chipsynth
instance.rs:1518: aria
instance.rs:1521: aria
instance.rs:1522: aria
instance.rs:1527: aria
instance.rs:1529: aria, chipsynth, serum, spire
instance.rs:1539: aria
instance.rs:1545: chipsynth
instance.rs:1546: chipsynth
instance.rs:1547: aria
instance.rs:1549: spire
instance.rs:1550: serum
instance.rs:1558: aria
instance.rs:1561: aria
instance.rs:1574: aria
instance.rs:1815: serum
instance.rs:2113: omnisphere
preset.rs:280: serum
preset.rs:281: serum
preset.rs:283: kontakt
preset.rs:284: kontakt
preset.rs:286: vital
preset.rs:287: vital
scanner.rs:597: vital
scanner.rs:598: vital
scanner.rs:599: vital
scanner.rs:602: vital
scanner.rs:603: vital
scanner.rs:716: serum
scanner.rs:723: serum
lib.rs:10: kontakt
lib.rs:13: serum
lib.rs:14: vital
```

`git log --all --name-only --format=` at the scaffold history listed only:
`.github/workflows/ci.yml`, `.gitignore`, Cargo manifest/lockfile, LICENSE,
README, `src/lib.rs`, `docs/publication-audit.md`. The imported commit includes only the reviewed package file list. The full
path-history check is repeated after committing and before publication; it
must contain no path
matching `serum2_uidesc|\.fxp$|\.nki$|\.vital$|\.vstpreset$|\.vst3/`.

## L4 — Trademark

The unchanged official asset is
[Steinberg's VST Compatible logo](https://github.com/steinbergmedia/vst3_doc/blob/6d4737c9e70750056e731d88d49aa06eefc8a1a4/artwork/VST_Compatible_Logo_Steinberg_with_TM.png).
Its SHA-256 equals the upstream asset:
`fdf4f96b7a8bc1f53f0d2c167a27ac5fac00f29ff4a7bc3eaad851ad4be10f35`.
[Official usage guidelines](https://steinbergmedia.github.io/vst3_dev_portal/pages/VST%2B3%2BLicensing/Usage%2Bguidelines.html)
govern this trademark asset; the project's MIT license does not relicense it.
The official logo appears beside the first format reference in the README
and on the notices, hosting notes and this audit page. Product, repository,
crate and helper-binary names do not contain the trademark. API format names
remain descriptive. The binding definitions were verified using the SDK as
recorded in L1. README and notices contain the required trademark attribution
and nonendorsement statement. The stylized variant prohibited by the plan
has no hits outside this audit.

## L5 — Package, dependencies and secrets

- `cargo package --list --allow-dirty`: only the manifest/lockfile, license,
  README/notices, official logo, source, reviewed tests, three examples,
  hosting/audit documentation, two workflows, ABI verification tool and Cargo
  metadata. No target artifacts, SDK checkout, plugin binaries, rendered audio,
  preset files, review artifacts or source application directories.
- `cargo tree -e normal`: no application `ms-*` or `lotel-*` crates, Tauri or Specta.
- `cargo license --json`: 53 packages across all lockfile targets/dev dependencies;
  every expression has MIT, Apache-2.0, ISC or Unlicense as a permissive option.
  `unicode-ident` also requires Unicode-3.0. The LGPL alternative in `r-efi`
  is not selected; MIT is available. No copyleft-only dependency remains.
- Source, tests, examples, workflows and documentation searches for credentials,
  API keys, tokens, passwords and service identifiers found no secret values.
  The local-path/application search finds only the README's provenance sentence
  and this audit's source references/command descriptions. No personal local
  path or source application's runtime identifier is embedded in code.
- The only scaffolding dependency deviation is `dirs-next` 2.0.0 aliased as
  `dirs`, retaining the needed local-data-directory API. Original dirs 6.0.0
  introduced copyleft-only option-ext through dirs-sys. Replacing it was
  necessary to satisfy L5; no API option was added for licensing purposes.

Scaffold incident record: `9527029969dec50cb600eae7423ea62ce2e3e437` was
published with an incorrect all-permissive audit statement. No imported code
was present. `7df2014` corrected the record publicly, and the owner was informed.
The current dependency graph resolves the failure; published history was not
rewritten. Scaffold CI run `37151752410` passed both platform jobs.

## Validation evidence before publication

Local macOS arm64: build all targets, complete tests, strict clippy, formatting,
rustdoc with warnings denied and diff whitespace checks passed. A repeat run
exposed an asynchronous teardown race after the initial success. The host now
tracks successful attachment, does not remove an unattached view, clears host
frame callbacks at teardown, and retains the native child hierarchy through
removed(). Five consecutive fresh-process editor suites passed after this fix,
followed by another complete suite. Native geometry calls also now pass actual
C-layout aggregates, preserving the arm64 and x86_64 argument ABI. Real installed
fixture coverage: three Vital scanner cases, four Vital editor cases, two Vital
lifecycle/signal-handler cases and three Addictive Drums editor cases. The
editor harnesses explicitly terminate instances before process exit; Vital
startup is serviced through the main run loop before closing its asynchronous
renderer. Crash reports were used locally; no crash dumps are
included in this repository.

Linux x86_64, Rust 1.93.1 in a local container: build all targets, complete tests,
strict clippy and the same ABI verifier passed. Without installed fixtures,
plugin-dependent targets report `SKIPPED:`. The separate required SDK target
reports exactly `1 passed; 0 failed; 0 ignored` with zero skipped cases,
verifies finite/non-silent note output and rejects invalid process buffers.
The load example prints `note-expression-synth`, reports first-block peak
0.120571926, and exits zero. The local Vital example also exits zero, with
first-block peak 0.34679246.

Reproducible SDK build (checkout root SHA and recursive submodules from L1):

```sh
cmake -S "$VST3_SDK_DIR" -B "$SDK_BUILD_DIR" -DCMAKE_BUILD_TYPE=Release \
  -DSMTG_ENABLE_VST3_HOSTING_EXAMPLES=OFF -DSMTG_CREATE_PLUGIN_LINK=OFF \
  -DSMTG_RUN_VST_VALIDATOR=OFF
cmake --build "$SDK_BUILD_DIR" --target note-expression-synth -j2
export VST3_TEST_PLUGIN="$SDK_BUILD_DIR/VST3/Release/note-expression-synth.vst3"
REQUIRE_TEST_PLUGIN=1 cargo test --locked --test fixture_plugin_tests -- --nocapture
cargo run --locked --example load -- "$VST3_TEST_PLUGIN"
```

`.github/workflows/integration.yml` installs the SDK's build dependencies,
checks out the immutable SDK root and submodules, builds that instrument,
verifies the bindings, requires the fixture, checks its executed-test count
and absence of skip lines, and runs the load example. It runs on pushes,
pull requests and manual dispatch, including release tags. CI is still a
publication checkpoint to verify on the exact release commit; local container
evidence is not described as remote CI success.

## Standard review safety corrections

Owner approval of this completed audit and the public push was recorded on
2026-10-03 before any imported-code publication. The standard dual review found
host-owned defects that were corrected locally before that push:

- Callback ring readers/writers now use bounded nonblocking slot locks; Linux
  run-loop snapshots own COM handler references through reentrant unregistration.
  Registrations are cleared before module teardown.
- Rust setjmp/process-wide signal recovery was removed: native faults are
  process-fatal and isolation belongs in disposable children. This corrects an
  undersized Linux ABI declaration, cross-thread jump risk and compiler
  returns-twice assumptions without changing any SDK interface layout.
- `EditorView::from_instance` is now unsafe with an owner/state/UI-thread
  contract and runtime state checks. Mutable audio processing remains possible;
  callers must drop the editor before terminating its instance.
- Cocoa allocation ownership is balanced for window, view and delegate, with
  RAII cleanup on construction failure and deferred cleanup during user close.
  A local instrumented copy of the exact implementation used Objective-C weak
  references to confirm deallocation after normal close, user close and an injected
  partial-construction failure. The generic
  main-thread helper rejects unavailable worker dispatch instead of running
  AppKit work on a worker. Its availability query uses CoreFoundation's
  [current-mode API](https://developer.apple.com/documentation/corefoundation/cfrunloopcopycurrentmode%28_%3A%29).
- Connection cleanup retains exact queried peers, disconnects direct fallbacks
  and rolls back partially successful connections. Standard metadata arrays,
  bounded scanner output capture and cyclic directory handling were corrected.
  Regressions exercise these paths; preset discovery coverage calls its actual
  production walker.

After these corrections, local macOS and Linux gates passed again: 42 unit tests
on each platform, 14 synthetic discovery/preset cases, two scanner CLI/options
cases and one compile doctest (59 non-plugin cases). macOS also executed the
12 installed Vital/Addictive Drums cases listed above. Linux executed the
mandatory SDK lifecycle/audio test separately with no skipped cases. The same
36 IID occurrences, 32 vtables, 233 method signatures and 90 POD fields pass
SDK verification. The native ownership probe passed all three cleanup paths;
a process-wide leaks run additionally reported system AppIntents XPC cycles,
so it is not claimed as a whole-process leak-free result.

## Committed path-history verification

`git log --all --name-only --format=` passed the prohibited-path check after
the local implementation commit. Its unique nonempty output paths are:

```text
.github/workflows/ci.yml
.github/workflows/integration.yml
.gitignore
Cargo.lock
Cargo.toml
LICENSE
README.md
THIRD-PARTY-NOTICES.md
assets/compatible-logo.png
docs/hosting-notes.md
docs/publication-audit.md
examples/custom_scanner.rs
examples/load.rs
examples/scan.rs
src/bin/plugin_scanner.rs
src/cli.rs
src/com.rs
src/discovery.rs
src/editor.rs
src/error.rs
src/factory.rs
src/host.rs
src/instance.rs
src/lib.rs
src/preset.rs
src/scanner.rs
src/watchdog.rs
tests/common/mod.rs
tests/deadlock_editor_tests.rs
tests/discovery_preset_tests.rs
tests/discovery_tests.rs
tests/fixture_plugin_tests.rs
tests/preset_tests.rs
tests/scanner_cli_tests.rs
tests/scanner_options_tests.rs
tests/sigsegv_guard_tests.rs
tests/vital_editor_tests.rs
tests/vital_integration_tests.rs
tools/verify_sdk_abi.py
```
