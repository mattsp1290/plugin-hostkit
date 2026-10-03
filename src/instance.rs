use std::os::raw::c_void;
use std::path::Path;

use crate::com::{
    self, AudioBusBuffers, EventListObj, HostContextObj, IAudioProcessorVtbl, IComponentVtbl,
    IID_IAUDIO_PROCESSOR, IID_ICOMPONENT, IID_ICONNECTION_POINT, IID_IEDIT_CONTROLLER,
    IPluginFactoryVtbl, K_NOT_IMPLEMENTED, K_RESULT_FALSE, K_RESULT_OK, PClassInfo, ParameterInfo,
    ProcessContext, ProcessData, ProcessSetup, TUID,
};

// VST3 bus media types (from ivstcomponent.h)
const K_AUDIO: i32 = 0; // Steinberg::Vst::MediaTypes::kAudio
const K_EVENT: i32 = 1; // Steinberg::Vst::MediaTypes::kEvent

// VST3 bus directions (from ivstcomponent.h)
const K_INPUT: i32 = 0; // Steinberg::Vst::BusDirections::kInput
const K_OUTPUT: i32 = 1; // Steinberg::Vst::BusDirections::kOutput

// VST3 speaker arrangement (from vstspeaker.h)
const K_SPEAKER_STEREO: u64 = 3; // kSpeakerL | kSpeakerR

// VST3 silence flags (from ivstaudioprocessor.h)
const K_ALL_CHANNELS_SILENT: u64 = u64::MAX;

// VST3 tail length sentinel (from ivstaudioprocessor.h)
const K_INFINITE_TAIL: u32 = 0xFFFF_FFFF; // Steinberg::Vst::kInfiniteTail

/// IConnectionPoint vtable: 3 FUnknown + 3 methods.
#[repr(C)]
struct IConnectionPointVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    connect: unsafe extern "C" fn(*mut c_void, other: *mut c_void) -> i32,
    disconnect: unsafe extern "C" fn(*mut c_void, other: *mut c_void) -> i32,
    notify: unsafe extern "C" fn(*mut c_void, message: *mut c_void) -> i32,
}

/// IEditController vtable for headless hosting (no view creation needed).
/// Layout: 3 FUnknown + 2 IPluginBase + 12 IEditController methods.
/// Note: createView (slot 18) is intentionally omitted — not needed for offline rendering.
#[repr(C)]
struct IEditControllerVtblHeadless {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    initialize: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32,
    terminate: unsafe extern "C" fn(*mut c_void) -> i32,
    set_component_state: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32,
    set_state: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32,
    get_state: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32,
    get_parameter_count: unsafe extern "C" fn(*mut c_void) -> i32,
    get_parameter_info: unsafe extern "C" fn(*mut c_void, i32, *mut c_void) -> i32,
    get_param_string_by_value: unsafe extern "C" fn(*mut c_void, u32, f64, *mut u16) -> i32,
    get_param_value_by_string: unsafe extern "C" fn(*mut c_void, u32, *const u16, *mut f64) -> i32,
    normalized_param_to_plain: unsafe extern "C" fn(*mut c_void, u32, f64) -> f64,
    plain_param_to_normalized: unsafe extern "C" fn(*mut c_void, u32, f64) -> f64,
    get_param_normalized: unsafe extern "C" fn(*mut c_void, u32) -> f64,
    set_param_normalized: unsafe extern "C" fn(*mut c_void, u32, f64) -> i32,
    set_component_handler: unsafe extern "C" fn(*mut c_void, handler: *mut c_void) -> i32,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    std::mem::size_of::<IEditControllerVtblHeadless>() == 17 * 8,
    "IEditControllerVtblHeadless should be 17 pointers"
);

use crate::error::Vst3Error;
use crate::host::{BusInfo, ProcessConfig, SymbolicSampleSize};

/// RAII wrapper for a VST3 plugin instance.
///
/// Lifecycle: load -> initialize -> setup_processing -> activate -> process -> deactivate -> terminate
///
/// This type is `Send` but NOT `Sync` — a single instance should only be
/// accessed from one thread at a time (typical for VST3 offline rendering).
pub struct VstInstance {
    name: String,
    _library: libloading::Library,
    config: ProcessConfig,
    state: InstanceState,
    bus_info: BusInfo,
    // COM interface pointers
    component: *mut c_void,
    processor: *mut c_void,
    component_vtbl: *const IComponentVtbl,
    processor_vtbl: *const IAudioProcessorVtbl,
    factory: *mut c_void,
    factory_vtbl: *const IPluginFactoryVtbl,
    // Edit controller (kept alive for IConnectionPoint communication)
    controller: *mut c_void,
    controller_is_separate: bool,
    // IConnectionPoint proxies (only for separate controllers).
    // Stored here so they're released on terminate. Null when not used.
    cp_comp: *mut c_void, // owned queried connection point while connected
    cp_ctrl: *mut c_void,
    cp_proxy_comp: *mut c_void, // proxy that component connects to (forwards to ctrl)
    cp_proxy_ctrl: *mut c_void, // proxy that controller connects to (forwards to comp)
    /// Initial parameter changes to inject into the first process() call.
    initial_params: Option<Vec<(u32, f64)>>,
    // Transport context for tempo-synced plugins
    tempo_bpm: Option<f64>,
    position_samples: i64,
    // Plugin-reported tail length (samples after note-off before silence).
    // None means infinite tail (kInfiniteTail = 0xFFFFFFFF).
    tail_samples: Option<u32>,
    // Plugin-reported latency (samples of internal delay).
    // Used to trim the start of rendered audio.
    latency_samples: u32,
    // Host context must outlive the plugin — VST3 spec requires it.
    // Box-allocated so the address is stable across VstInstance moves
    // (the factory and component may retain the pointer).
    host_context: Box<HostContextObj>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstanceState {
    Loaded,
    Initialized,
    SetupDone,
    Active,
    Terminated,
}

// SAFETY: VstInstance is Send because:
// - The COM pointers (component, processor) are exclusively owned: created in
//   create_instance() via the plugin factory, never shared with any other code,
//   and released in terminate()/Drop.
// - All mutation goes through &mut self methods, preventing concurrent access.
// - The library handle (_library) keeps the shared library mapped and is also
//   exclusively owned.
// VstInstance is NOT Sync — concurrent access from multiple threads would be
// unsound because the COM methods are not thread-safe.
unsafe impl Send for VstInstance {}

impl VstInstance {
    /// Load a VST3 plugin from its shared library path.
    ///
    /// This loads the dynamic library, finds the first Audio Module Class,
    /// and creates IComponent + IAudioProcessor instances via the factory.
    /// Call `initialize()` next.
    #[tracing::instrument(name = "vst3.load", skip_all, fields(path = %library_path.display()))]
    pub fn load(library_path: &Path) -> Result<Self, Vst3Error> {
        // On macOS, create and load the CFBundle BEFORE dlopen so that static
        // constructors and ObjC +load methods can find the bundle in the global
        // table. VSTGUI-based plugins cache resource paths during
        // static init — without the bundle registered, they cache nulls that later
        // cause SIGSEGV in IPlugView::attached().
        #[cfg(target_os = "macos")]
        let bundle_ref = unsafe { prepare_macos_bundle(library_path)? };
        #[cfg(not(target_os = "macos"))]
        let bundle_ref: *mut c_void = std::ptr::null_mut();

        let library = unsafe {
            // SAFETY: library_path is a valid filesystem path. libloading delegates to
            // the OS dynamic linker (dlopen), which is safe to call with any path.
            // On macOS, the dylib was already loaded by CFBundleLoadExecutable above,
            // so dlopen returns the cached handle (no re-initialization).
            libloading::Library::new(library_path)
        }
        .map_err(|e| Vst3Error::LoadError(e.to_string()))?;

        let name = library_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        // Create component and audio processor instances via the plugin factory.
        // Also calls bundleEntry to initialize plugin-global resources (GUI, etc.).
        // Native faults are process-fatal; isolate probes in a child process.
        let (component, processor, factory) =
            unsafe { call_create_instance(&library, library_path, &name, bundle_ref)? };

        // SAFETY: COM objects begin with a pointer to their vtable. The double
        // dereference extracts the vtable pointer from the COM object pointer.
        // These vtable pointers are stable for the lifetime of the library.
        let component_vtbl = unsafe { *(component as *const *const IComponentVtbl) };
        let processor_vtbl = unsafe { *(processor as *const *const IAudioProcessorVtbl) };
        let factory_vtbl = unsafe { *(factory as *const *const IPluginFactoryVtbl) };

        let mut instance = Self {
            name,
            _library: library,
            config: ProcessConfig::default(),
            state: InstanceState::Loaded,
            bus_info: BusInfo {
                num_audio_inputs: 0,
                num_audio_outputs: 2,
                num_event_inputs: 1,
                num_event_outputs: 0,
            },
            component,
            processor,
            component_vtbl,
            processor_vtbl,
            factory,
            factory_vtbl,
            controller: std::ptr::null_mut(),
            controller_is_separate: false,
            cp_comp: std::ptr::null_mut(),
            cp_ctrl: std::ptr::null_mut(),
            cp_proxy_comp: std::ptr::null_mut(),
            cp_proxy_ctrl: std::ptr::null_mut(),
            initial_params: None,
            tempo_bpm: None,
            position_samples: 0,
            tail_samples: None,
            latency_samples: 0,
            host_context: Box::new(HostContextObj::new()),
        };

        // Try IPluginFactory3::setHostContext. Steinberg SDK plugins
        // store this context internally and pass it to components/controllers.
        // Without it, createView crashes because the plugin's host context is NULL.
        unsafe {
            let mut factory3: *mut c_void = std::ptr::null_mut();
            let f3_result = ((*factory_vtbl).base.query_interface)(
                factory,
                &com::IID_IPLUGIN_FACTORY3,
                &mut factory3,
            );
            if f3_result == K_RESULT_OK && !factory3.is_null() {
                let f3_vtbl = *(factory3 as *const *const com::IPluginFactory3Vtbl);
                let result =
                    ((*f3_vtbl).set_host_context)(factory3, instance.host_context.as_ptr());
                ((*f3_vtbl).base.base.release)(factory3);
                tracing::debug!(plugin = %instance.name, result, "IPluginFactory3::setHostContext");
            }
        }

        tracing::info!(plugin = %instance.name, "loaded VST3 library and created instance");

        Ok(instance)
    }

    /// Initialize the plugin component.
    ///
    /// Calls `IComponent::initialize` with a minimal host context.
    #[tracing::instrument(name = "vst3.initialize", skip_all, fields(plugin = %self.name))]
    pub fn initialize(&mut self) -> Result<(), Vst3Error> {
        if self.state != InstanceState::Loaded {
            return Err(Vst3Error::InitFailed("invalid state".into()));
        }

        // Pass IComponentHandler as FUnknown context.
        // The context exposes IComponentHandler as its FUnknown identity,
        // so the FUnknown* passed to both component and controller IS
        // IComponentHandler. Some plugins pass this pointer internally via
        // IConnectionPoint and cast it without QI.
        let host_ctx_ptr = self.host_context.handler_ptr();
        // Native faults are process-fatal; isolate probes in a child process.
        let result = unsafe {
            call_initialize_component(
                self.component,
                self.component_vtbl,
                host_ctx_ptr,
                &self.name,
            )
        };

        if result != K_RESULT_OK {
            return Err(Vst3Error::InitFailed(format!(
                "IComponent::initialize returned {result}"
            )));
        }

        // Query bus configuration from the plugin
        self.query_bus_info();

        // Create and initialize the edit controller, then sync component state.
        // Some plugins need the controller initialized so that
        // default parameter values (amplitude envelope, etc.) are applied to the
        // component. Without this, the plugin may ignore note-off events.
        self.init_edit_controller();

        tracing::debug!(plugin = %self.name, ?self.bus_info, "initialized");
        self.state = InstanceState::Initialized;
        Ok(())
    }

