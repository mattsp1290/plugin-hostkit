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

mod activation;
mod buses;
mod controller;
mod lifecycle;
mod processing;
mod state;

impl VstInstance {
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
}

impl Drop for VstInstance {
    fn drop(&mut self) {
        if self.state != InstanceState::Terminated {
            let _ = self.terminate();
        }
    }
}

mod native;
use native::*;

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