    /// Configure processing parameters (sample rate, block size).
    ///
    /// Calls `IAudioProcessor::setupProcessing`.
    #[tracing::instrument(name = "vst3.setup_processing", skip_all, fields(plugin = %self.name))]
    pub fn setup_processing(&mut self, config: ProcessConfig) -> Result<(), Vst3Error> {
        if self.state != InstanceState::Initialized {
            return Err(Vst3Error::SetupFailed("must initialize first".into()));
        }

        if !config.sample_rate.is_finite()
            || config.sample_rate <= 0.0
            || config.max_block_size == 0
            || config.max_block_size > i32::MAX as u32
            || config.symbolic_sample_size != SymbolicSampleSize::Float32
        {
            return Err(Vst3Error::SetupFailed("require a positive finite sample rate, nonzero i32 block size, and Float32 buffers".into()));
        }

        let mut setup = ProcessSetup {
            process_mode: config.process_mode as i32,
            symbolic_sample_size: match config.symbolic_sample_size {
                SymbolicSampleSize::Float32 => 0,
                SymbolicSampleSize::Float64 => 1,
            },
            max_samples_per_block: config.max_block_size as i32,
            sample_rate: config.sample_rate,
        };

        let result = unsafe {
            // SAFETY: processor and processor_vtbl are valid (state is Initialized).
            // setup is a fully initialized stack-local ProcessSetup.
            ((*self.processor_vtbl).setup_processing)(self.processor, &mut setup)
        };

        if result != K_RESULT_OK {
            return Err(Vst3Error::SetupFailed(format!(
                "IAudioProcessor::setupProcessing returned {result}"
            )));
        }

        // SAFETY: processor and processor_vtbl are valid (setupProcessing succeeded).
        unsafe { self.negotiate_bus_arrangements() };

        self.config = config;
        tracing::debug!(
            plugin = %self.name,
            sample_rate = self.config.sample_rate,
            block_size = self.config.max_block_size,
            "processing setup complete"
        );
        self.state = InstanceState::SetupDone;
        Ok(())
    }

    /// Negotiate speaker arrangements with the plugin.
    ///
    /// Tries stereo for all buses first. On rejection, queries each bus via
    /// `getBusArrangement` and retries with the plugin's preferred layouts.
    /// The SDK exposes per-bus speaker arrangements for this negotiation.
    ///
    /// Supply input arrangements whenever input buses exist; some plugins
    /// reject null input arrangements.
    ///
    /// SAFETY: `processor` and `processor_vtbl` must be valid pointers.
    unsafe fn negotiate_bus_arrangements(&self) {
        unsafe {
            let num_ins = self.bus_info.num_audio_inputs as usize;
            let num_outs = self.bus_info.num_audio_outputs as usize;

            if num_outs == 0 && num_ins == 0 {
                return;
            }

            let mut in_arr: Vec<u64> = vec![K_SPEAKER_STEREO; num_ins];
            let mut out_arr: Vec<u64> = vec![K_SPEAKER_STEREO; num_outs];

            let in_ptr = if num_ins > 0 {
                in_arr.as_ptr()
            } else {
                std::ptr::null()
            };
            let out_ptr = if num_outs > 0 {
                out_arr.as_ptr()
            } else {
                std::ptr::null()
            };

            let bus_result = ((*self.processor_vtbl).set_bus_arrangements)(
                self.processor,
                in_ptr,
                num_ins as i32,
                out_ptr,
                num_outs as i32,
            );

            if bus_result == K_RESULT_OK {
                tracing::debug!(
                    plugin = %self.name,
                    ?in_arr,
                    ?out_arr,
                    "setBusArrangements(stereo)"
                );
                return;
            }

            // Fallback: query the plugin's preferred arrangement per bus and retry.
            tracing::debug!(
                plugin = %self.name,
                bus_result,
                num_ins,
                num_outs,
                "setBusArrangements(stereo) rejected — querying plugin preferences"
            );

            for (i, arrangement) in in_arr.iter_mut().enumerate() {
                let mut arr: u64 = 0;
                let r = ((*self.processor_vtbl).get_bus_arrangement)(
                    self.processor,
                    K_INPUT,
                    i as i32,
                    &mut arr,
                );
                if r == K_RESULT_OK && arr != 0 {
                    *arrangement = arr;
                }
            }

            for (i, arrangement) in out_arr.iter_mut().enumerate() {
                let mut arr: u64 = 0;
                let r = ((*self.processor_vtbl).get_bus_arrangement)(
                    self.processor,
                    K_OUTPUT,
                    i as i32,
                    &mut arr,
                );
                if r == K_RESULT_OK && arr != 0 {
                    *arrangement = arr;
                }
            }

            let in_ptr = if num_ins > 0 {
                in_arr.as_ptr()
            } else {
                std::ptr::null()
            };
            let out_ptr = if num_outs > 0 {
                out_arr.as_ptr()
            } else {
                std::ptr::null()
            };

            let retry_result = ((*self.processor_vtbl).set_bus_arrangements)(
                self.processor,
                in_ptr,
                num_ins as i32,
                out_ptr,
                num_outs as i32,
            );

            if retry_result != K_RESULT_OK {
                tracing::warn!(
                    plugin = %self.name,
                    retry_result,
                    "setBusArrangements retry with plugin preferences also failed"
                );
            } else {
                tracing::debug!(
                    plugin = %self.name,
                    ?in_arr,
                    ?out_arr,
                    "setBusArrangements succeeded with plugin-preferred layouts"
                );
            }
        }
    }

    /// Activate or deactivate all audio output and event input buses.
    /// Only manages instrument-relevant buses (audio out, event in) — audio inputs
    /// and event outputs are not activated by this instrument-oriented bus configuration.
    ///
    /// When activating (forward order: audio outputs then event inputs), fails if
    /// bus 0 of either type cannot be activated — audio output 0 is required for
    /// rendering, event input 0 is required for MIDI. Other buses are best-effort.
    /// On failure, already-activated buses are rolled back.
    ///
    /// When deactivating (reverse order: event inputs then audio outputs, highest
    /// index first), all failures are best-effort (trace-logged).
    ///
    /// SAFETY: `component` and `component_vtbl` must be valid pointers.
    unsafe fn set_buses_active(&self, active: bool) -> Result<(), Vst3Error> {
        if active {
            // SAFETY: caller guarantees component and component_vtbl are valid.
            unsafe { self.activate_buses() }
        } else {
            // SAFETY: caller guarantees component and component_vtbl are valid.
            unsafe { self.deactivate_buses() };
            Ok(())
        }
    }

    /// Activate buses in forward order: audio outputs 0..N, then event inputs 0..N.
    /// Bus 0 of each type is fatal; others are best-effort.
    ///
    /// If auxiliary buses (index > 0) fail to activate, processing continues
    /// with those buses inactive. The host still provides scratch buffers for them
    /// in process() — plugins should handle inactive buses receiving zeroed buffers.
    ///
    /// SAFETY: `component` and `component_vtbl` must be valid pointers.
    unsafe fn activate_buses(&self) -> Result<(), Vst3Error> {
        // Audio output buses — bus 0 is fatal, others best-effort.
        for i in 0..self.bus_info.num_audio_outputs {
            // SAFETY: component and component_vtbl are valid (caller guarantee).
            let result = unsafe {
                ((*self.component_vtbl).activate_bus)(
                    self.component,
                    K_AUDIO,
                    K_OUTPUT,
                    i as i32,
                    1,
                )
            };
            if result != K_RESULT_OK {
                if i == 0 {
                    return Err(Vst3Error::SetupFailed(format!(
                        "activateBus(audio output 0) failed: {result}"
                    )));
                }
                tracing::warn!(
                    plugin = %self.name,
                    bus_index = i,
                    result,
                    "activateBus(audio output) failed"
                );
            }
        }

        // Event input buses — bus 0 is fatal (instruments need MIDI), others best-effort.
        for i in 0..self.bus_info.num_event_inputs {
            // SAFETY: component and component_vtbl are valid (caller guarantee).
            let result = unsafe {
                ((*self.component_vtbl).activate_bus)(self.component, K_EVENT, K_INPUT, i as i32, 1)
            };
            if result != K_RESULT_OK {
                if i == 0 {
                    // Rollback: deactivate audio output buses that were already activated.
                    unsafe { self.deactivate_buses() };
                    return Err(Vst3Error::SetupFailed(format!(
                        "activateBus(event input 0) failed: {result}"
                    )));
                }
                tracing::warn!(
                    plugin = %self.name,
                    bus_index = i,
                    result,
                    "activateBus(event input) failed"
                );
            }
        }

        Ok(())
    }

    /// Deactivate buses in reverse order: event inputs N..0, then audio outputs N..0.
    /// All failures are best-effort (trace-logged) — we're tearing down.
    ///
    /// SAFETY: `component` and `component_vtbl` must be valid pointers.
    unsafe fn deactivate_buses(&self) {
        // SAFETY: component and component_vtbl are valid (caller guarantee).
        unsafe {
            deactivate_buses_raw(
                self.component,
                self.component_vtbl,
                &self.bus_info,
                &self.name,
            )
        }
    }

    /// Activate the plugin for processing.
    ///
    /// Activates audio/event buses, then calls `IComponent::setActive(true)`
    /// and `IAudioProcessor::setProcessing(true)`.
    pub fn activate(&mut self) -> Result<(), Vst3Error> {
        if self.state != InstanceState::SetupDone {
            return Err(Vst3Error::SetupFailed("must setup processing first".into()));
        }

        // Activate buses before setActive — VST3 spec requires this ordering.
        // SAFETY: component and component_vtbl are valid (state is SetupDone).
        unsafe { self.set_buses_active(true)? };

        // IComponent::setActive(true)
        let result = unsafe {
            // SAFETY: component_vtbl is valid (state is SetupDone).
            ((*self.component_vtbl).set_active)(self.component, 1)
        };
        if result != K_RESULT_OK {
            // Rollback: deactivate buses that were just activated.
            let _ = unsafe { self.set_buses_active(false) };
            return Err(Vst3Error::SetupFailed(format!(
                "IComponent::setActive(true) returned {result}"
            )));
        }

        // IAudioProcessor::setProcessing(true)
        let result = unsafe {
            // SAFETY: processor_vtbl is valid (state is SetupDone, setActive succeeded).
            ((*self.processor_vtbl).set_processing)(self.processor, 1)
        };
        if result != K_RESULT_OK && result != K_NOT_IMPLEMENTED && result != K_RESULT_FALSE {
            // Rollback: reverse of activate sequence (buses → setActive).
            unsafe {
                // SAFETY: setActive(true) succeeded, so setActive(false) is valid.
                ((*self.component_vtbl).set_active)(self.component, 0);
                let _ = self.set_buses_active(false);
            };
            return Err(Vst3Error::SetupFailed(format!(
                "IAudioProcessor::setProcessing(true) returned {result}"
            )));
        }
        if result == K_NOT_IMPLEMENTED {
            tracing::debug!("IAudioProcessor::setProcessing returned kNotImplemented — tolerating");
        }

        // Query plugin's declared tail length for render timeout optimization.
        // SAFETY: processor and processor_vtbl are valid (setProcessing succeeded).
        let raw_tail = unsafe { ((*self.processor_vtbl).get_tail_samples)(self.processor) };
        self.tail_samples = if raw_tail == K_INFINITE_TAIL {
            tracing::debug!(plugin = %self.name, "getTailSamples: infinite (falling back to config timeout)");
            None
        } else {
            tracing::debug!(plugin = %self.name, tail_samples = raw_tail, "getTailSamples");
            Some(raw_tail)
        };

        // Query plugin's reported latency for pre-roll compensation.
        // SAFETY: processor and processor_vtbl are valid (setProcessing succeeded).
        self.latency_samples =
            unsafe { ((*self.processor_vtbl).get_latency_samples)(self.processor) };
        if self.latency_samples > 0 {
            tracing::debug!(plugin = %self.name, latency_samples = self.latency_samples, "getLatencySamples");
        }

        tracing::debug!(
            plugin = %self.name,
            audio_outputs = self.bus_info.num_audio_outputs,
            event_inputs = self.bus_info.num_event_inputs,
            "activated"
        );
        self.state = InstanceState::Active;
        Ok(())
    }

    /// Deactivate the plugin.
    pub fn deactivate(&mut self) -> Result<(), Vst3Error> {
        if self.state != InstanceState::Active {
            return Ok(()); // already inactive
        }

        unsafe {
            // SAFETY: state is Active, so both setProcessing(false) and
            // setActive(false) are valid reverse-lifecycle calls.
            ((*self.processor_vtbl).set_processing)(self.processor, 0);
            ((*self.component_vtbl).set_active)(self.component, 0);

            // Deactivate buses after setActive(false) — reverse of activate() ordering.
            let _ = self.set_buses_active(false);
        }

        tracing::debug!(plugin = %self.name, "deactivated");
        self.state = InstanceState::SetupDone;
        Ok(())
    }

    /// Terminate and release all resources.
    pub fn terminate(&mut self) -> Result<(), Vst3Error> {
        if self.state == InstanceState::Terminated {
            return Ok(());
        }

        // Snapshot whether we need to deactivate before overwriting state.
        let was_active = self.state == InstanceState::Active;

        // Set Terminated FIRST: if any COM call below crashes or panics,
        // Drop will not re-enter terminate().
        self.state = InstanceState::Terminated;

        // VST3 requires terminate() on the main thread. Some plugins clean up ObjC/AppKit resources here — calling from a worker
        // thread dereferences uninitialized thread-local AppKit state → SIGSEGV.
        //
        // Raw pointers are not Send, but dispatch_sync_f blocks until the
        // closure completes, so the pointers are only accessed from one thread
        // at a time. We use a wrapper struct to satisfy the Send bound.
        struct TerminateCtx {
            component: *mut c_void,
            component_vtbl: *const IComponentVtbl,
            processor: *mut c_void,
            processor_vtbl: *const IAudioProcessorVtbl,
            controller: *mut c_void,
            controller_is_separate: bool,
            cp_comp: *mut c_void,
            cp_ctrl: *mut c_void,
            cp_proxy_comp: *mut c_void,
            cp_proxy_ctrl: *mut c_void,
            factory: *mut c_void,
            factory_vtbl: *const IPluginFactoryVtbl,
            library: *const libloading::Library,
            #[cfg(target_os = "linux")]
            host_context: *const HostContextObj,
            bus_info: BusInfo,
            name: String,
        }
        // SAFETY: Raw pointers are accessed only within dispatch_sync_f which
        // blocks the caller — no concurrent access is possible. The library
        // pointer remains valid because self._library outlives the synchronous
        // dispatch (it is dropped when VstInstance is dropped, after terminate
        // returns). All pointer fields are guarded by null checks before
        // dereferencing inside the closure.
        unsafe impl Send for TerminateCtx {}

        let ctx = TerminateCtx {
            component: self.component,
            component_vtbl: self.component_vtbl,
            processor: self.processor,
            processor_vtbl: self.processor_vtbl,
            controller: self.controller,
            controller_is_separate: self.controller_is_separate,
            cp_comp: self.cp_comp,
            cp_ctrl: self.cp_ctrl,
            cp_proxy_comp: self.cp_proxy_comp,
            cp_proxy_ctrl: self.cp_proxy_ctrl,
            factory: self.factory,
            factory_vtbl: self.factory_vtbl,
            library: &self._library as *const libloading::Library,
            #[cfg(target_os = "linux")]
            host_context: &*self.host_context,
            bus_info: self.bus_info.clone(),
            name: self.name.clone(),
        };

        crate::run_on_main_sync(move || {
            // Force whole-struct capture so the `unsafe impl Send for TerminateCtx`
            // applies. Rust 2021+ editions capture individual fields by default,
            // which bypasses the struct-level Send impl.
            let ctx = ctx;

            // Best-effort deactivate — VST3 lifecycle requires it before terminate,
            // but we inline the calls instead of calling self.deactivate() because
            // we've already set state to Terminated.
            if was_active {
                unsafe {
                    if !ctx.processor.is_null() {
                        ((*ctx.processor_vtbl).set_processing)(ctx.processor, 0);
                    }
                    if !ctx.component.is_null() {
                        ((*ctx.component_vtbl).set_active)(ctx.component, 0);
                        deactivate_buses_raw(
                            ctx.component,
                            ctx.component_vtbl,
                            &ctx.bus_info,
                            &ctx.name,
                        );
                    }
                }
                tracing::debug!(plugin = %ctx.name, "deactivated before terminate");
            }

            unsafe {
                // SAFETY: COM pointers are valid if non-null. After release, each
                // pointer is set to null to prevent double-free. State is already
                // Terminated so Drop cannot re-enter this method.

                // Disconnect and release the edit controller
                if !ctx.controller.is_null() {
                    let ctrl_vtbl = *(ctx.controller as *const *const IEditControllerVtblHeadless);

                    // Disconnect IConnectionPoint proxies (only for separate controllers —
                    // unified controllers were never connected)
                    if ctx.controller_is_separate {
                        let comp_cp = ctx.cp_comp;
                        let ctrl_cp = ctx.cp_ctrl;
                        if !comp_cp.is_null() && !ctrl_cp.is_null() {
                            let comp_peer = if ctx.cp_proxy_comp.is_null() {
                                ctrl_cp
                            } else {
                                ctx.cp_proxy_comp
                            };
                            let ctrl_peer = if ctx.cp_proxy_ctrl.is_null() {
                                comp_cp
                            } else {
                                ctx.cp_proxy_ctrl
                            };
                            let comp_vtbl = *(comp_cp as *const *const IConnectionPointVtbl);
                            let ctrl_vtbl = *(ctrl_cp as *const *const IConnectionPointVtbl);
                            ((*comp_vtbl).disconnect)(comp_cp, comp_peer);
                            ((*ctrl_vtbl).disconnect)(ctrl_cp, ctrl_peer);
                        }

                        // Clear proxy targets to prevent dangling notify() calls
                        // between disconnect and release.
                        if !ctx.cp_proxy_comp.is_null() {
                            com::ConnectionProxyObj::clear_target(ctx.cp_proxy_comp);
                        }
                        if !ctx.cp_proxy_ctrl.is_null() {
                            com::ConnectionProxyObj::clear_target(ctx.cp_proxy_ctrl);
                        }

                        // Release the proxies (ref-counted, will be freed).
                        if !ctx.cp_proxy_comp.is_null() {
                            let vtbl = *(ctx.cp_proxy_comp as *const *const IConnectionPointVtbl);
                            ((*vtbl).release)(ctx.cp_proxy_comp);
                        }
                        if !ctx.cp_proxy_ctrl.is_null() {
                            let vtbl = *(ctx.cp_proxy_ctrl as *const *const IConnectionPointVtbl);
                            ((*vtbl).release)(ctx.cp_proxy_ctrl);
                        }

                        // Release the plugin's connection point interfaces.
                        if !comp_cp.is_null() {
                            let cp_vtbl = *(comp_cp as *const *const IConnectionPointVtbl);
                            ((*cp_vtbl).release)(comp_cp);
                        }
                        if !ctrl_cp.is_null() {
                            let cp_vtbl = *(ctrl_cp as *const *const IConnectionPointVtbl);
                            ((*cp_vtbl).release)(ctrl_cp);
                        }
                    }

                    // Terminate and release the controller
                    // Null the handler first so the plugin can't
                    // call performEdit on a dangling pointer during terminate.
                    ((*ctrl_vtbl).set_component_handler)(ctx.controller, std::ptr::null_mut());
                    if ctx.controller_is_separate {
                        ((*ctrl_vtbl).terminate)(ctx.controller);
                        ((*ctrl_vtbl).release)(ctx.controller);
                    } else {
                        // Unified controller: release the addRef'd QueryInterface reference.
                        // Do not call terminate — the component handles its own teardown.
                        ((*ctrl_vtbl).release)(ctx.controller);
                    }
                }

                // IComponent::terminate — known crash site for buggy plugins.
                if !ctx.component.is_null() {
                    tracing::debug!(plugin = %ctx.name, "calling IComponent::terminate (known crash site for buggy plugins)");
                    ((*ctx.component_vtbl).terminate_component)(ctx.component);
                }

                #[cfg(target_os = "linux")]
                (*ctx.host_context).clear_run_loop();

                // Release IAudioProcessor (obtained via queryInterface which did addRef)
                if !ctx.processor.is_null() {
                    ((*ctx.processor_vtbl).release)(ctx.processor);
                }

                // Release IComponent
                if !ctx.component.is_null() {
                    ((*ctx.component_vtbl).release)(ctx.component);
                }

                // Release IPluginFactory
                if !ctx.factory.is_null() {
                    ((*ctx.factory_vtbl).base.release)(ctx.factory);
                }

                // Call bundleExit to clean up plugin-global resources
                call_bundle_exit(&*ctx.library);
            }

            tracing::debug!(plugin = %ctx.name, "terminated");
        });

        // Write back nulled pointers from the dispatched closure
        self.component = std::ptr::null_mut();
        self.processor = std::ptr::null_mut();
        self.controller = std::ptr::null_mut();
        self.cp_comp = std::ptr::null_mut();
        self.cp_ctrl = std::ptr::null_mut();
        self.cp_proxy_comp = std::ptr::null_mut();
        self.cp_proxy_ctrl = std::ptr::null_mut();
        self.factory = std::ptr::null_mut();

        Ok(())
    }

    /// Process a block of audio. The plugin must be in Active state.
    /// Buffers must have equal nonzero lengths no larger than the configured
    /// block size, and match `primary_output_channels()`. Only Float32 is supported.
    ///
    /// `input_events`: MIDI events for this block as (sample_offset, note, velocity, channel).
    ///     velocity > 0 produces a note-on; velocity == 0 produces a note-off.
    /// `output_channels`: mutable slices to fill with rendered audio (one per channel).
    #[tracing::instrument(name = "vst3.process", skip_all, fields(plugin = %self.name), level = "trace")]
    pub fn process(
        &mut self,
        input_events: &[(u32, u8, u8, u8)],
        output_channels: &mut [&mut [f32]],
    ) -> Result<(), Vst3Error> {
        if self.state != InstanceState::Active {
            return Err(Vst3Error::NotActive);
        }

        // Handle pending restartComponent flags from the plugin.
        let restart_flags = self.host_context.drain_restart_flags();
        if restart_flags != 0 {
            if restart_flags & com::K_PARAM_VALUES_CHANGED != 0 {
                // Re-query all parameter values from the controller and inject them.
                // This handles preset loads and internal state changes that the plugin
                // signals via restartComponent(kParamValuesChanged).
                if !self.controller.is_null() {
                    let ctrl_vtbl =
                        unsafe { *(self.controller as *const *const IEditControllerVtblHeadless) };
                    let param_count =
                        unsafe { ((*ctrl_vtbl).get_parameter_count)(self.controller) };
                    if param_count > 0 {
                        let mut params = Vec::with_capacity(param_count as usize);
                        for i in 0..param_count {
                            let mut info = com::ParameterInfo::default();
                            let r = unsafe {
                                ((*ctrl_vtbl).get_parameter_info)(
                                    self.controller,
                                    i,
                                    std::ptr::addr_of_mut!(info) as *mut std::os::raw::c_void,
                                )
                            };
                            if r == K_RESULT_OK {
                                let value = unsafe {
                                    ((*ctrl_vtbl).get_param_normalized)(self.controller, info.id)
                                };
                                params.push((info.id, value));
                            }
                        }
                        tracing::debug!(
                            count = params.len(),
                            "restartComponent(kParamValuesChanged): re-syncing parameters"
                        );
                        // Merge with any pre-existing initial_params (from a prior
                        // restartComponent that hasn't been drained yet). Dedup by
                        // param ID with last-write-wins to avoid injecting duplicate
                        // parameter changes in a single process block.
                        use std::collections::HashMap;
                        let mut param_map: HashMap<u32, f64> = match self.initial_params.take() {
                            Some(existing) => existing.into_iter().collect(),
                            None => HashMap::new(),
                        };
                        for (id, value) in params {
                            param_map.insert(id, value);
                        }
                        self.initial_params = Some(param_map.into_iter().collect());
                    }
                }
            }
            if restart_flags & com::K_LATENCY_CHANGED != 0 {
                let new_latency =
                    unsafe { ((*self.processor_vtbl).get_latency_samples)(self.processor) };
                tracing::debug!(
                    plugin = %self.name,
                    old = self.latency_samples,
                    new = new_latency,
                    "restartComponent(kLatencyChanged)"
                );
                self.latency_samples = new_latency;
            }
            if restart_flags & com::K_RELOAD_COMPONENT != 0 {
                tracing::warn!(plugin = %self.name, "restartComponent(kReloadComponent) — full reload not supported in render context");
            }
            if restart_flags & com::K_IO_CHANGED != 0 {
                tracing::warn!(plugin = %self.name, "restartComponent(kIoChanged) — I/O reconfiguration not supported in render context");
            }
            let unhandled = restart_flags
                & !(com::K_RELOAD_COMPONENT
                    | com::K_IO_CHANGED
                    | com::K_PARAM_VALUES_CHANGED
                    | com::K_LATENCY_CHANGED
                    | com::K_PARAM_TITLES_CHANGED);
            if unhandled != 0 {
                tracing::trace!(plugin = %self.name, flags = unhandled, "restartComponent: unhandled flags");
            }
        }

        let num_channels = output_channels.len();
        let num_samples = if num_channels > 0 {
            output_channels[0].len()
        } else {
            0
        };

        let output_layout = self.output_channel_counts()?;
        if output_layout.first().copied() != Some(num_channels)
            || num_samples == 0
            || num_samples > self.config.max_block_size as usize
            || output_channels.iter().any(|ch| ch.len() != num_samples)
            || input_events
                .iter()
                .any(|&(offset, note, velocity, channel)| {
                    offset as usize >= num_samples || note > 127 || velocity > 127 || channel > 15
                })
        {
            return Err(Vst3Error::RenderError("buffers must match the negotiated first output bus and block size; MIDI events must be in range".into()));
        }

        // Convert input events to VST3 Event format.
        // Use the MIDI note number as note_id so plugins that match
        // note-off to note-on by ID can pair them.
        let vst_events: Vec<com::Event> = input_events
            .iter()
            .map(|&(offset, note, vel, ch)| {
                let note_id = note as i32;
                if vel > 0 {
                    com::Event::note_on(
                        offset as i32,
                        ch as i16,
                        note as i16,
                        vel as f32 / 127.0,
                        note_id,
                    )
                } else {
                    com::Event::note_off(offset as i32, ch as i16, note as i16, 0.0, note_id)
                }
            })
            .collect();

        let mut event_list = EventListObj::with_events(vst_events);

        // Build output bus buffers — bus 0 uses the caller's buffers, auxiliary
        // buses get scratch buffers (we activate all buses but only capture bus 0).
        let num_active_outputs = output_layout.len();

        let mut channel_ptrs: Vec<*mut f32> = output_channels
            .iter_mut()
            .map(|ch| ch.as_mut_ptr())
            .collect();

        let mut output_buses: Vec<AudioBusBuffers> = Vec::with_capacity(num_active_outputs);
        output_buses.push(AudioBusBuffers {
            num_channels: num_channels as i32,
            silence_flags: 0,
            channel_buffers_32: channel_ptrs.as_mut_ptr(),
        });

        // Buses 1..N: per-channel scratch buffers for auxiliary outputs (discarded).
        // All scratch data must outlive the process() call — stored in these Vecs.
        // Inner Vecs are heap-allocated so their data pointers stay stable when the
        // outer Vec grows; pre-allocate to make this invariant explicit.
        let num_scratch_channels = output_layout.iter().skip(1).sum();
        let mut scratch_bufs: Vec<Vec<f32>> = Vec::with_capacity(num_scratch_channels);
        let mut scratch_channel_ptrs: Vec<Vec<*mut f32>> =
            Vec::with_capacity(num_active_outputs.saturating_sub(1));
        for &num_channels in output_layout.iter().skip(1) {
            let mut ptrs: Vec<*mut f32> = Vec::with_capacity(num_channels);
            for _ in 0..num_channels {
                let mut buf = vec![0.0f32; num_samples];
                ptrs.push(buf.as_mut_ptr());
                scratch_bufs.push(buf);
            }
            output_buses.push(AudioBusBuffers {
                num_channels: num_channels as i32,
                silence_flags: K_ALL_CHANNELS_SILENT,
                channel_buffers_32: ptrs.as_mut_ptr(),
            });
            scratch_channel_ptrs.push(ptrs);
        }

        // Always provide ProcessContext; plugins may require timing information.
        // Some plugins may check the kPlaying transport flag
        // before processing note events.
        let default_tempo = 120.0;
        let mut ctx = ProcessContext::new(
            self.config.sample_rate,
            self.tempo_bpm.unwrap_or(default_tempo),
        );
        if self.tempo_bpm.is_none() {
            // Clear kTempoValid when no explicit tempo was set, but keep
            // kPlaying and other flags so the plugin sees an active transport.
            ctx.state &= !com::K_TEMPO_VALID;
        }
        ctx.update_position(self.position_samples);
        let ctx_ptr = &mut ctx as *mut ProcessContext as *mut c_void;

        // Always provide non-null IParameterChanges for both input and output.
        // Standard VST3 hosts always do this; some plugins  skip event
        // processing entirely when inputParameterChanges is null.
        // On the first call, inject initial parameter values from the edit controller.
        // Combine initial params (first call only) with pending performEdit changes.
        let initial = self.initial_params.take();
        let pending = self.host_context.drain_pending_params();
        let mut input_param_changes = match (initial, pending.is_empty()) {
            (Some(params), true) => {
                tracing::debug!(count = params.len(), "injecting initial parameter changes");
                com::ParameterChangesObj::with_params(params)
            }
            (Some(params), false) => {
                // Dedup by param ID (last-write-wins): performEdit values
                // override stale initial_params for the same parameter.
                use std::collections::HashMap;
                let mut map: HashMap<u32, f64> = params.into_iter().collect();
                for (id, v) in pending {
                    map.insert(id, v);
                }
                tracing::debug!(
                    count = map.len(),
                    "injecting initial + performEdit parameter changes"
                );
                com::ParameterChangesObj::with_params(map.into_iter().collect())
            }
            (None, false) => {
                tracing::trace!(
                    count = pending.len(),
                    "injecting performEdit parameter changes"
                );
                com::ParameterChangesObj::with_params(pending)
            }
            (None, true) => com::ParameterChangesObj::empty(),
        };
        let mut output_param_changes = com::ParameterChangesObj::empty();

        let mut output_event_list = EventListObj::new();

        let mut process_data = ProcessData {
            process_mode: self.config.process_mode as i32,
            symbolic_sample_size: 0, // float32
            num_samples: num_samples as i32,
            num_inputs: 0,
            num_outputs: output_buses.len() as i32,
            inputs: std::ptr::null_mut(),
            outputs: output_buses.as_mut_ptr(),
            input_parameter_changes: input_param_changes.as_ptr(),
            output_parameter_changes: output_param_changes.as_ptr(),
            input_events: event_list.as_ptr(),
            output_events: output_event_list.as_ptr(),
            process_context: ctx_ptr,
        };

        let result = unsafe {
            // SAFETY: processor_vtbl is valid (state is Active). All referenced data
            // (output_buses, scratch_bufs, scratch_channel_ptrs, channel_ptrs,
            // event_list, process_data, ctx) are stack-allocated and outlive this call.
            ((*self.processor_vtbl).process)(self.processor, &mut process_data)
        };

        // Explicit drop after process() returns — raw pointers in output_buses
        // referenced this memory. Placing drop here makes the lifetime intent
        // clear and suppresses the unused-binding warning.
        drop(scratch_bufs);
        drop(scratch_channel_ptrs);

        // Advance transport position for the next process() call
        self.position_samples += num_samples as i64;

        if result != K_RESULT_OK {
            return Err(Vst3Error::RenderError(format!(
                "IAudioProcessor::process returned {result}"
            )));
        }

        Ok(())
    }

    /// Set the tempo (BPM) to communicate to the plugin via ProcessContext.
    /// Clamped to 20–300 BPM.
    pub fn set_tempo(&mut self, bpm: f64) {
        self.tempo_bpm = Some(bpm.clamp(20.0, 300.0));
    }

    /// Reset the transport position to zero. Call before rendering each note
    /// so tempo-synced effects start from a consistent position.
    pub fn reset_position(&mut self) {
        self.position_samples = 0;
    }

    /// Apply preset state to the plugin component.
    ///
    /// The state bytes should come from `PresetData::component_state` (parsed
    /// from a `.vstpreset` file) or from a plugin-specific preset format.
    ///
    /// The plugin must be at least Initialized (after `initialize()` returns).
    #[tracing::instrument(name = "vst3.set_state", skip_all, fields(plugin = %self.name, bytes = state_bytes.len()))]
    pub fn set_state(&mut self, state_bytes: &[u8]) -> Result<(), Vst3Error> {
        match self.state {
            InstanceState::Loaded | InstanceState::Terminated => {
                return Err(Vst3Error::StateError(
                    "plugin must be initialized before setting state".into(),
                ));
            }
            _ => {}
        }

        let mut stream = com::MemoryStream::from_bytes(state_bytes.to_vec());
        let result = unsafe {
            // SAFETY: component_vtbl is valid (state is Initialized or later).
            // stream is stack-allocated and outlives this call.
            ((*self.component_vtbl).set_state)(self.component, stream.as_ptr())
        };

        if result != K_RESULT_OK {
            return Err(Vst3Error::StateError(format!(
                "IComponent::setState returned {result}"
            )));
        }

        // Forward state to the edit controller so it can synchronize its
        // parameter representation. Required by the VST3 spec for plugins
        // with separate controllers; no-op for unified controllers.
        if self.controller_is_separate && !self.controller.is_null() {
            let mut ctrl_stream = com::MemoryStream::from_bytes(state_bytes.to_vec());
            let ctrl_vtbl =
                unsafe { *(self.controller as *const *const IEditControllerVtblHeadless) };
            // SAFETY: controller and ctrl_vtbl are valid (checked above).
            // Synchronize the controller with component state. Native faults
            // remain process-fatal, as with all plugin lifecycle calls.
            let ctrl_result = unsafe {
                call_set_component_state(
                    self.controller,
                    ctrl_vtbl,
                    ctrl_stream.as_ptr(),
                    &self.name,
                )
            };
            if ctrl_result != K_RESULT_OK {
                tracing::warn!(
                    plugin = %self.name,
                    result = ctrl_result,
                    "IEditController::setComponentState failed or crashed (non-fatal)"
                );
            }
        }

        // Drain any restartComponent flags the plugin queued during setState.
        // Plugins  fire restartComponent(kParamValuesChanged) here,
        // which would cause process() to re-read parameters from the controller.
        // But the controller may have stale/corrupt values if setComponentState
        // rejected the state. Draining the flags prevents process() from
        // overwriting the correct component state with bad controller values.
        //
        // This unconditional drain is safe for well-behaved plugins too:
        // initial_params is cleared on the next line, so even if process()
        // handled kParamValuesChanged and set initial_params, the clear would
        // discard them. The component already has the correct state from
        // IComponent::setState above.
        let restart = self.host_context.drain_restart_flags();
        if restart != 0 {
            tracing::debug!(
                plugin = %self.name,
                restart_flags = restart,
                "set_state: drained restartComponent flags 0x{restart:x} (not forwarding to process)"
            );
        }

        // Clear stale initial_params captured from factory defaults during
        // initialize(). The component now holds restored state — injecting the
        // old defaults on the first process() call would overwrite it.
        self.initial_params = None;

        tracing::debug!(plugin = %self.name, "preset state applied");
        Ok(())
    }

    /// Capture the current plugin component state as raw bytes.
    ///
    /// The bytes can be passed back to `set_state()` to restore the plugin's
    /// parameters, or forwarded to a child `vst3-renderer` process via
    /// component state followed by controller state synchronization.
    ///
    /// The plugin must be at least Initialized.
    #[tracing::instrument(name = "vst3.get_state", skip_all, fields(plugin = %self.name))]
    pub fn get_state(&self) -> Result<Vec<u8>, Vst3Error> {
        match self.state {
            InstanceState::Loaded | InstanceState::Terminated => {
                return Err(Vst3Error::StateError(
                    "plugin must be initialized before getting state".into(),
                ));
            }
            _ => {}
        }

        let mut stream = com::MemoryStream::new();
        let result = unsafe {
            // SAFETY: component_vtbl is valid (state is Initialized or later).
            // stream is stack-allocated and outlives this call.
            ((*self.component_vtbl).get_state)(self.component, stream.as_ptr())
        };

        if result != K_RESULT_OK {
            return Err(Vst3Error::StateError(format!(
                "IComponent::getState returned {result}"
            )));
        }

        let bytes = stream.into_vec();
        tracing::debug!(plugin = %self.name, bytes = bytes.len(), "plugin state captured");
        Ok(bytes)
    }

    /// Capture the edit controller's own state as raw bytes.
    ///
    /// This is SEPARATE from `get_state()` which captures the component
    /// (audio processor) state. The controller state stores UI-related
    /// data such as preset browser position, scroll state, etc.
    /// This is optional; plugins may return kNotImplemented.
    pub fn get_controller_state(&self) -> Result<Vec<u8>, Vst3Error> {
        if self.controller.is_null() {
            return Err(Vst3Error::StateError("no controller".into()));
        }
        let ctrl_vtbl = unsafe { *(self.controller as *const *const IEditControllerVtblHeadless) };
        let mut stream = com::MemoryStream::new();
        let result = unsafe { ((*ctrl_vtbl).get_state)(self.controller, stream.as_ptr()) };
        if result != K_RESULT_OK {
            return Err(Vst3Error::StateError(format!(
                "IEditController::getState returned {result}"
            )));
        }
        let bytes = stream.into_vec();
        tracing::debug!(plugin = %self.name, bytes = bytes.len(), "controller state captured");
        Ok(bytes)
    }

    /// Restore the edit controller's own state from raw bytes.
    ///
    /// This restores UI state (preset selection, browser, etc.) that was
    /// previously captured with `get_controller_state()`.
    pub fn set_controller_state(&mut self, state_bytes: &[u8]) -> Result<(), Vst3Error> {
        if self.controller.is_null() {
            return Err(Vst3Error::StateError("no controller".into()));
        }
        let ctrl_vtbl = unsafe { *(self.controller as *const *const IEditControllerVtblHeadless) };
        let mut stream = com::MemoryStream::from_bytes(state_bytes.to_vec());
        let result = unsafe { ((*ctrl_vtbl).set_state)(self.controller, stream.as_ptr()) };
        if result != K_RESULT_OK {
            return Err(Vst3Error::StateError(format!(
                "IEditController::setState returned {result}"
            )));
        }
        tracing::debug!(plugin = %self.name, bytes = state_bytes.len(), "controller state restored");
        Ok(())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn config(&self) -> &ProcessConfig {
        &self.config
    }

    /// Channel count of the negotiated first audio output bus.
    /// Query after processing setup; bus counts are available via `bus_info`.
    pub fn primary_output_channels(&self) -> Result<usize, Vst3Error> {
        self.output_channel_counts()?
            .first()
            .copied()
            .ok_or_else(|| Vst3Error::RenderError("no audio output bus".into()))
    }

    fn output_channel_counts(&self) -> Result<Vec<usize>, Vst3Error> {
        if matches!(
            self.state,
            InstanceState::Loaded | InstanceState::Terminated
        ) {
            return Err(Vst3Error::RenderError(
                "initialize before querying channel layouts".into(),
            ));
        }
        (0..self.bus_info.num_audio_outputs)
            .map(|index| {
                let mut arrangement = 0u64;
                let result = unsafe {
                    ((*self.processor_vtbl).get_bus_arrangement)(
                        self.processor,
                        K_OUTPUT,
                        index as i32,
                        &mut arrangement,
                    )
                };
                if result != K_RESULT_OK {
                    return Err(Vst3Error::RenderError(format!(
                        "getBusArrangement({index}) failed: {result}"
                    )));
                }
                Ok(arrangement.count_ones() as usize)
            })
            .collect()
    }

    /// Bus counts, which are distinct from channel counts.
    pub fn bus_info(&self) -> &BusInfo {
        &self.bus_info
    }

    pub(crate) fn ensure_editor_ready(&self) -> Result<(), Vst3Error> {
        if matches!(
            self.state,
            InstanceState::Loaded | InstanceState::Terminated
        ) || self.component.is_null()
            || self.factory.is_null()
        {
            return Err(Vst3Error::InitFailed(
                "editor requires an initialized, live instance".into(),
            ));
        }
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.state == InstanceState::Active
    }

    /// Plugin-reported tail length in samples (after note-off before output decays to silence).
    /// Returns `None` for plugins that report infinite tail (kInfiniteTail).
    /// Only valid after `activate()`.
    pub fn tail_samples(&self) -> Option<u32> {
        self.tail_samples
    }

    /// Plugin-reported internal latency in samples.
    /// Used to trim the start of rendered audio (pre-roll compensation).
    /// Only valid after `activate()`.
    pub fn latency_samples(&self) -> u32 {
        self.latency_samples
    }

    /// Get the raw IComponent COM pointer (for editor creation).
    pub(crate) fn component_ptr(&self) -> *mut c_void {
        self.component
    }

    /// Get the raw IPluginFactory COM pointer (for editor creation).
    pub(crate) fn factory_ptr(&self) -> *mut c_void {
        self.factory
    }

    /// Get the raw IEditController COM pointer (for editor creation).
    ///
    /// Returns null if the controller was never initialized (plugin doesn't support it).
    pub(crate) fn controller_ptr(&self) -> *mut c_void {
        self.controller
    }

    /// Get a raw pointer to the HostContextObj (for plug frame QI forwarding).
    ///
    /// # Safety invariant
    /// The returned pointer is only used for COM queryInterface forwarding in
    /// `unified_host_qi`, which computes sub-field addresses via `addr_of!`
    /// but never writes through the pointer. No mutation occurs.
    pub(crate) fn host_context_ptr(&self) -> *const com::HostContextObj {
        &*self.host_context as *const com::HostContextObj
    }

    /// Service the Linux IRunLoop: poll registered fds and fire due timers
    /// on the instance's host context.
    ///
    /// # Safety
    /// The registered IEventHandler/ITimerHandler pointers must still be valid.
    #[cfg(target_os = "linux")]
    pub unsafe fn service_run_loop(&self) {
        unsafe {
            self.host_context.service_run_loop();
        }
    }

    /// Create the edit controller and sync component state to it.
    ///
    /// Best-effort: if the plugin doesn't support IEditController, this is a no-op.
    fn init_edit_controller(&mut self) {
        let host_ctx_ptr = self.host_context.handler_ptr();
        unsafe {
            // Try 1: unified component — queryInterface for IEditController
            let mut controller: *mut c_void = std::ptr::null_mut();
            let result = ((*self.component_vtbl).query_interface)(
                self.component,
                &IID_IEDIT_CONTROLLER,
                &mut controller,
            );

            let is_unified = result == K_RESULT_OK && !controller.is_null();

            if !is_unified {
                // Try 2: separate controller — get CID and create via factory
                controller = std::ptr::null_mut();
                let mut controller_cid: TUID = [0u8; 16];
                let cid_result = ((*self.component_vtbl).get_controller_class_id)(
                    self.component,
                    &mut controller_cid,
                );

                if cid_result != K_RESULT_OK || controller_cid == [0u8; 16] {
                    return;
                }

                let create_result = ((*self.factory_vtbl).create_instance)(
                    self.factory,
                    &controller_cid,
                    &IID_IEDIT_CONTROLLER,
                    &mut controller,
                );

                if create_result != K_RESULT_OK || controller.is_null() {
                    return;
                }

                // Initialize the separate controller.
                // Pass IComponentHandler pointer as FUnknown context.
                // The context exposes IComponentHandler as its FUnknown identity,
                // so the FUnknown* passed to initialize IS IComponentHandler. Some
                // plugins  cast this directly without QI, so passing
                // IHostApplication here would crash.
                let ctrl_vtbl = *(controller as *const *const IEditControllerVtblHeadless);
                let init_result = ((*ctrl_vtbl).initialize)(controller, host_ctx_ptr);
                if init_result != K_RESULT_OK {
                    ((*ctrl_vtbl).release)(controller);
                    return;
                }
            }

            // Set IComponentHandler on the controller so it can route parameter changes.
            // Uses the IComponentHandler sub-interface of the unified host context,
            // which also exposes IComponentHandler2, IHostApplication, and
            // IPlugInterfaceSupport via queryInterface.
            let ctrl_vtbl = *(controller as *const *const IEditControllerVtblHeadless);
            let handler_ptr = self.host_context.handler_ptr();
            let sch_result = ((*ctrl_vtbl).set_component_handler)(controller, handler_ptr);
            tracing::debug!(plugin = %self.name, sch_result, "setComponentHandler");

            // Query IEditController2 and set knob mode to linear.
            // Notify the controller of supported host interfaces before creating its view.
            let iec2_iid: TUID = [
                0x7F, 0x4E, 0xFE, 0x59, 0xF3, 0x20, 0x49, 0x67, 0xAC, 0x27, 0xA3, 0xAE, 0xAF, 0xB6,
                0x30, 0x38,
            ];
            let mut iec2: *mut c_void = std::ptr::null_mut();
            let iec2_result = ((*ctrl_vtbl).query_interface)(controller, &iec2_iid, &mut iec2);
            if iec2_result == com::K_RESULT_OK && !iec2.is_null() {
                // IEditController2 vtable: 3 FUnknown + 3 methods.
                // setKnobMode is the first method after FUnknown (slot 3).
                #[repr(C)]
                // IEditController2: 3 FUnknown + 3 methods (setKnobMode, openHelp, openAboutBox).
                struct IEditController2Vtbl {
                    _qi: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> i32,
                    _add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
                    release: unsafe extern "C" fn(*mut c_void) -> u32,
                    set_knob_mode: unsafe extern "C" fn(*mut c_void, mode: i32) -> i32,
                    _open_help: unsafe extern "C" fn(*mut c_void, only_check: u8) -> i32,
                    _open_about_box: unsafe extern "C" fn(*mut c_void, only_check: u8) -> i32,
                }
                const _: () = assert!(
                    std::mem::size_of::<IEditController2Vtbl>() == 6 * 8,
                    "IEditController2Vtbl should be 6 pointers"
                );
                let vtbl = *(iec2 as *const *const IEditController2Vtbl);
                const K_LINEAR_MODE: i32 = 0;
                let r = ((*vtbl).set_knob_mode)(iec2, K_LINEAR_MODE);
                tracing::debug!(plugin = %self.name, result = r, "IEditController2::setKnobMode(kLinearMode)");
                ((*vtbl).release)(iec2);
            }

            // Query IMidiMapping for CC-to-parameter mapping.
            // Plugins that support MIDI CC automation expose this interface.
            let midi_mapping_iid: TUID = [
                0xDF, 0x0F, 0xF9, 0xF7, 0x49, 0xB7, 0x46, 0x69, 0xB6, 0x3A, 0xB7, 0x32, 0x7A, 0xDB,
                0xF5, 0xE5,
            ];
            let mut midi_mapping: *mut c_void = std::ptr::null_mut();
            let mm_result =
                ((*ctrl_vtbl).query_interface)(controller, &midi_mapping_iid, &mut midi_mapping);
            if mm_result == K_RESULT_OK && !midi_mapping.is_null() {
                tracing::debug!(plugin = %self.name, "IMidiMapping available — CC-to-parameter mapping supported");
                // Release — we just check for availability, don't store the interface.
                // Future: enumerate mappings via getMidiControllerAssignment.
                let mm_vtbl = *(midi_mapping as *const *const com::FUnknownVtbl);
                ((*mm_vtbl).release)(midi_mapping);
            }

            // Query INoteExpressionController for MPE/per-note expression support.
            let note_expr_iid: TUID = [
                0xB7, 0xF8, 0xF8, 0x59, 0x41, 0x23, 0x48, 0x72, 0x91, 0x16, 0x95, 0x81, 0x4F, 0x37,
                0x21, 0xA3,
            ];
            let mut note_expr: *mut c_void = std::ptr::null_mut();
            let ne_result =
                ((*ctrl_vtbl).query_interface)(controller, &note_expr_iid, &mut note_expr);
            if ne_result == K_RESULT_OK && !note_expr.is_null() {
                tracing::debug!(plugin = %self.name, "INoteExpressionController available — MPE support detected");
                // Release — we check for availability only.
                // Future: enumerate expression types and include NoteExpressionValueEvent in ProcessData.
                let ne_vtbl = *(note_expr as *const *const com::FUnknownVtbl);
                ((*ne_vtbl).release)(note_expr);
            }

            // Query standard interfaces from both component and controller; these
            // can lazily initialize
            // internal structures when first queried for certain interfaces.
            // IIDs: IEditController2, IUnitInfo, IMidiMapping, IProgramListData, IUnitData
            const GRAB_IIDS: &[(&str, TUID)] = &[
                (
                    "IEditController2",
                    [
                        0x7F, 0x4E, 0xFE, 0x59, 0xF3, 0x20, 0x49, 0x67, 0xAC, 0x27, 0xA3, 0xAE,
                        0xAF, 0xB6, 0x30, 0x38,
                    ],
                ),
                (
                    "IUnitInfo",
                    [
                        0x3D, 0x4B, 0xD6, 0xB5, 0x91, 0x3A, 0x4F, 0xD2, 0xA8, 0x86, 0xE7, 0x68,
                        0xA5, 0xEB, 0x92, 0xC1,
                    ],
                ),
                (
                    "IMidiMapping",
                    [
                        0xDF, 0x0F, 0xF9, 0xF7, 0x49, 0xB7, 0x46, 0x69, 0xB6, 0x3A, 0xB7, 0x32,
                        0x7A, 0xDB, 0xF5, 0xE5,
                    ],
                ),
                (
                    "IProgramListData",
                    [
                        0x86, 0x83, 0xB0, 0x1F, 0x7B, 0x35, 0x4F, 0x70, 0xA2, 0x65, 0x1D, 0xEC,
                        0x35, 0x3A, 0xF4, 0xFF,
                    ],
                ),
                (
                    "IUnitData",
                    [
                        0x6C, 0x38, 0x96, 0x11, 0xD3, 0x91, 0x45, 0x5D, 0xB8, 0x70, 0xB8, 0x33,
                        0x94, 0xA0, 0xEF, 0xDD,
                    ],
                ),
            ];
            for (_name, iid) in GRAB_IIDS {
                let mut obj: *mut c_void = std::ptr::null_mut();
                // Query component first
                let comp_ok =
                    ((*self.component_vtbl).query_interface)(self.component, iid, &mut obj)
                        == K_RESULT_OK
                        && !obj.is_null();
                if comp_ok {
                    let vtbl = *(obj as *const *const com::FUnknownVtbl);
                    ((*vtbl).release)(obj);
                }
                // Fallback to controller
                if !comp_ok {
                    obj = std::ptr::null_mut();
                    let ctrl_ok = ((*ctrl_vtbl).query_interface)(controller, iid, &mut obj)
                        == K_RESULT_OK
                        && !obj.is_null();
                    if ctrl_ok {
                        let vtbl = *(obj as *const *const com::FUnknownVtbl);
                        ((*vtbl).release)(obj);
                    }
                }
            }

            // Connect component ↔ controller via IConnectionPoint.
            // Only for separate controllers — unified controllers (same COM object)
            // would corrupt internal state via circular notifications.
            //
            // Connection order:
            // 1. Try direct component↔controller connection
            // 2. If direct succeeds, interpose host proxies for logging/tracing
            // 3. If proxy interposition fails, keep direct connection
            //
            // This ordering is critical: some plugins
            // reject proxy connections and may corrupt their connection state
            // if proxies are attempted first. Direct-first avoids this.
            if !is_unified {
                let mut comp_cp: *mut c_void = std::ptr::null_mut();
                let mut ctrl_cp: *mut c_void = std::ptr::null_mut();

                let comp_cp_ok = ((*self.component_vtbl).query_interface)(
                    self.component,
                    &IID_ICONNECTION_POINT,
                    &mut comp_cp,
                ) == K_RESULT_OK
                    && !comp_cp.is_null();

                let ctrl_cp_ok = ((*ctrl_vtbl).query_interface)(
                    controller,
                    &IID_ICONNECTION_POINT,
                    &mut ctrl_cp,
                ) == K_RESULT_OK
                    && !ctrl_cp.is_null();

                tracing::info!(plugin = %self.name, comp_cp_ok, ctrl_cp_ok, "IConnectionPoint query");
                if comp_cp_ok && ctrl_cp_ok {
                    let comp_cp_vtbl = *(comp_cp as *const *const IConnectionPointVtbl);
                    let ctrl_cp_vtbl = *(ctrl_cp as *const *const IConnectionPointVtbl);

                    // Step 1: Direct connection
                    let direct_ok = connect_pair(comp_cp, comp_cp_vtbl, ctrl_cp, ctrl_cp_vtbl);
                    tracing::info!(plugin = %self.name, direct_ok, "IConnectionPoint direct connection");
                    let mut connected = direct_ok;

                    // Use only the interface pointers returned by queryInterface.
                    // Product-internal offsets are not part of the public ABI.

                    if direct_ok {
                        // Step 2: Interpose proxies for logging (optional enhancement).
                        // Disconnect direct, create proxies, reconnect through proxies.
                        let proxy_comp =
                            com::ConnectionProxyObj::new_boxed("comp→ctrl", controller);
                        let proxy_ctrl =
                            com::ConnectionProxyObj::new_boxed("ctrl→comp", self.component);
                        let proxy_comp_vtbl = *(proxy_comp as *const *const IConnectionPointVtbl);
                        let proxy_ctrl_vtbl = *(proxy_ctrl as *const *const IConnectionPointVtbl);

                        // Wire proxy targets
                        ((*proxy_comp_vtbl).connect)(proxy_comp, ctrl_cp);
                        ((*proxy_ctrl_vtbl).connect)(proxy_ctrl, comp_cp);

                        // Disconnect direct, reconnect through proxies
                        ((*comp_cp_vtbl).disconnect)(comp_cp, ctrl_cp);
                        ((*ctrl_cp_vtbl).disconnect)(ctrl_cp, comp_cp);
                        let pr1 = ((*comp_cp_vtbl).connect)(comp_cp, proxy_comp);
                        let pr2 = ((*ctrl_cp_vtbl).connect)(ctrl_cp, proxy_ctrl);

                        if pr1 == K_RESULT_OK && pr2 == K_RESULT_OK {
                            tracing::info!(plugin = %self.name, "IConnectionPoint proxies interposed");
                            self.cp_proxy_comp = proxy_comp;
                            self.cp_proxy_ctrl = proxy_ctrl;
                        } else {
                            // Proxy interposition failed — restore direct connection.
                            tracing::info!(
                                plugin = %self.name, pr1, pr2,
                                "proxy interposition failed — keeping direct connection"
                            );
                            // Cleanup proxies
                            ((*proxy_comp_vtbl).disconnect)(proxy_comp, ctrl_cp);
                            ((*proxy_ctrl_vtbl).disconnect)(proxy_ctrl, comp_cp);
                            if pr1 == K_RESULT_OK {
                                ((*comp_cp_vtbl).disconnect)(comp_cp, proxy_comp);
                            }
                            if pr2 == K_RESULT_OK {
                                ((*ctrl_cp_vtbl).disconnect)(ctrl_cp, proxy_ctrl);
                            }
                            let vtbl = *(proxy_comp as *const *const com::FUnknownVtbl);
                            ((*vtbl).release)(proxy_comp);
                            let vtbl = *(proxy_ctrl as *const *const com::FUnknownVtbl);
                            ((*vtbl).release)(proxy_ctrl);

                            // Re-establish direct connection
                            connected = connect_pair(comp_cp, comp_cp_vtbl, ctrl_cp, ctrl_cp_vtbl);
                        }
                    } else {
                        // Direct connection failed — plugin may not support IConnectionPoint
                        // notify, or CPs may already be internally connected.
                        tracing::warn!(
                            plugin = %self.name,
                            "IConnectionPoint direct connection failed"
                        );
                    }
                    if connected {
                        self.cp_comp = comp_cp;
                        self.cp_ctrl = ctrl_cp;
                    }
                }

                // Keep exact queried interfaces for successful connections;
                // releasing here would lose the peers needed for disconnect.
                if comp_cp_ok && self.cp_comp.is_null() {
                    let cp_vtbl = *(comp_cp as *const *const IConnectionPointVtbl);
                    ((*cp_vtbl).release)(comp_cp);
                }
                if ctrl_cp_ok && self.cp_ctrl.is_null() {
                    let cp_vtbl = *(ctrl_cp as *const *const IConnectionPointVtbl);
                    ((*cp_vtbl).release)(ctrl_cp);
                }
            } else {
                tracing::info!(plugin = %self.name, "skipping IConnectionPoint for unified controller");
            }

            // Enumerate parameters before syncing state so host-side parameter
            // discovery is complete before applying controller state. Some plugins
            // lazily initialize internal structures during parameter enumeration
            // that setComponentState depends on.
            let pre_sync_param_count = ((*ctrl_vtbl).get_parameter_count)(controller);
            if pre_sync_param_count > 0 {
                for i in 0..pre_sync_param_count {
                    let mut info: ParameterInfo = std::mem::zeroed();
                    let _ = ((*ctrl_vtbl).get_parameter_info)(
                        controller,
                        i,
                        &mut info as *mut _ as *mut c_void,
                    );
                }
                tracing::debug!(
                    plugin = %self.name,
                    pre_sync_param_count,
                    "pre-sync parameter enumeration"
                );
            }

            // Sync component state → controller.
            let mut stream = com::MemoryStream::new();
            let get_result = ((*self.component_vtbl).get_state)(self.component, stream.as_ptr());
            let mut scs_result_val: Option<i32> = None;
            if get_result == K_RESULT_OK && !stream.is_empty() {
                tracing::debug!(
                    plugin = %self.name,
                    stream_bytes = stream.len(),
                    "component getState returned data"
                );
                stream.reset_position();
                let scs_result =
                    call_set_component_state(controller, ctrl_vtbl, stream.as_ptr(), &self.name);
                scs_result_val = Some(scs_result);
                if scs_result != K_RESULT_OK {
                    tracing::warn!(
                        plugin = %self.name,
                        result = scs_result,
                        "setComponentState failed — controller will use factory defaults"
                    );
                }
            }

            // Collect final parameter values from the controller AFTER
            // setComponentState so we capture post-sync values. These are
            // injected into the first process() call as inputParameterChanges.
            let param_count = ((*ctrl_vtbl).get_parameter_count)(controller);
            let mut initial_params = Vec::new();
            for i in 0..param_count {
                let mut info: ParameterInfo = std::mem::zeroed();
                if ((*ctrl_vtbl).get_parameter_info)(
                    controller,
                    i,
                    &mut info as *mut _ as *mut c_void,
                ) == K_RESULT_OK
                {
                    let value = ((*ctrl_vtbl).get_param_normalized)(controller, info.id);
                    initial_params.push((info.id, value));
                }
            }
            tracing::info!(
                plugin = %self.name,
                param_count,
                unified = is_unified,
                get_state = get_result,
                set_component_state = ?scs_result_val,
                "controller diagnostics"
            );
            tracing::debug!(plugin = %self.name, count = initial_params.len(), "collected initial params from controller");
            self.initial_params = Some(initial_params);

            // Store controller — must stay alive for the IConnectionPoint link
            self.controller = controller;
            self.controller_is_separate = !is_unified;
            tracing::debug!(plugin = %self.name, unified = is_unified, "edit controller initialized");
        }
    }

    /// Query bus info from the initialized component.
    fn query_bus_info(&mut self) {
        unsafe {
            // SAFETY: component_vtbl is valid. Called from initialize() where state
            // transitions to Initialized, so the component is ready for bus queries.
            // MediaTypes: kAudio = 0, kEvent = 1
            // BusDirections: kInput = 0, kOutput = 1
            let audio_inputs = ((*self.component_vtbl).get_bus_count)(self.component, 0, 0);
            let audio_outputs = ((*self.component_vtbl).get_bus_count)(self.component, 0, 1);
            let event_inputs = ((*self.component_vtbl).get_bus_count)(self.component, 1, 0);
            let event_outputs = ((*self.component_vtbl).get_bus_count)(self.component, 1, 1);

            self.bus_info = BusInfo {
                num_audio_inputs: audio_inputs.max(0) as u32,
                num_audio_outputs: audio_outputs.max(0) as u32,
                num_event_inputs: event_inputs.max(0) as u32,
                num_event_outputs: event_outputs.max(0) as u32,
            };
        }
    }
}

impl Drop for VstInstance {
    fn drop(&mut self) {
        if self.state != InstanceState::Terminated {
            let _ = self.terminate();
        }
    }
}

/// Deactivate buses in reverse order using raw pointers.
///
/// Best-effort: all failures are trace-logged, never returned as errors.
/// Used by both `VstInstance::deactivate_buses()` and the terminate closure
/// (which operates on `TerminateCtx` fields instead of `&self`).
///
/// SAFETY: `component` and `component_vtbl` must be valid, non-null pointers.
unsafe fn deactivate_buses_raw(
    component: *mut c_void,
    component_vtbl: *const IComponentVtbl,
    bus_info: &BusInfo,
    name: &str,
) {
    // Event inputs first (reverse of activation order).
    for i in (0..bus_info.num_event_inputs).rev() {
        let result =
            unsafe { ((*component_vtbl).activate_bus)(component, K_EVENT, K_INPUT, i as i32, 0) };
        if result != K_RESULT_OK {
            tracing::trace!(
                plugin = %name,
                bus_index = i,
                result,
                "deactivateBus(event input) failed (best-effort)"
            );
        }
    }
    // Audio outputs last (reverse of activation order).
    for i in (0..bus_info.num_audio_outputs).rev() {
        let result =
            unsafe { ((*component_vtbl).activate_bus)(component, K_AUDIO, K_OUTPUT, i as i32, 0) };
        if result != K_RESULT_OK {
            tracing::trace!(
                plugin = %name,
                bus_index = i,
                result,
                "deactivateBus(audio output) failed (best-effort)"
            );
        }
    }
}

/// Resolve the .vst3 bundle directory from a binary path.
///
/// Binary is at Something.vst3/Contents/MacOS/Something — returns the .vst3 dir.
#[cfg(target_os = "macos")]
fn vst3_bundle_path(library_path: &Path) -> Result<std::path::PathBuf, Vst3Error> {
    library_path
        .ancestors()
        .find(|p| {
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("vst3"))
        })
        .map(|p| p.to_path_buf())
        .ok_or_else(|| Vst3Error::LoadError("could not find .vst3 bundle directory".into()))
}

/// Create and load a CFBundle for a VST3 plugin BEFORE dlopen.
///
/// This must be called before `Library::new()` (dlopen) so that the CFBundle is
/// registered in the global bundle table when static constructors and ObjC +load
/// methods run. VSTGUI-based plugins look up their bundle during
/// static initialization to cache resource paths, font managers, and image loaders.
/// If the bundle isn't registered yet, these lookups return null and the cached nulls
/// cause SIGSEGV later in `IPlugView::attached()`.
///
/// Returns the CFBundleRef (retained, caller must not release — the plugin retains it).
#[cfg(target_os = "macos")]
unsafe fn prepare_macos_bundle(library_path: &Path) -> Result<*mut c_void, Vst3Error> {
    let bundle_path = vst3_bundle_path(library_path)?;
    let bundle_path_str = bundle_path
        .to_str()
        .ok_or_else(|| Vst3Error::LoadError("bundle path is not valid UTF-8".into()))?;

    // Load CoreFoundation dynamically
    let cf = unsafe {
        libloading::Library::new(
            "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation",
        )
    }
    .map_err(|e| Vst3Error::LoadError(format!("failed to load CoreFoundation: {e}")))?;

    type CFStringCreateWithCString =
        unsafe extern "C" fn(*mut c_void, *const u8, u32) -> *mut c_void;
    type CFURLCreateWithFileSystemPath =
        unsafe extern "C" fn(*mut c_void, *mut c_void, i64, bool) -> *mut c_void;
    type CFBundleCreate = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    // Use CFBundleLoadExecutableAndReturnError (preferred API)
    // rather than the older CFBundleLoadExecutable.
    type CFBundleLoadExecutableAndReturnError =
        unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> u8;
    type CFRelease = unsafe extern "C" fn(*mut c_void);

    let cf_string_create: CFStringCreateWithCString =
        *unsafe { cf.get::<CFStringCreateWithCString>(b"CFStringCreateWithCString") }
            .map_err(|e| Vst3Error::LoadError(format!("CFStringCreateWithCString: {e}")))?;
    let cf_url_create: CFURLCreateWithFileSystemPath =
        *unsafe { cf.get::<CFURLCreateWithFileSystemPath>(b"CFURLCreateWithFileSystemPath") }
            .map_err(|e| Vst3Error::LoadError(format!("CFURLCreateWithFileSystemPath: {e}")))?;
    let cf_bundle_create: CFBundleCreate = *unsafe { cf.get::<CFBundleCreate>(b"CFBundleCreate") }
        .map_err(|e| Vst3Error::LoadError(format!("CFBundleCreate: {e}")))?;
    let cf_bundle_load_executable: CFBundleLoadExecutableAndReturnError = *unsafe {
        cf.get::<CFBundleLoadExecutableAndReturnError>(b"CFBundleLoadExecutableAndReturnError")
    }
    .map_err(|e| Vst3Error::LoadError(format!("CFBundleLoadExecutableAndReturnError: {e}")))?;
    let cf_release: CFRelease = *unsafe { cf.get::<CFRelease>(b"CFRelease") }
        .map_err(|e| Vst3Error::LoadError(format!("CFRelease: {e}")))?;

    // Keep CoreFoundation loaded for process lifetime
    std::mem::forget(cf);

    unsafe {
        // Create CFString from path (kCFStringEncodingUTF8 = 0x08000100)
        let path_cstr = std::ffi::CString::new(bundle_path_str)
            .map_err(|_| Vst3Error::LoadError("null byte in path".into()))?;
        let cf_path = cf_string_create(std::ptr::null_mut(), path_cstr.as_ptr() as _, 0x08000100);
        if cf_path.is_null() {
            return Err(Vst3Error::LoadError(
                "CFStringCreateWithCString failed".into(),
            ));
        }

        // Create CFURL (kCFURLPOSIXPathStyle = 0)
        let cf_url = cf_url_create(std::ptr::null_mut(), cf_path, 0, true);
        cf_release(cf_path);
        if cf_url.is_null() {
            return Err(Vst3Error::LoadError(
                "CFURLCreateWithFileSystemPath failed".into(),
            ));
        }

        // Create CFBundle — this registers it in the global bundle table
        let bundle = cf_bundle_create(std::ptr::null_mut(), cf_url);
        cf_release(cf_url);
        if bundle.is_null() {
            return Err(Vst3Error::LoadError("CFBundleCreate failed".into()));
        }

        // Load the bundle's executable using the preferred API.
        // This triggers dlopen internally, causing static constructors and +load
        // methods to run WITH the CFBundle already registered in the global table.
        let mut cf_error: *mut c_void = std::ptr::null_mut();
        let loaded = cf_bundle_load_executable(bundle, &mut cf_error);
        if loaded == 0 {
            // Release error if present, then fail
            if !cf_error.is_null() {
                cf_release(cf_error);
            }
            cf_release(bundle);
            return Err(Vst3Error::LoadError(
                "CFBundleLoadExecutableAndReturnError failed".into(),
            ));
        }
        tracing::debug!(
            path = %bundle_path.display(),
            "CFBundleLoadExecutableAndReturnError succeeded"
        );

        // Don't release the bundle — the plugin retains it for resource loading.
        Ok(bundle)
    }
}

/// Call `bundleEntry` on macOS using a pre-created CFBundleRef.
///
/// The bundle_ref must have been created by `prepare_macos_bundle` before
/// `Library::new` (dlopen) so that static constructors could find the bundle.
#[cfg(target_os = "macos")]
unsafe fn call_bundle_entry(
    library: &libloading::Library,
    bundle_ref: *mut c_void,
) -> Result<bool, Vst3Error> {
    type BundleEntryProc = unsafe extern "C" fn(*mut c_void) -> bool;
    let entry: Result<libloading::Symbol<BundleEntryProc>, _> =
        unsafe { library.get(b"bundleEntry") };
    let entry = match entry {
        Ok(f) => f,
        Err(_) => return Ok(false), // not all plugins export bundleEntry
    };

    let result = unsafe { entry(bundle_ref) };
    // Don't release the bundle — the plugin retains it for resource loading.
    // It will be released when bundleExit is called.
    tracing::debug!(result, "bundleEntry");
    Ok(result)
}

#[cfg(target_os = "linux")]
unsafe fn call_bundle_entry(
    library: &libloading::Library,
    library_path: &Path,
) -> Result<bool, Vst3Error> {
    type ModuleEntryProc = unsafe extern "C" fn(*mut c_void) -> bool;
    let entry: Result<libloading::Symbol<ModuleEntryProc>, _> =
        unsafe { library.get(b"ModuleEntry") };
    match entry {
        Ok(f) => {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(library_path.as_os_str().as_bytes())
                .map_err(|_| Vst3Error::LoadError("library path contains NUL".into()))?;
            // ModuleEntry receives the dlopen handle, not the filename. Retain
            // an existing handle temporarily; the Library still owns its reference.
            let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD) };
            if handle.is_null() {
                return Err(Vst3Error::LoadError(
                    "loaded module handle unavailable".into(),
                ));
            }
            let initialized = unsafe { f(handle) };
            unsafe { libc::dlclose(handle) };
            Ok(initialized)
        }
        Err(_) => Ok(false),
    }
}

/// Call `bundleExit` / `ModuleExit` to clean up plugin-global resources.
unsafe fn call_bundle_exit(library: &libloading::Library) {
    #[cfg(target_os = "macos")]
    {
        type BundleExitProc = unsafe extern "C" fn() -> bool;
        if let Ok(exit) = unsafe { library.get::<BundleExitProc>(b"bundleExit") } {
            unsafe { exit() };
        }
    }
    #[cfg(target_os = "linux")]
    {
        type ModuleExitProc = unsafe extern "C" fn() -> bool;
        if let Ok(exit) = unsafe { library.get::<ModuleExitProc>(b"ModuleExit") } {
            unsafe { exit() };
        }
    }
}

/// Load the VST3 factory and create IComponent + IAudioProcessor instances.
///
/// Finds the first "Audio Module Class" in the factory and instantiates it.
///
/// On macOS, `bundle_ref` is the CFBundleRef created by `prepare_macos_bundle`
/// (must be non-null). On other platforms it is ignored.
unsafe fn create_instance(
    library: &libloading::Library,
    library_path: &Path,
    name: &str,
    bundle_ref: *mut c_void,
) -> Result<(*mut c_void, *mut c_void, *mut c_void), Vst3Error> {
    // Step 0: Call bundleEntry to initialize plugin-global resources (GUI, etc.)
    #[cfg(target_os = "macos")]
    unsafe {
        let _ = library_path; // only used on Linux
        call_bundle_entry(library, bundle_ref)?;
    }
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = bundle_ref; // only used on macOS
        call_bundle_entry(library, library_path)?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = (library_path, bundle_ref);

    // Step 1: Get the factory entry point
    type GetFactoryProc = unsafe extern "C" fn() -> *mut c_void;
    let get_factory: libloading::Symbol<GetFactoryProc> = unsafe {
        // SAFETY: The library is a valid VST3 plugin; GetPluginFactory is the
        // standard VST3 entry point exported by all conforming plugins.
        library
            .get(b"GetPluginFactory")
            .map_err(|_| Vst3Error::EntryPointNotFound)?
    };

    let factory = unsafe {
        // SAFETY: get_factory is a valid function pointer obtained from the library.
        get_factory()
    };
    if factory.is_null() {
        return Err(Vst3Error::FactoryFailed);
    }

    let vtbl = unsafe {
        // SAFETY: factory is non-null (checked above). COM objects begin with a
        // vtable pointer.
        *(factory as *const *const IPluginFactoryVtbl)
    };

    // Step 2: Find the first Audio Module Class
    let count = unsafe {
        // SAFETY: vtbl is valid, factory is a live COM object.
        ((*vtbl).count_classes)(factory)
    };
    let mut audio_class_cid: Option<[u8; 16]> = None;

    for i in 0..count {
        let mut info = PClassInfo::default();
        if unsafe {
            // SAFETY: vtbl and factory are valid; info is a stack-allocated output param.
            ((*vtbl).get_class_info)(factory, i, &mut info)
        } == K_RESULT_OK
        {
            let category = com::cstr_from_buf(&info.category);
            if category == "Audio Module Class" {
                audio_class_cid = Some(info.cid);
                let class_name = com::cstr_from_buf(&info.name);
                tracing::debug!(plugin = %name, class = %class_name, "found Audio Module Class");
                break;
            }
        }
    }

    let cid = match audio_class_cid {
        Some(cid) => cid,
        None => {
            unsafe {
                // SAFETY: factory is a live COM object; release decrements its ref count.
                ((*vtbl).base.release)(factory)
            };
            return Err(Vst3Error::ComponentFailed(
                "no Audio Module Class found".into(),
            ));
        }
    };

    // Step 3: Create IComponent instance
    let mut component: *mut c_void = std::ptr::null_mut();
    let result = unsafe {
        // SAFETY: factory is valid, cid is a valid class ID found above,
        // IID_ICOMPONENT is the correct interface ID. component is an output param.
        ((*vtbl).create_instance)(factory, &cid, &IID_ICOMPONENT, &mut component)
    };

    if result != K_RESULT_OK || component.is_null() {
        unsafe { ((*vtbl).base.release)(factory) };
        return Err(Vst3Error::ComponentFailed(format!(
            "createInstance returned {result} for {name}"
        )));
    }

    // Step 4: Query IAudioProcessor from IComponent
    let comp_vtbl = unsafe { *(component as *const *const IComponentVtbl) };
    let mut processor: *mut c_void = std::ptr::null_mut();
    let result = unsafe {
        // SAFETY: component is a valid IComponent (created above, result checked).
        // comp_vtbl is its vtable. processor is an output param.
        ((*comp_vtbl).query_interface)(component, &IID_IAUDIO_PROCESSOR, &mut processor)
    };

    if result != K_RESULT_OK || processor.is_null() {
        unsafe { ((*comp_vtbl).release)(component) };
        unsafe { ((*vtbl).base.release)(factory) };
        return Err(Vst3Error::ComponentFailed(format!(
            "queryInterface for IAudioProcessor failed for {name}"
        )));
    }

    Ok((component, processor, factory))
}

/// Establish both directions or roll back whichever direction succeeded.
/// All interface pointers must remain valid through rollback.
unsafe fn connect_pair(
    comp: *mut c_void,
    comp_vtbl: *const IConnectionPointVtbl,
    ctrl: *mut c_void,
    ctrl_vtbl: *const IConnectionPointVtbl,
) -> bool {
    unsafe {
        let a = ((*comp_vtbl).connect)(comp, ctrl);
        let b = ((*ctrl_vtbl).connect)(ctrl, comp);
        tracing::debug!(a, b, "connection pair results");
        if a == K_RESULT_OK && b == K_RESULT_OK {
            return true;
        }
        if a == K_RESULT_OK {
            ((*comp_vtbl).disconnect)(comp, ctrl);
        }
        if b == K_RESULT_OK {
            ((*ctrl_vtbl).disconnect)(ctrl, comp);
        }
        false
    }
}

// Foreign faults are process-fatal. Probe untrusted plugins in a child process;
// signal jumps through Rust and C++ frames cannot provide safe recovery.
unsafe fn call_set_component_state(
    controller: *mut c_void,
    ctrl_vtbl: *const IEditControllerVtblHeadless,
    stream: *mut c_void,
    _plugin_name: &str,
) -> i32 {
    unsafe { ((*ctrl_vtbl).set_component_state)(controller, stream) }
}

unsafe fn call_create_instance(
    library: &libloading::Library,
    library_path: &Path,
    name: &str,
    bundle_ref: *mut c_void,
) -> Result<(*mut c_void, *mut c_void, *mut c_void), Vst3Error> {
    unsafe { create_instance(library, library_path, name, bundle_ref) }
}

unsafe fn call_initialize_component(
    component: *mut c_void,
    component_vtbl: *const IComponentVtbl,
    host_ctx_ptr: *mut c_void,
    _plugin_name: &str,
) -> i32 {
    unsafe { ((*component_vtbl).initialize)(component, host_ctx_ptr) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: We can't test VstInstance::load without a real .so file,
    // but we can test the error path.
    #[test]
    fn connection_pair_rolls_back_partial_retaining_connections() {
        #[repr(C)]
        struct Peer {
            vtable: *const IConnectionPointVtbl,
            accepts: bool,
            refs: u32,
            connected: *mut c_void,
        }
        unsafe extern "C" fn qi(_: *mut c_void, _: *const TUID, _: *mut *mut c_void) -> i32 {
            -1
        }
        unsafe extern "C" fn retain(this: *mut c_void) -> u32 {
            unsafe {
                let p = &mut *(this as *mut Peer);
                p.refs += 1;
                p.refs
            }
        }
        unsafe extern "C" fn release(this: *mut c_void) -> u32 {
            unsafe {
                let p = &mut *(this as *mut Peer);
                p.refs -= 1;
                p.refs
            }
        }
        unsafe extern "C" fn connect(this: *mut c_void, other: *mut c_void) -> i32 {
            unsafe {
                let p = &mut *(this as *mut Peer);
                if !p.accepts {
                    return K_RESULT_FALSE;
                }
                assert!(p.connected.is_null());
                p.connected = other;
                retain(other);
                K_RESULT_OK
            }
        }
        unsafe extern "C" fn disconnect(this: *mut c_void, other: *mut c_void) -> i32 {
            unsafe {
                let p = &mut *(this as *mut Peer);
                assert_eq!(p.connected, other);
                p.connected = std::ptr::null_mut();
                release(other);
                K_RESULT_OK
            }
        }
        unsafe extern "C" fn notify(_: *mut c_void, _: *mut c_void) -> i32 {
            K_RESULT_OK
        }
        static VTABLE: IConnectionPointVtbl = IConnectionPointVtbl {
            query_interface: qi,
            add_ref: retain,
            release,
            connect,
            disconnect,
            notify,
        };
        for accepts in [(true, false), (false, true), (false, false), (true, true)] {
            let mut a = Peer {
                vtable: &VTABLE,
                accepts: accepts.0,
                refs: 1,
                connected: std::ptr::null_mut(),
            };
            let mut b = Peer {
                vtable: &VTABLE,
                accepts: accepts.1,
                refs: 1,
                connected: std::ptr::null_mut(),
            };
            let ap = std::ptr::from_mut(&mut a).cast();
            let bp = std::ptr::from_mut(&mut b).cast();
            unsafe {
                let connected = connect_pair(ap, &VTABLE, bp, &VTABLE);
                assert_eq!(connected, accepts.0 && accepts.1);
                if connected {
                    disconnect(ap, bp);
                    disconnect(bp, ap);
                }
            }
            assert_eq!((a.refs, b.refs), (1, 1));
            assert!(a.connected.is_null() && b.connected.is_null());
        }
    }

    #[test]
    fn load_nonexistent_plugin() {
        let result = VstInstance::load(Path::new("/nonexistent/plugin.so"));
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(matches!(err, Vst3Error::LoadError(_)));
    }
}
