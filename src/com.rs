//! VST3 COM types, vtable definitions, and host-side COM object implementations.
//!
//! These types mirror the VST3 SDK C++ structs with `#[repr(C)]` to ensure
//! binary-compatible layouts for FFI calls.

use std::collections::HashMap;
use std::os::raw::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

// ── Host callback ring buffer (freeze diagnostics) ─────────────────
//
// Bounded nonblocking ring that records host COM callbacks with a
// timestamp. Survives main-thread freezes because entries are written
// from whatever thread the callback fires on and can be read from the
// watchdog background thread.

/// Callback type tag for the ring buffer.
#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum HostCbKind {
    BeginEdit = 1,
    PerformEdit = 2,
    EndEdit = 3,
    RestartComponent = 4,
    SetDirty = 5,
    RequestOpenEditor = 6,
    StartGroupEdit = 7,
    FinishGroupEdit = 8,
    CreateContextMenu = 9,
    UnitSelection = 10,
    ProgramListChange = 11,
    HostQiUnknown = 12,
}

/// A single ring buffer entry — 24 bytes, no heap allocation.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct HostCbEntry {
    pub mono_ns: u64,
    pub kind: HostCbKind,
    pub _pad: [u8; 3],
    pub param_id: u32,
    pub extra: u64, // f64::to_bits() for PerformEdit, flags for RestartComponent, IID prefix for QI
}

const RING_LEN: usize = 128;
static RING_HEAD: AtomicUsize = AtomicUsize::new(0);

// Both readers and writers use nonblocking ownership of each slot. Busy
// entries are skipped: diagnostics must not delay callbacks or the watchdog.
struct RingSlot {
    data: Mutex<HostCbEntry>,
}

const EMPTY_ENTRY: HostCbEntry = HostCbEntry {
    mono_ns: 0,
    kind: HostCbKind::BeginEdit, // placeholder
    _pad: [0; 3],
    param_id: 0,
    extra: 0,
};
#[allow(clippy::declare_interior_mutable_const)] // Repeated initializer creates distinct atomics.
const EMPTY_SLOT: RingSlot = RingSlot {
    data: Mutex::new(EMPTY_ENTRY),
};

static RING: [RingSlot; RING_LEN] = [EMPTY_SLOT; RING_LEN];

fn mono_ns() -> u64 {
    // mach_absolute_time is async-signal-safe and allocation-free on macOS.
    // On Linux, use clock_gettime(CLOCK_MONOTONIC).
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn mach_absolute_time() -> u64;
        }
        unsafe { mach_absolute_time() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::time::Instant;
        static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let epoch = EPOCH.get_or_init(Instant::now);
        epoch.elapsed().as_nanos() as u64
    }
}

fn ring_push(kind: HostCbKind, param_id: u32, extra: u64) {
    let idx = RING_HEAD.fetch_add(1, Ordering::Relaxed) % RING_LEN;
    let slot = &RING[idx];
    let entry = HostCbEntry {
        mono_ns: mono_ns(),
        kind,
        _pad: [0; 3],
        param_id,
        extra,
    };
    if let Ok(mut data) = slot.data.try_lock() {
        *data = entry;
    }
}

/// Dump the last RING_LEN host callback entries as a human-readable string.
/// Called from the watchdog thread when a freeze is detected.
pub fn dump_callback_ring() -> String {
    let head = RING_HEAD.load(Ordering::Relaxed);
    let start = head.saturating_sub(RING_LEN);
    let mut lines = Vec::with_capacity(RING_LEN);
    let mut prev_ns: u64 = 0;
    for i in start..head {
        let slot = &RING[i % RING_LEN];
        let entry = match slot.data.try_lock() {
            Ok(data) => *data,
            Err(_) => continue,
        };
        if entry.mono_ns == 0 {
            continue;
        }
        let delta = if prev_ns > 0 {
            format!(
                "+{:.3}ms",
                (entry.mono_ns.saturating_sub(prev_ns)) as f64 / 1_000_000.0
            )
        } else {
            "       ".to_string()
        };
        prev_ns = entry.mono_ns;
        lines.push(format!(
            "  [{:>12}ns {delta}] {:?} param_id={} extra=0x{:016x}",
            entry.mono_ns, entry.kind, entry.param_id, entry.extra
        ));
    }
    if lines.is_empty() {
        "  (no host callbacks recorded)".to_string()
    } else {
        lines.join("\n")
    }
}

// ── Basic COM types ─────────────────────────────────────────────────

/// Result code from VST3 COM calls.
pub(crate) type TResult = i32;

pub(crate) const K_RESULT_OK: TResult = 0;
pub(crate) const K_RESULT_FALSE: TResult = 1;
pub(crate) const K_NOT_IMPLEMENTED: TResult = 3;
pub(crate) const K_NO_INTERFACE: TResult = -1;

/// 16-byte COM interface identifier.
#[allow(clippy::upper_case_acronyms)] // SDK ABI spelling.
pub(crate) type TUID = [u8; 16];

// ── Interface IDs ───────────────────────────────────────────────────

/// IComponent IID: {E831FF31-F2D5-4301-928E-BBEE25697802}
pub(crate) const IID_ICOMPONENT: TUID = [
    0xE8, 0x31, 0xFF, 0x31, 0xF2, 0xD5, 0x43, 0x01, 0x92, 0x8E, 0xBB, 0xEE, 0x25, 0x69, 0x78, 0x02,
];

/// IAudioProcessor IID: {42043F99-B7DA-453C-A569-E79D9AAEC33D}
pub(crate) const IID_IAUDIO_PROCESSOR: TUID = [
    0x42, 0x04, 0x3F, 0x99, 0xB7, 0xDA, 0x45, 0x3C, 0xA5, 0x69, 0xE7, 0x9D, 0x9A, 0xAE, 0xC3, 0x3D,
];

/// FUnknown IID: {00000000-0000-0000-C000-000000000046}
pub(crate) const IID_FUNKNOWN: TUID = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

/// IEditController IID: {DCD7BBE3-7742-448D-A874-AACC979C759E}
pub(crate) const IID_IEDIT_CONTROLLER: TUID = [
    0xDC, 0xD7, 0xBB, 0xE3, 0x77, 0x42, 0x44, 0x8D, 0xA8, 0x74, 0xAA, 0xCC, 0x97, 0x9C, 0x75, 0x9E,
];

// ── FUnknown vtable ─────────────────────────────────────────────────

#[repr(C)]
pub(crate) struct FUnknownVtbl {
    pub query_interface:
        unsafe extern "C" fn(this: *mut c_void, iid: *const TUID, obj: *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(this: *mut c_void) -> u32,
    pub release: unsafe extern "C" fn(this: *mut c_void) -> u32,
}

// ── IPluginFactory vtable (for instance creation) ───────────────────

/// Basic class info from IPluginFactory::getClassInfo.
#[repr(C)]
pub(crate) struct PClassInfo {
    pub cid: TUID,
    pub cardinality: i32,
    pub category: [u8; 32],
    pub name: [u8; 64],
}

impl Default for PClassInfo {
    fn default() -> Self {
        Self {
            cid: [0; 16],
            cardinality: 0,
            category: [0; 32],
            name: [0; 64],
        }
    }
}

/// IPluginFactory vtable (extends FUnknown with 4 methods).
#[repr(C)]
pub(crate) struct IPluginFactoryVtbl {
    pub base: FUnknownVtbl,
    pub get_factory_info: unsafe extern "C" fn(this: *mut c_void, info: *mut c_void) -> TResult,
    pub count_classes: unsafe extern "C" fn(this: *mut c_void) -> i32,
    pub get_class_info:
        unsafe extern "C" fn(this: *mut c_void, index: i32, info: *mut PClassInfo) -> TResult,
    pub create_instance: unsafe extern "C" fn(
        this: *mut c_void,
        cid: *const TUID,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> TResult,
}

/// IPluginFactory3 IID: {4555A2AB-C123-4E57-9B12-291036878931}
pub(crate) const IID_IPLUGIN_FACTORY3: TUID = [
    0x45, 0x55, 0xA2, 0xAB, 0xC1, 0x23, 0x4E, 0x57, 0x9B, 0x12, 0x29, 0x10, 0x36, 0x87, 0x89, 0x31,
];

/// IPluginFactory3 vtable (extends IPluginFactory via IPluginFactory2).
/// Layout: 7 IPluginFactory + 1 IPluginFactory2 + 2 IPluginFactory3 = 10 function pointers.
/// Verified against VST3_SDK/pluginterfaces/base/ipluginbase.h.
#[repr(C)]
pub(crate) struct IPluginFactory3Vtbl {
    pub base: IPluginFactoryVtbl,
    // IPluginFactory2 (1)
    pub get_class_info2:
        unsafe extern "C" fn(this: *mut c_void, index: i32, info: *mut c_void) -> TResult,
    // IPluginFactory3 (2)
    pub get_class_info_unicode:
        unsafe extern "C" fn(this: *mut c_void, index: i32, info: *mut c_void) -> TResult,
    pub set_host_context: unsafe extern "C" fn(this: *mut c_void, context: *mut c_void) -> TResult,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    std::mem::size_of::<IPluginFactory3Vtbl>() == 10 * 8,
    "IPluginFactory3Vtbl should be 10 pointers"
);

// ── IComponent vtable ───────────────────────────────────────────────

/// IComponent extends IPluginBase (which extends FUnknown).
/// Layout: 3 FUnknown + 2 IPluginBase + 9 IComponent = 14 function pointers.
#[repr(C)]
pub(crate) struct IComponentVtbl {
    // FUnknown (3)
    pub query_interface:
        unsafe extern "C" fn(this: *mut c_void, iid: *const TUID, obj: *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(this: *mut c_void) -> u32,
    pub release: unsafe extern "C" fn(this: *mut c_void) -> u32,
    // IPluginBase (2)
    pub initialize: unsafe extern "C" fn(this: *mut c_void, context: *mut c_void) -> TResult,
    pub terminate_component: unsafe extern "C" fn(this: *mut c_void) -> TResult,
    // IComponent (9)
    pub get_controller_class_id:
        unsafe extern "C" fn(this: *mut c_void, class_id: *mut TUID) -> TResult,
    pub set_io_mode: unsafe extern "C" fn(this: *mut c_void, mode: i32) -> TResult,
    pub get_bus_count: unsafe extern "C" fn(this: *mut c_void, media_type: i32, dir: i32) -> i32,
    pub get_bus_info: unsafe extern "C" fn(
        this: *mut c_void,
        media_type: i32,
        dir: i32,
        index: i32,
        bus: *mut c_void,
    ) -> TResult,
    pub get_routing_info: unsafe extern "C" fn(
        this: *mut c_void,
        in_info: *mut c_void,
        out_info: *mut c_void,
    ) -> TResult,
    pub activate_bus: unsafe extern "C" fn(
        this: *mut c_void,
        media_type: i32,
        dir: i32,
        index: i32,
        state: u8,
    ) -> TResult,
    pub set_active: unsafe extern "C" fn(this: *mut c_void, state: u8) -> TResult,
    pub set_state: unsafe extern "C" fn(this: *mut c_void, state: *mut c_void) -> TResult,
    pub get_state: unsafe extern "C" fn(this: *mut c_void, state: *mut c_void) -> TResult,
}

// ── IAudioProcessor vtable ──────────────────────────────────────────

/// IAudioProcessor extends FUnknown with 8 methods.
/// Layout: 3 FUnknown + 8 IAudioProcessor = 11 function pointers.
#[repr(C)]
pub(crate) struct IAudioProcessorVtbl {
    // FUnknown (3)
    pub query_interface:
        unsafe extern "C" fn(this: *mut c_void, iid: *const TUID, obj: *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(this: *mut c_void) -> u32,
    pub release: unsafe extern "C" fn(this: *mut c_void) -> u32,
    // IAudioProcessor (8)
    pub set_bus_arrangements: unsafe extern "C" fn(
        this: *mut c_void,
        inputs: *const u64,
        num_ins: i32,
        outputs: *const u64,
        num_outs: i32,
    ) -> TResult,
    pub get_bus_arrangement:
        unsafe extern "C" fn(this: *mut c_void, dir: i32, index: i32, arr: *mut u64) -> TResult,
    pub can_process_sample_size:
        unsafe extern "C" fn(this: *mut c_void, symbolic_sample_size: i32) -> TResult,
    pub get_latency_samples: unsafe extern "C" fn(this: *mut c_void) -> u32,
    pub setup_processing:
        unsafe extern "C" fn(this: *mut c_void, setup: *mut ProcessSetup) -> TResult,
    pub set_processing: unsafe extern "C" fn(this: *mut c_void, state: u8) -> TResult,
    pub process: unsafe extern "C" fn(this: *mut c_void, data: *mut ProcessData) -> TResult,
    pub get_tail_samples: unsafe extern "C" fn(this: *mut c_void) -> u32,
}

// ── Processing structures ───────────────────────────────────────────

/// VST3 ProcessSetup (24 bytes on 64-bit).
#[repr(C)]
pub(crate) struct ProcessSetup {
    pub process_mode: i32,
    pub symbolic_sample_size: i32,
    pub max_samples_per_block: i32,
    pub sample_rate: f64,
}

/// VST3 AudioBusBuffers (24 bytes on 64-bit).
#[repr(C)]
pub(crate) struct AudioBusBuffers {
    pub num_channels: i32,
    pub silence_flags: u64,
    pub channel_buffers_32: *mut *mut f32,
}

/// VST3 ProcessData (80 bytes on 64-bit).
#[repr(C)]
pub(crate) struct ProcessData {
    pub process_mode: i32,
    pub symbolic_sample_size: i32,
    pub num_samples: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    pub inputs: *mut AudioBusBuffers,
    pub outputs: *mut AudioBusBuffers,
    pub input_parameter_changes: *mut c_void,
    pub output_parameter_changes: *mut c_void,
    pub input_events: *mut c_void,
    pub output_events: *mut c_void,
    pub process_context: *mut c_void,
}

// ── VST3 ProcessContext ─────────────────────────────────────────────

/// Transport state flags for ProcessContext::state.
pub(crate) const K_PLAYING: u32 = 1 << 1;
pub(crate) const K_PROJECT_TIME_VALID: u32 = 1 << 9;
pub(crate) const K_TEMPO_VALID: u32 = 1 << 10;
pub(crate) const K_BAR_POSITION_VALID: u32 = 1 << 11;
pub(crate) const K_TIME_SIG_VALID: u32 = 1 << 13;
pub(crate) const K_CONT_TIME_VALID: u32 = 1 << 17;

/// VST3 Chord — embedded in ProcessContext.
#[repr(C)]
pub(crate) struct Chord {
    pub key_note: u8,
    pub root_note: u8,
    pub chord_mask: i16,
}

/// VST3 FrameRate — embedded in ProcessContext.
#[repr(C)]
pub(crate) struct FrameRate {
    pub frames_per_second: u32,
    pub flags: u32,
}

/// VST3 ProcessContext — passed via ProcessData.process_context to give
/// the plugin tempo, time-signature, and transport position information.
///
/// Layout must exactly match the C++ `Steinberg::Vst::ProcessContext` struct
/// from `pluginterfaces/vst/ivstprocesscontext.h`.
#[repr(C)]
pub(crate) struct ProcessContext {
    pub state: u32,
    // 4 bytes padding (alignment of next field is 8)
    pub sample_rate: f64,
    pub project_time_samples: i64,
    pub system_time: i64,
    pub continuous_time_samples: i64,
    pub project_time_music: f64,
    pub bar_position_music: f64,
    pub cycle_start_music: f64,
    pub cycle_end_music: f64,
    pub tempo: f64,
    pub time_sig_numerator: i32,
    pub time_sig_denominator: i32,
    pub chord: Chord,
    pub smpte_offset_subframes: i32,
    pub frame_rate: FrameRate,
    pub samples_to_next_clock: i32,
}

const _: () = assert!(std::mem::size_of::<ProcessContext>() == 112);

impl ProcessContext {
    /// Create a new ProcessContext for offline rendering at the given sample
    /// rate and tempo. Position starts at zero; call `update_position` to
    /// advance the playhead between process() calls.
    pub fn new(sample_rate: f64, tempo: f64) -> Self {
        Self {
            state: K_PLAYING
                | K_TEMPO_VALID
                | K_TIME_SIG_VALID
                | K_PROJECT_TIME_VALID
                | K_BAR_POSITION_VALID
                | K_CONT_TIME_VALID,
            sample_rate,
            project_time_samples: 0,
            system_time: 0,
            continuous_time_samples: 0,
            project_time_music: 0.0,
            bar_position_music: 0.0,
            cycle_start_music: 0.0,
            cycle_end_music: 0.0,
            tempo,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
            chord: Chord {
                key_note: 0,
                root_note: 0,
                chord_mask: 0,
            },
            smpte_offset_subframes: 0,
            frame_rate: FrameRate {
                frames_per_second: 0,
                flags: 0,
            },
            samples_to_next_clock: 0,
        }
    }

    /// Advance the playhead to the given sample position, recomputing the
    /// musical-time fields (quarter-note position and bar position).
    pub fn update_position(&mut self, sample_position: i64) {
        self.project_time_samples = sample_position;
        self.continuous_time_samples = sample_position;
        // Convert samples → quarter notes: (samples / sample_rate) * (tempo / 60)
        let time_secs = sample_position as f64 / self.sample_rate;
        self.project_time_music = time_secs * (self.tempo / 60.0);
        // Bar position: floor to nearest bar boundary (4 quarter notes in 4/4)
        let beats_per_bar = self.time_sig_numerator as f64;
        self.bar_position_music = (self.project_time_music / beats_per_bar).floor() * beats_per_bar;
    }
}

// ── VST3 Event types ────────────────────────────────────────────────

pub(crate) const K_NOTE_ON_EVENT: u16 = 0;
pub(crate) const K_NOTE_OFF_EVENT: u16 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct NoteOnEvent {
    pub channel: i16,
    pub pitch: i16,
    pub tuning: f32,
    pub velocity: f32,
    pub length: i32,
    pub note_id: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct NoteOffEvent {
    pub channel: i16,
    pub pitch: i16,
    pub velocity: f32,
    pub note_id: i32,
    pub tuning: f32,
}

/// Union of event data types. Aligned to 8 bytes to match C layout
/// (the C union contains DataEvent which has a pointer member).
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub(crate) union EventData {
    pub note_on: NoteOnEvent,
    pub note_off: NoteOffEvent,
}

/// VST3 Event (48 bytes on 64-bit).
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Event {
    pub bus_index: i32,
    pub sample_offset: i32,
    pub ppq_position: f64,
    pub flags: u16,
    pub event_type: u16,
    // 4 bytes implicit padding (EventData requires 8-byte alignment)
    pub data: EventData,
}

impl Event {
    pub fn note_on(
        sample_offset: i32,
        channel: i16,
        pitch: i16,
        velocity: f32,
        note_id: i32,
    ) -> Self {
        Self {
            bus_index: 0,
            sample_offset,
            ppq_position: 0.0,
            flags: 0,
            event_type: K_NOTE_ON_EVENT,
            data: EventData {
                note_on: NoteOnEvent {
                    channel,
                    pitch,
                    tuning: 0.0,
                    velocity,
                    length: 0,
                    note_id,
                },
            },
        }
    }

    pub fn note_off(
        sample_offset: i32,
        channel: i16,
        pitch: i16,
        velocity: f32,
        note_id: i32,
    ) -> Self {
        Self {
            bus_index: 0,
            sample_offset,
            ppq_position: 0.0,
            flags: 0,
            event_type: K_NOTE_OFF_EVENT,
            data: EventData {
                note_off: NoteOffEvent {
                    channel,
                    pitch,
                    velocity,
                    note_id,
                    tuning: 0.0,
                },
            },
        }
    }
}

// ── IEventList host implementation ──────────────────────────────────

/// IEventList vtable (extends FUnknown with 3 methods).
#[repr(C)]
struct IEventListVtbl {
    // FUnknown (3)
    query_interface:
        unsafe extern "C" fn(this: *mut c_void, iid: *const TUID, obj: *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(this: *mut c_void) -> u32,
    release: unsafe extern "C" fn(this: *mut c_void) -> u32,
    // IEventList (3)
    get_event_count: unsafe extern "C" fn(this: *mut c_void) -> i32,
    get_event: unsafe extern "C" fn(this: *mut c_void, index: i32, event: *mut Event) -> TResult,
    add_event: unsafe extern "C" fn(this: *mut c_void, event: *mut Event) -> TResult,
}

/// Host-side IEventList COM object.
///
/// Created on the stack, populated with events, and passed to
/// `IAudioProcessor::process` via `ProcessData::inputEvents`.
/// Must not be moved while a pointer to it is held by the plugin.
#[repr(C)]
pub(crate) struct EventListObj {
    vtable: *const IEventListVtbl,
    events: Vec<Event>,
}

// Static vtable — lives for the program lifetime.
static EVENT_LIST_VTBL: IEventListVtbl = IEventListVtbl {
    query_interface: event_list_query_interface,
    add_ref: event_list_add_ref,
    release: event_list_release,
    get_event_count: event_list_get_event_count,
    get_event: event_list_get_event,
    add_event: event_list_add_event,
};

unsafe extern "C" fn event_list_query_interface(
    _this: *mut c_void,
    _iid: *const TUID,
    _obj: *mut *mut c_void,
) -> TResult {
    K_NO_INTERFACE
}

unsafe extern "C" fn event_list_add_ref(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "C" fn event_list_release(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "C" fn event_list_get_event_count(this: *mut c_void) -> i32 {
    unsafe {
        // SAFETY: `this` is a valid pointer to an EventListObj, obtained from
        // EventListObj::as_ptr() on a live object that is not moved while this
        // pointer is held (documented on as_ptr).
        let obj = &*(this as *const EventListObj);
        obj.events.len() as i32
    }
}

unsafe extern "C" fn event_list_get_event(
    this: *mut c_void,
    index: i32,
    event: *mut Event,
) -> TResult {
    unsafe {
        // SAFETY: `this` is a valid EventListObj pointer (see event_list_get_event_count).
        // `event` is a caller-provided output pointer guaranteed valid by VST3 contract.
        let obj = &*(this as *const EventListObj);
        if index < 0 || (index as usize) >= obj.events.len() {
            return K_RESULT_FALSE;
        }
        std::ptr::write(event, obj.events[index as usize]);
        K_RESULT_OK
    }
}

unsafe extern "C" fn event_list_add_event(this: *mut c_void, event: *mut Event) -> TResult {
    unsafe {
        // SAFETY: `this` is a valid EventListObj pointer (see event_list_get_event_count).
        // `event` points to a valid, initialized Event (VST3 host contract).
        // Note: push() may reallocate, invalidating pointers from prior getEvent calls.
        // This is safe because the VST3 spec does not guarantee pointer stability across
        // addEvent calls, and this is only used for output_events (plugin writes, host
        // reads after process() returns).
        let obj = &mut *(this as *mut EventListObj);
        obj.events.push(std::ptr::read(event));
        K_RESULT_OK
    }
}

impl EventListObj {
    /// Creates an empty event list. Used for output_events in ProcessData.
    pub fn new() -> Self {
        Self {
            vtable: &EVENT_LIST_VTBL,
            events: Vec::new(),
        }
    }

    pub fn with_events(events: Vec<Event>) -> Self {
        Self {
            vtable: &EVENT_LIST_VTBL,
            events,
        }
    }

    /// Get the COM interface pointer. The EventListObj must not be moved
    /// while this pointer is in use.
    pub fn as_ptr(&mut self) -> *mut c_void {
        self as *mut Self as *mut c_void
    }
}

// ── IParameterChanges host implementation ────────────────────────────

/// A single parameter change point: (sample_offset, value).
struct ParamPoint {
    offset: i32,
    value: f64,
}

/// Host-side IParamValueQueue COM object.
#[repr(C)]
pub(crate) struct ParamValueQueueObj {
    vtable: *const ParamValueQueueVtbl,
    param_id: u32,
    points: Vec<ParamPoint>,
}

#[repr(C)]
struct ParamValueQueueVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    get_parameter_id: unsafe extern "C" fn(*mut c_void) -> u32,
    get_point_count: unsafe extern "C" fn(*mut c_void) -> i32,
    get_point: unsafe extern "C" fn(*mut c_void, i32, *mut i32, *mut f64) -> TResult,
    add_point: unsafe extern "C" fn(*mut c_void, i32, f64, *mut i32) -> TResult,
}

static PARAM_VALUE_QUEUE_VTBL: ParamValueQueueVtbl = ParamValueQueueVtbl {
    query_interface: pvq_query_interface,
    add_ref: pvq_add_ref,
    release: pvq_release,
    get_parameter_id: pvq_get_parameter_id,
    get_point_count: pvq_get_point_count,
    get_point: pvq_get_point,
    add_point: pvq_add_point,
};

unsafe extern "C" fn pvq_query_interface(
    _: *mut c_void,
    _: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe { *obj = std::ptr::null_mut() };
    K_NO_INTERFACE
}
unsafe extern "C" fn pvq_add_ref(_: *mut c_void) -> u32 {
    1
}
unsafe extern "C" fn pvq_release(_: *mut c_void) -> u32 {
    1
}
unsafe extern "C" fn pvq_get_parameter_id(this: *mut c_void) -> u32 {
    unsafe { (*(this as *const ParamValueQueueObj)).param_id }
}
unsafe extern "C" fn pvq_get_point_count(this: *mut c_void) -> i32 {
    unsafe { (*(this as *const ParamValueQueueObj)).points.len() as i32 }
}
unsafe extern "C" fn pvq_get_point(
    this: *mut c_void,
    index: i32,
    offset: *mut i32,
    value: *mut f64,
) -> TResult {
    unsafe {
        let obj = &*(this as *const ParamValueQueueObj);
        if index < 0 || (index as usize) >= obj.points.len() {
            return K_RESULT_FALSE;
        }
        let p = &obj.points[index as usize];
        *offset = p.offset;
        *value = p.value;
        K_RESULT_OK
    }
}
unsafe extern "C" fn pvq_add_point(_: *mut c_void, _: i32, _: f64, _: *mut i32) -> TResult {
    K_RESULT_FALSE
}

/// Host-side IParameterChanges COM object.
#[repr(C)]
pub(crate) struct ParameterChangesObj {
    vtable: *const ParameterChangesVtbl,
    queues: Vec<ParamValueQueueObj>,
}

#[repr(C)]
struct ParameterChangesVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    get_parameter_count: unsafe extern "C" fn(*mut c_void) -> i32,
    get_parameter_data: unsafe extern "C" fn(*mut c_void, i32) -> *mut c_void,
    add_parameter_data: unsafe extern "C" fn(*mut c_void, *const u32, *mut i32) -> *mut c_void,
}

static PARAMETER_CHANGES_VTBL: ParameterChangesVtbl = ParameterChangesVtbl {
    query_interface: pc_query_interface,
    add_ref: pc_add_ref,
    release: pc_release,
    get_parameter_count: pc_get_parameter_count,
    get_parameter_data: pc_get_parameter_data,
    add_parameter_data: pc_add_parameter_data,
};

unsafe extern "C" fn pc_query_interface(
    _: *mut c_void,
    _: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe { *obj = std::ptr::null_mut() };
    K_NO_INTERFACE
}
unsafe extern "C" fn pc_add_ref(_: *mut c_void) -> u32 {
    1
}
unsafe extern "C" fn pc_release(_: *mut c_void) -> u32 {
    1
}
unsafe extern "C" fn pc_get_parameter_count(this: *mut c_void) -> i32 {
    unsafe { (*(this as *const ParameterChangesObj)).queues.len() as i32 }
}
unsafe extern "C" fn pc_get_parameter_data(this: *mut c_void, index: i32) -> *mut c_void {
    unsafe {
        let obj = &mut *(this as *mut ParameterChangesObj);
        if index < 0 || (index as usize) >= obj.queues.len() {
            return std::ptr::null_mut();
        }
        &mut obj.queues[index as usize] as *mut ParamValueQueueObj as *mut c_void
    }
}
// Intentionally a no-op: if we pushed to queues here, previously returned
// pointers from getParameterData would be invalidated by Vec reallocation.
// Input parameter changes are fully constructed before passing to process().
unsafe extern "C" fn pc_add_parameter_data(
    _: *mut c_void,
    _: *const u32,
    _: *mut i32,
) -> *mut c_void {
    std::ptr::null_mut()
}

impl ParameterChangesObj {
    /// Create an empty IParameterChanges with no queues.
    /// Always pass this (rather than null) to comply with the VST3 hosting
    /// Providing non-null inputParameterChanges/outputParameterChanges.
    pub(crate) fn empty() -> Self {
        Self {
            vtable: &PARAMETER_CHANGES_VTBL,
            queues: Vec::new(),
        }
    }

    /// Create parameter changes with the given (param_id, value) pairs,
    /// all set at sample offset 0.
    pub(crate) fn with_params(params: Vec<(u32, f64)>) -> Self {
        let queues = params
            .into_iter()
            .map(|(id, val)| ParamValueQueueObj {
                vtable: &PARAM_VALUE_QUEUE_VTBL,
                param_id: id,
                points: vec![ParamPoint {
                    offset: 0,
                    value: val,
                }],
            })
            .collect();
        Self {
            vtable: &PARAMETER_CHANGES_VTBL,
            queues,
        }
    }

    /// Get the COM interface pointer. The object must not be moved
    /// while this pointer is in use (it points to `self`).
    pub(crate) fn as_ptr(&mut self) -> *mut c_void {
        self as *mut Self as *mut c_void
    }
}

// ── IPlugViewContentScaleSupport vtable ─────────────────────────────

/// IPlugViewContentScaleSupport vtable: 3 FUnknown + 1 method.
/// Used when querying IPlugView for content scale factor support.
#[repr(C)]
#[cfg(target_os = "macos")]
pub(crate) struct IPlugViewContentScaleVtbl {
    pub query_interface:
        unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    pub set_content_scale_factor: unsafe extern "C" fn(*mut c_void, f32) -> TResult,
}

// ── Unified Host Context ────────────────────────────────────────────
//
// Single COM object implementing IHostApplication, IComponentHandler,
// IComponentHandler2, IPlugInterfaceSupport, and IUnitHandler.
// One host object supplies both lifecycle contexts to
// IComponent::initialize() and IEditController::setComponentHandler().
// Plugins can discover all five interfaces via queryInterface from any
// sub-interface.
//
// Memory layout (all pointers, #[repr(C)]):
//   offset  0: IHostApplication vtable pointer  (primary / FUnknown identity)
//   offset  8: IPlugInterfaceSupport vtable pointer
//   offset 16: IComponentHandler vtable pointer
//   offset 24: IComponentHandler2 vtable pointer
//   offset 32: IUnitHandler vtable pointer

/// IHostApplication IID: {58E595CC-DB2D-4969-8B6A-AF8C36A664E5}
const IID_IHOST_APPLICATION: TUID = [
    0x58, 0xE5, 0x95, 0xCC, 0xDB, 0x2D, 0x49, 0x69, 0x8B, 0x6A, 0xAF, 0x8C, 0x36, 0xA6, 0x64, 0xE5,
];

/// IPlugInterfaceSupport IID: {4FB58B9E-9EAA-4E0F-AB36-1C1C-CCB5-6FEA}
const IID_IPLUG_INTERFACE_SUPPORT: TUID = [
    0x4F, 0xB5, 0x8B, 0x9E, 0x9E, 0xAA, 0x4E, 0x0F, 0xAB, 0x36, 0x1C, 0x1C, 0xCC, 0xB5, 0x6F, 0xEA,
];

// ── restartComponent flags (from ivsteditcontroller.h) ─────────────
/// Plugin wants full reload (deactivate → reactivate).
pub(crate) const K_RELOAD_COMPONENT: i32 = 1 << 0;
/// I/O configuration changed (bus count, channel count).
pub(crate) const K_IO_CHANGED: i32 = 1 << 1;
/// Parameter values changed — host should re-sync from controller.
pub(crate) const K_PARAM_VALUES_CHANGED: i32 = 1 << 2;
/// Latency changed — host should re-query getLatencySamples.
pub(crate) const K_LATENCY_CHANGED: i32 = 1 << 3;
/// Parameter titles/units changed.
pub(crate) const K_PARAM_TITLES_CHANGED: i32 = 1 << 4;

/// IComponentHandler IID: {93A0BEA3-0BD0-45DB-8E89-0B0CC1E46AC6}
pub(crate) const IID_ICOMPONENT_HANDLER: TUID = [
    0x93, 0xA0, 0xBE, 0xA3, 0x0B, 0xD0, 0x45, 0xDB, 0x8E, 0x89, 0x0B, 0x0C, 0xC1, 0xE4, 0x6A, 0xC6,
];

/// IComponentHandler2 IID: {F040B4B3-A360-45EC-ABCD-C045B4D5A2CC}
const IID_ICOMPONENT_HANDLER2: TUID = [
    0xF0, 0x40, 0xB4, 0xB3, 0xA3, 0x60, 0x45, 0xEC, 0xAB, 0xCD, 0xC0, 0x45, 0xB4, 0xD5, 0xA2, 0xCC,
];

/// IComponentHandler3 IID: {69F11617-D26B-400D-A4B6-B9647B6EBBAB}
const IID_ICOMPONENT_HANDLER3: TUID = [
    0x69, 0xF1, 0x16, 0x17, 0xD2, 0x6B, 0x40, 0x0D, 0xA4, 0xB6, 0xB9, 0x64, 0x7B, 0x6E, 0xBB, 0xAB,
];

/// IUnitHandler IID: {4B5147F8-4654-486B-8DAB-30BA163A3C56}
/// Source: vstsdk/pluginterfaces/vst/ivstunithandling.h
const IID_IUNIT_HANDLER: TUID = [
    0x4B, 0x51, 0x47, 0xF8, 0x46, 0x54, 0x48, 0x6B, 0x8D, 0xAB, 0x30, 0xBA, 0x16, 0x3A, 0x3C, 0x56,
];

/// IInfoListener IID: {0F194781-8D98-4ADA-BBA0-C1EFC011D8D0}
/// Source: vstsdk/pluginterfaces/vst/ivstchannelcontextinfo.h
const IID_IINFO_LISTENER: TUID = [
    0x0F, 0x19, 0x47, 0x81, 0x8D, 0x98, 0x4A, 0xDA, 0xBB, 0xA0, 0xC1, 0xEF, 0xC0, 0x11, 0xD8, 0xD0,
];

/// Linux::IRunLoop IID: {18C35366-9776-4F1A-9C5B83857A871389}
/// Source: vstsdk/pluginterfaces/gui/iplugview.h (Linux namespace)
#[cfg(target_os = "linux")]
const IID_IRUN_LOOP: TUID = [
    0x18, 0xC3, 0x53, 0x66, 0x97, 0x76, 0x4F, 0x1A, 0x9C, 0x5B, 0x83, 0x85, 0x7A, 0x87, 0x13, 0x89,
];

// ── Vtable definitions ──────────────────────────────────────────────

/// IHostApplication vtable: 3 FUnknown + 2 methods.
#[repr(C)]
struct IHostApplicationVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    get_name: unsafe extern "C" fn(*mut c_void, name: *mut u16) -> TResult,
    create_instance: unsafe extern "C" fn(
        *mut c_void,
        cid: *const TUID,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> TResult,
}

/// IPlugInterfaceSupport vtable: 3 FUnknown + 1 method.
#[repr(C)]
struct IPlugInterfaceSupportVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    is_plug_interface_supported: unsafe extern "C" fn(*mut c_void, iid: *const TUID) -> TResult,
}

/// IComponentHandler vtable: 3 FUnknown + 4 methods.
#[repr(C)]
struct IComponentHandlerVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    begin_edit: unsafe extern "C" fn(*mut c_void, id: u32) -> TResult,
    perform_edit: unsafe extern "C" fn(*mut c_void, id: u32, value: f64) -> TResult,
    end_edit: unsafe extern "C" fn(*mut c_void, id: u32) -> TResult,
    restart_component: unsafe extern "C" fn(*mut c_void, flags: i32) -> TResult,
}

/// IComponentHandler2 vtable: 3 FUnknown + 4 methods.
#[repr(C)]
struct IComponentHandler2Vtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    set_dirty: unsafe extern "C" fn(*mut c_void, state: u8) -> TResult,
    request_open_editor: unsafe extern "C" fn(*mut c_void, name: *const c_void) -> TResult,
    start_group_edit: unsafe extern "C" fn(*mut c_void) -> TResult,
    finish_group_edit: unsafe extern "C" fn(*mut c_void) -> TResult,
}

/// IComponentHandler3 vtable: 3 FUnknown + 1 method (createContextMenu).
#[repr(C)]
struct IComponentHandler3Vtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    create_context_menu: unsafe extern "C" fn(*mut c_void, *mut c_void, *const u32) -> *mut c_void,
}

/// IInfoListener vtable: 3 FUnknown + 1 method (setChannelContextInfos).
/// Source: vstsdk/pluginterfaces/vst/ivstattributes.h
#[repr(C)]
struct IInfoListenerVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    set_channel_context_infos: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
}

/// IUnitHandler vtable: 3 FUnknown + 2 methods.
/// notifyUnitSelection(UnitID) and notifyProgramListChange(ProgramListID, int32).
#[repr(C)]
struct IUnitHandlerVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    notify_unit_selection: unsafe extern "C" fn(*mut c_void, unit_id: i32) -> TResult,
    notify_program_list_change:
        unsafe extern "C" fn(*mut c_void, list_id: i32, program_index: i32) -> TResult,
}

/// Linux::IRunLoop vtable: 3 FUnknown + 4 methods.
/// registerEventHandler/unregisterEventHandler (fd watches),
/// registerTimer/unregisterTimer (periodic callbacks).
/// Source: vstsdk/pluginterfaces/gui/iplugview.h (Linux namespace)
#[cfg(target_os = "linux")]
#[repr(C)]
struct IRunLoopVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    register_event_handler:
        unsafe extern "C" fn(*mut c_void, handler: *mut c_void, fd: i32) -> TResult,
    unregister_event_handler: unsafe extern "C" fn(*mut c_void, handler: *mut c_void) -> TResult,
    register_timer:
        unsafe extern "C" fn(*mut c_void, handler: *mut c_void, milliseconds: u64) -> TResult,
    unregister_timer: unsafe extern "C" fn(*mut c_void, handler: *mut c_void) -> TResult,
}

/// IEventHandler vtable (plugin-side): 3 FUnknown + onFDIsSet.
/// Used to call back into the plugin when a watched fd is ready.
#[cfg(target_os = "linux")]
#[repr(C)]
struct IEventHandlerVtbl {
    _query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    _add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    _release: unsafe extern "C" fn(*mut c_void) -> u32,
    on_fd_is_set: unsafe extern "C" fn(*mut c_void, fd: i32),
}

/// ITimerHandler vtable (plugin-side): 3 FUnknown + onTimer.
/// Used to call back into the plugin when a timer fires.
#[cfg(target_os = "linux")]
#[repr(C)]
struct ITimerHandlerVtbl {
    _query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    _add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    _release: unsafe extern "C" fn(*mut c_void) -> u32,
    on_timer: unsafe extern "C" fn(*mut c_void),
}

// ── Unified host context object ─────────────────────────────────────

/// Owns one plugin COM reference; Arc snapshots extend registration lifetime
/// without calling foreign addRef/release under the registry mutex.
#[cfg(target_os = "linux")]
struct RetainedHandler {
    ptr: *mut c_void,
}

#[cfg(target_os = "linux")]
impl RetainedHandler {
    unsafe fn retain(ptr: *mut c_void) -> Option<std::sync::Arc<Self>> {
        if ptr.is_null() {
            return None;
        }
        let vtbl = unsafe { *(ptr as *const *const FUnknownVtbl) };
        unsafe {
            ((*vtbl).add_ref)(ptr);
        }
        Some(std::sync::Arc::new(Self { ptr }))
    }
}

#[cfg(target_os = "linux")]
impl Drop for RetainedHandler {
    fn drop(&mut self) {
        unsafe {
            let vtbl = *(self.ptr as *const *const FUnknownVtbl);
            ((*vtbl).release)(self.ptr);
        }
    }
}

// SAFETY: shared ownership only extends the COM reference lifetime. Callbacks
// remain confined to the caller's serialized UI event loop; COM refcounts must
// support registration/unregistration on the threads permitted by the SDK.
#[cfg(target_os = "linux")]
unsafe impl Send for RetainedHandler {}
#[cfg(target_os = "linux")]
unsafe impl Sync for RetainedHandler {}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct RunLoopState {
    event_handlers: Vec<(i32, std::sync::Arc<RetainedHandler>)>,
    timers: Vec<(u64, std::sync::Arc<RetainedHandler>, std::time::Instant)>,
}

/// Unified host context implementing IHostApplication, IPlugInterfaceSupport,
/// IComponentHandler, IComponentHandler2, IUnitHandler, and (on Linux)
/// IRunLoop. Box-allocated on VstInstance for pointer stability. Ref counting
/// is a no-op (lifetime tied to VstInstance).
///
/// Passed to `IComponent::initialize` (offset 0 = IHostApplication) and
/// `IEditController::setComponentHandler` (offset 16 = IComponentHandler).
/// Plugins can queryInterface from either entry point to discover all interfaces.
#[repr(C)]
pub(crate) struct HostContextObj {
    host_vtable: *const IHostApplicationVtbl,
    pis_vtable: *const IPlugInterfaceSupportVtbl,
    handler_vtable: *const IComponentHandlerVtbl,
    handler2_vtable: *const IComponentHandler2Vtbl,
    uh_vtable: *const IUnitHandlerVtbl,
    handler3_vtable: *const IComponentHandler3Vtbl,
    info_listener_vtable: *const IInfoListenerVtbl,
    #[cfg(target_os = "linux")]
    rl_vtable: *const IRunLoopVtbl,
    // Parameter changes queued by IComponentHandler::performEdit.
    // Drained by VstInstance::process() each block.
    pending_params: Mutex<Vec<(u32, f64)>>,
    // Restart flags queued by IComponentHandler::restartComponent.
    // Drained by VstInstance::process() or the editor event loop.
    pending_restart_flags: AtomicI32,
    #[cfg(target_os = "linux")]
    run_loop_state: Mutex<RunLoopState>,
}

// Verify field offsets match the assumptions in the per-sub-interface queryInterface
// functions (pis_query_interface, ch_query_interface, ch2_query_interface), which
// compute the base HostContextObj pointer by subtracting the field offset.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(std::mem::offset_of!(HostContextObj, host_vtable) == 0);
    assert!(std::mem::offset_of!(HostContextObj, pis_vtable) == 8);
    assert!(std::mem::offset_of!(HostContextObj, handler_vtable) == 16);
    assert!(std::mem::offset_of!(HostContextObj, handler2_vtable) == 24);
    assert!(std::mem::offset_of!(HostContextObj, uh_vtable) == 32);
    assert!(std::mem::offset_of!(HostContextObj, handler3_vtable) == 40);
    assert!(std::mem::offset_of!(HostContextObj, info_listener_vtable) == 48);
};
#[cfg(all(target_pointer_width = "64", target_os = "linux"))]
const _: () = {
    assert!(std::mem::offset_of!(HostContextObj, rl_vtable) == 56);
};

// ── Static vtables ──────────────────────────────────────────────────

static HOST_APP_VTBL: IHostApplicationVtbl = IHostApplicationVtbl {
    query_interface: host_query_interface,
    add_ref: host_noop_add_ref,
    release: host_noop_release,
    get_name: host_get_name,
    create_instance: host_create_instance,
};

static PIS_VTBL: IPlugInterfaceSupportVtbl = IPlugInterfaceSupportVtbl {
    query_interface: pis_query_interface,
    add_ref: host_noop_add_ref,
    release: host_noop_release,
    is_plug_interface_supported: pis_is_supported,
};

static CH_VTBL: IComponentHandlerVtbl = IComponentHandlerVtbl {
    query_interface: ch_query_interface,
    add_ref: host_noop_add_ref,
    release: host_noop_release,
    begin_edit: ch_begin_edit,
    perform_edit: ch_perform_edit,
    end_edit: ch_end_edit,
    restart_component: ch_restart,
};

static CH2_VTBL: IComponentHandler2Vtbl = IComponentHandler2Vtbl {
    query_interface: ch2_query_interface,
    add_ref: host_noop_add_ref,
    release: host_noop_release,
    set_dirty: ch2_set_dirty,
    request_open_editor: ch2_request_open_editor,
    start_group_edit: ch2_start_group_edit,
    finish_group_edit: ch2_finish_group_edit,
};

static CH3_VTBL: IComponentHandler3Vtbl = IComponentHandler3Vtbl {
    query_interface: ch3_query_interface,
    add_ref: host_noop_add_ref,
    release: host_noop_release,
    create_context_menu: ch3_create_context_menu,
};

static IL_VTBL: IInfoListenerVtbl = IInfoListenerVtbl {
    query_interface: il_query_interface,
    add_ref: host_noop_add_ref,
    release: host_noop_release,
    set_channel_context_infos: il_set_channel_context_infos,
};

static UH_VTBL: IUnitHandlerVtbl = IUnitHandlerVtbl {
    query_interface: uh_query_interface,
    add_ref: host_noop_add_ref,
    release: host_noop_release,
    notify_unit_selection: uh_notify_unit_selection,
    notify_program_list_change: uh_notify_program_list_change,
};

#[cfg(target_os = "linux")]
static RL_VTBL: IRunLoopVtbl = IRunLoopVtbl {
    query_interface: rl_query_interface,
    add_ref: host_noop_add_ref,
    release: host_noop_release,
    register_event_handler: rl_register_event_handler,
    unregister_event_handler: rl_unregister_event_handler,
    register_timer: rl_register_timer,
    unregister_timer: rl_unregister_timer,
};

// ── Unified queryInterface ──────────────────────────────────────────
//
// All five sub-interface QI functions delegate here. Returns the
// correct sub-interface pointer for any supported IID.

// SAFETY: Returned *mut c_void pointers are derived from *const but are sound because
// all COM callbacks on these interfaces are read-only (no mutation through `this`).
pub(crate) unsafe fn unified_host_qi(
    base: *const HostContextObj,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        if *iid == IID_IHOST_APPLICATION || *iid == IID_FUNKNOWN {
            *obj = base as *mut c_void;
            return K_RESULT_OK;
        }
        if *iid == IID_IPLUG_INTERFACE_SUPPORT {
            *obj = std::ptr::addr_of!((*base).pis_vtable) as *mut c_void;
            return K_RESULT_OK;
        }
        if *iid == IID_ICOMPONENT_HANDLER {
            *obj = std::ptr::addr_of!((*base).handler_vtable) as *mut c_void;
            return K_RESULT_OK;
        }
        if *iid == IID_ICOMPONENT_HANDLER2 {
            *obj = std::ptr::addr_of!((*base).handler2_vtable) as *mut c_void;
            return K_RESULT_OK;
        }
        if *iid == IID_ICOMPONENT_HANDLER3 {
            *obj = std::ptr::addr_of!((*base).handler3_vtable) as *mut c_void;
            return K_RESULT_OK;
        }
        if *iid == IID_IINFO_LISTENER {
            *obj = std::ptr::addr_of!((*base).info_listener_vtable) as *mut c_void;
            return K_RESULT_OK;
        }
        if *iid == IID_IUNIT_HANDLER {
            *obj = std::ptr::addr_of!((*base).uh_vtable) as *mut c_void;
            return K_RESULT_OK;
        }
        #[cfg(target_os = "linux")]
        if *iid == IID_IRUN_LOOP {
            *obj = std::ptr::addr_of!((*base).rl_vtable) as *mut c_void;
            return K_RESULT_OK;
        }
        *obj = std::ptr::null_mut();
        let id = &*iid;
        tracing::debug!(
            iid = format_args!(
                "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                id[0],
                id[1],
                id[2],
                id[3],
                id[4],
                id[5],
                id[6],
                id[7],
                id[8],
                id[9],
                id[10],
                id[11],
                id[12],
                id[13],
                id[14],
                id[15]
            ),
            "host queryInterface: unsupported IID"
        );
        ring_push(
            HostCbKind::HostQiUnknown,
            u32::from_le_bytes([id[0], id[1], id[2], id[3]]),
            0,
        );
        K_NO_INTERFACE
    }
}

// IHostApplication QI — this IS the base (offset 0)
unsafe extern "C" fn host_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe { unified_host_qi(this as *const HostContextObj, iid, obj) }
}

// IPlugInterfaceSupport QI — this is at offset of pis_vtable
unsafe extern "C" fn pis_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, pis_vtable))
            as *const HostContextObj;
        unified_host_qi(base, iid, obj)
    }
}

// IComponentHandler QI — this is at offset of handler_vtable
unsafe extern "C" fn ch_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, handler_vtable))
            as *const HostContextObj;
        unified_host_qi(base, iid, obj)
    }
}

// IComponentHandler2 QI — this is at offset of handler2_vtable
unsafe extern "C" fn ch2_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, handler2_vtable))
            as *const HostContextObj;
        unified_host_qi(base, iid, obj)
    }
}

// IComponentHandler3 QI — this is at offset of handler3_vtable
unsafe extern "C" fn ch3_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, handler3_vtable))
            as *const HostContextObj;
        unified_host_qi(base, iid, obj)
    }
}

unsafe extern "C" fn ch3_create_context_menu(
    _this: *mut c_void,
    plug_view: *mut c_void,
    param_id: *const u32,
) -> *mut c_void {
    let id_prefix = if param_id.is_null() {
        0
    } else {
        unsafe { *param_id }
    };
    tracing::info!(
        plug_view = format_args!("0x{:x}", plug_view as usize),
        param_id_prefix = format_args!("{:08x}", id_prefix),
        "IComponentHandler3::createContextMenu — returning null (not implemented)"
    );
    ring_push(HostCbKind::CreateContextMenu, id_prefix, plug_view as u64);
    std::ptr::null_mut()
}

// IInfoListener QI — this is at offset of info_listener_vtable
unsafe extern "C" fn il_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        let base = (this as *const u8)
            .sub(std::mem::offset_of!(HostContextObj, info_listener_vtable))
            as *const HostContextObj;
        unified_host_qi(base, iid, obj)
    }
}

unsafe extern "C" fn il_set_channel_context_infos(
    _this: *mut c_void,
    _list: *mut c_void,
) -> TResult {
    tracing::trace!("IInfoListener::setChannelContextInfos (stub)");
    K_RESULT_OK
}

// IUnitHandler QI — this is at offset of uh_vtable
unsafe extern "C" fn uh_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, uh_vtable))
            as *const HostContextObj;
        unified_host_qi(base, iid, obj)
    }
}

// IRunLoop QI — this is at offset of rl_vtable (Linux only)
#[cfg(target_os = "linux")]
unsafe extern "C" fn rl_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, rl_vtable))
            as *const HostContextObj;
        unified_host_qi(base, iid, obj)
    }
}

// ── Shared no-op ref counting ───────────────────────────────────────

// No-op ref counting — HostContextObj lifetime is tied to VstInstance.
// The host owns this object; plugins may call addRef/release but it has
// no effect. This is safe because the object outlives all plugin references.
unsafe extern "C" fn host_noop_add_ref(_: *mut c_void) -> u32 {
    1
}
unsafe extern "C" fn host_noop_release(_: *mut c_void) -> u32 {
    1
}

// ── IHostApplication methods ────────────────────────────────────────

unsafe extern "C" fn host_get_name(_this: *mut c_void, name: *mut u16) -> TResult {
    // VST3 SDK: getName(String128 name) — buffer is always 128 TChar (256 bytes).
    tracing::debug!("plugin queried host name");
    let app_name: Vec<u16> = "plugin-hostkit\0".encode_utf16().collect();
    unsafe {
        std::ptr::copy_nonoverlapping(app_name.as_ptr(), name, app_name.len());
    }
    K_RESULT_OK
}

/// IMessage IID: {936F033B-C6C0-47DB-BB08-82F813C1E613}
const IID_IMESSAGE: TUID = [
    0x93, 0x6F, 0x03, 0x3B, 0xC6, 0xC0, 0x47, 0xDB, 0xBB, 0x08, 0x82, 0xF8, 0x13, 0xC1, 0xE6, 0x13,
];

unsafe extern "C" fn host_create_instance(
    _this: *mut c_void,
    cid: *const TUID,
    _iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        // VST3 SDK convention: interface IIDs are used as CIDs in createInstance.
        // Plugins request IMessage for component-controller communication,
        // and IAttributeList for standalone attribute storage.
        if *cid == IID_IMESSAGE {
            tracing::debug!("host createInstance: IMessage");
            *obj = MessageObj::new_boxed();
            return K_RESULT_OK;
        }
        if *cid == IID_IATTRIBUTE_LIST {
            tracing::debug!("host createInstance: IAttributeList");
            *obj = AttributeListObj::new_boxed();
            return K_RESULT_OK;
        }
        let cid = &*cid;
        tracing::debug!(
            cid = format_args!(
                "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                cid[0],
                cid[1],
                cid[2],
                cid[3],
                cid[4],
                cid[5],
                cid[6],
                cid[7],
                cid[8],
                cid[9],
                cid[10],
                cid[11],
                cid[12],
                cid[13],
                cid[14],
                cid[15]
            ),
            "host createInstance: unsupported CID"
        );
        *obj = std::ptr::null_mut();
        K_NO_INTERFACE
    }
}

// ── IPlugInterfaceSupport methods ───────────────────────────────────

/// Interfaces the host advertises as supported via IPlugInterfaceSupport.
/// Matches the VST3 SDK reference implementation (PlugInterfaceSupport).
/// This tells plugins which plugin-side interfaces the host can consume.
const SUPPORTED_PLUG_INTERFACES: &[TUID] = &[
    // VST 3.0.0
    IID_ICOMPONENT,
    IID_IAUDIO_PROCESSOR,
    // IEditController {DCD7BBE3-7742-448D-A874-AACC979C759E}
    [
        0xDC, 0xD7, 0xBB, 0xE3, 0x77, 0x42, 0x44, 0x8D, 0xA8, 0x74, 0xAA, 0xCC, 0x97, 0x9C, 0x75,
        0x9E,
    ],
    // IConnectionPoint {70A4156F-6E6E-4026-9891-48BFAA60D8D1}
    [
        0x70, 0xA4, 0x15, 0x6F, 0x6E, 0x6E, 0x40, 0x26, 0x98, 0x91, 0x48, 0xBF, 0xAA, 0x60, 0xD8,
        0xD1,
    ],
    // IUnitInfo {3D4BD6B5-913A-4FD2-A886-E768A5EB92C1}
    [
        0x3D, 0x4B, 0xD6, 0xB5, 0x91, 0x3A, 0x4F, 0xD2, 0xA8, 0x86, 0xE7, 0x68, 0xA5, 0xEB, 0x92,
        0xC1,
    ],
    // IUnitData {6C389611-D391-455D-B870-B83394A0EFDD}
    [
        0x6C, 0x38, 0x96, 0x11, 0xD3, 0x91, 0x45, 0x5D, 0xB8, 0x70, 0xB8, 0x33, 0x94, 0xA0, 0xEF,
        0xDD,
    ],
    // IProgramListData {8683B01F-7B35-4F70-A265-1DEC353AF4FF}
    [
        0x86, 0x83, 0xB0, 0x1F, 0x7B, 0x35, 0x4F, 0x70, 0xA2, 0x65, 0x1D, 0xEC, 0x35, 0x3A, 0xF4,
        0xFF,
    ],
    // VST 3.0.1: IMidiMapping {DF0FF9F7-49B7-4669-B63A-B7327ADBF5E5}
    [
        0xDF, 0x0F, 0xF9, 0xF7, 0x49, 0xB7, 0x46, 0x69, 0xB6, 0x3A, 0xB7, 0x32, 0x7A, 0xDB, 0xF5,
        0xE5,
    ],
    // VST 3.1: IEditController2 {7F4EFE59-F320-4967-AC27-A3AEAFB63038}
    [
        0x7F, 0x4E, 0xFE, 0x59, 0xF3, 0x20, 0x49, 0x67, 0xAC, 0x27, 0xA3, 0xAE, 0xAF, 0xB6, 0x30,
        0x38,
    ],
    // IUnitHandler {4B5147F8-4654-486B-8DAB-30BA163A3C56}
    IID_IUNIT_HANDLER,
];

unsafe extern "C" fn pis_is_supported(_this: *mut c_void, iid: *const TUID) -> TResult {
    let iid = unsafe { &*iid };
    for supported in SUPPORTED_PLUG_INTERFACES {
        if *iid == *supported {
            tracing::debug!(
                iid = format_args!(
                    "{:02x}{:02x}{:02x}{:02x}...",
                    iid[0], iid[1], iid[2], iid[3]
                ),
                "isPlugInterfaceSupported: yes"
            );
            return K_RESULT_OK; // kResultTrue
        }
    }
    tracing::debug!(
        iid = format_args!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            iid[0],
            iid[1],
            iid[2],
            iid[3],
            iid[4],
            iid[5],
            iid[6],
            iid[7],
            iid[8],
            iid[9],
            iid[10],
            iid[11],
            iid[12],
            iid[13],
            iid[14],
            iid[15]
        ),
        "isPlugInterfaceSupported: NO"
    );
    K_RESULT_FALSE
}

// ── IComponentHandler methods ───────────────────────────────────────

unsafe extern "C" fn ch_begin_edit(_: *mut c_void, id: u32) -> TResult {
    tracing::debug!(param_id = id, "IComponentHandler::beginEdit");
    ring_push(HostCbKind::BeginEdit, id, 0);
    K_RESULT_OK
}

unsafe extern "C" fn ch_end_edit(_: *mut c_void, id: u32) -> TResult {
    tracing::debug!(param_id = id, "IComponentHandler::endEdit");
    ring_push(HostCbKind::EndEdit, id, 0);
    K_RESULT_OK
}

/// Queue the parameter change for injection into the next process() call.
/// The plugin's editor UI calls this when the user twists a knob.
unsafe extern "C" fn ch_perform_edit(this: *mut c_void, id: u32, value: f64) -> TResult {
    unsafe {
        tracing::trace!(param_id = id, value, "IComponentHandler::performEdit");
        ring_push(HostCbKind::PerformEdit, id, value.to_bits());
        // 'this' points to the handler_vtable field inside HostContextObj.
        let base = (this as *mut u8).sub(std::mem::offset_of!(HostContextObj, handler_vtable))
            as *mut HostContextObj;
        match (*base).pending_params.lock() {
            Ok(mut queue) => queue.push((id, value)),
            Err(poisoned) => {
                tracing::warn!("pending_params mutex poisoned in performEdit, recovering");
                poisoned.into_inner().push((id, value));
            }
        }
        K_RESULT_OK
    }
}
/// Accumulate restart flags for processing by the event loop or process().
/// The plugin's editor or processor calls this when internal state changes
/// that the host needs to react to (e.g., parameter values changed after
/// preset load, latency changed, I/O reconfiguration needed).
unsafe extern "C" fn ch_restart(this: *mut c_void, flags: i32) -> TResult {
    unsafe {
        tracing::debug!(flags, "IComponentHandler::restartComponent");
        ring_push(HostCbKind::RestartComponent, 0, flags as u64);
        let base = (this as *mut u8).sub(std::mem::offset_of!(HostContextObj, handler_vtable))
            as *mut HostContextObj;
        // Accumulate flags — multiple restartComponent calls between drain points
        // should OR their flags together, not replace. AcqRel ensures visibility
        // across threads: a plugin's audio processor may call restartComponent
        // from a background thread while the main thread drains flags.
        (*base)
            .pending_restart_flags
            .fetch_or(flags, Ordering::AcqRel);
        K_RESULT_OK
    }
}

// ── IComponentHandler2 methods ──────────────────────────────────────

unsafe extern "C" fn ch2_start_group_edit(_: *mut c_void) -> TResult {
    tracing::debug!("IComponentHandler2::startGroupEdit");
    ring_push(HostCbKind::StartGroupEdit, 0, 0);
    K_RESULT_OK
}

unsafe extern "C" fn ch2_finish_group_edit(_: *mut c_void) -> TResult {
    tracing::debug!("IComponentHandler2::finishGroupEdit");
    ring_push(HostCbKind::FinishGroupEdit, 0, 0);
    K_RESULT_OK
}

unsafe extern "C" fn ch2_set_dirty(_: *mut c_void, state: u8) -> TResult {
    tracing::debug!(state, "IComponentHandler2::setDirty");
    ring_push(HostCbKind::SetDirty, 0, state as u64);
    K_RESULT_OK
}

unsafe extern "C" fn ch2_request_open_editor(_: *mut c_void, name: *const c_void) -> TResult {
    tracing::warn!(
        name_ptr = format_args!("0x{:x}", name as usize),
        "IComponentHandler2::requestOpenEditor — not implemented. Plugin may be waiting for a window."
    );
    ring_push(HostCbKind::RequestOpenEditor, 0, name as u64);
    K_RESULT_OK
}

// ── IUnitHandler methods (no-op) ───────────────────────────────────

unsafe extern "C" fn uh_notify_unit_selection(_: *mut c_void, unit_id: i32) -> TResult {
    tracing::debug!(unit_id, "IUnitHandler::notifyUnitSelection");
    ring_push(HostCbKind::UnitSelection, unit_id as u32, 0);
    K_RESULT_OK
}

unsafe extern "C" fn uh_notify_program_list_change(
    _: *mut c_void,
    list_id: i32,
    program_index: i32,
) -> TResult {
    tracing::debug!(
        list_id,
        program_index,
        "IUnitHandler::notifyProgramListChange"
    );
    ring_push(
        HostCbKind::ProgramListChange,
        list_id as u32,
        program_index as u64,
    );
    K_RESULT_OK
}

// ── IRunLoop methods (Linux only) ──────────────────────────────────

#[cfg(target_os = "linux")]
unsafe extern "C" fn rl_register_event_handler(
    this: *mut c_void,
    handler: *mut c_void,
    fd: i32,
) -> TResult {
    unsafe {
        tracing::debug!(
            fd,
            handler = format_args!("0x{:x}", handler as usize),
            "IRunLoop::registerEventHandler"
        );
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, rl_vtable))
            as *const HostContextObj;
        let Some(owned) = RetainedHandler::retain(handler) else {
            return K_RESULT_FALSE;
        };
        let registered = match (*base).run_loop_state.lock() {
            Ok(mut state) => {
                state.event_handlers.push((fd, owned));
                true
            }
            Err(_) => false,
        };
        if registered {
            K_RESULT_OK
        } else {
            K_RESULT_FALSE
        }
    }
}

#[cfg(target_os = "linux")]
unsafe extern "C" fn rl_unregister_event_handler(
    this: *mut c_void,
    handler: *mut c_void,
) -> TResult {
    unsafe {
        tracing::debug!(
            handler = format_args!("0x{:x}", handler as usize),
            "IRunLoop::unregisterEventHandler"
        );
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, rl_vtable))
            as *const HostContextObj;
        let removed = match (*base).run_loop_state.lock() {
            Ok(mut state) => state
                .event_handlers
                .iter()
                .position(|(_, h)| h.ptr == handler)
                .map(|pos| state.event_handlers.remove(pos).1),
            Err(_) => return K_RESULT_FALSE,
        };
        // release outside lock — plugin's release may have side effects.
        drop(removed);
        K_RESULT_OK
    }
}

#[cfg(target_os = "linux")]
unsafe extern "C" fn rl_register_timer(
    this: *mut c_void,
    handler: *mut c_void,
    milliseconds: u64,
) -> TResult {
    unsafe {
        tracing::debug!(
            milliseconds,
            handler = format_args!("0x{:x}", handler as usize),
            "IRunLoop::registerTimer"
        );
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, rl_vtable))
            as *const HostContextObj;
        let Some(owned) = RetainedHandler::retain(handler) else {
            return K_RESULT_FALSE;
        };
        let registered = match (*base).run_loop_state.lock() {
            Ok(mut state) => {
                state
                    .timers
                    .push((milliseconds, owned, std::time::Instant::now()));
                true
            }
            Err(_) => false,
        };
        if registered {
            K_RESULT_OK
        } else {
            K_RESULT_FALSE
        }
    }
}

#[cfg(target_os = "linux")]
unsafe extern "C" fn rl_unregister_timer(this: *mut c_void, handler: *mut c_void) -> TResult {
    unsafe {
        tracing::debug!(
            handler = format_args!("0x{:x}", handler as usize),
            "IRunLoop::unregisterTimer"
        );
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, rl_vtable))
            as *const HostContextObj;
        let removed = match (*base).run_loop_state.lock() {
            Ok(mut state) => state
                .timers
                .iter()
                .position(|(_, h, _)| h.ptr == handler)
                .map(|pos| state.timers.remove(pos).1),
            Err(_) => return K_RESULT_FALSE,
        };
        // release outside lock — plugin's release may have side effects.
        drop(removed);
        K_RESULT_OK
    }
}

// ── HostContextObj impl ─────────────────────────────────────────────

impl HostContextObj {
    pub fn new() -> Self {
        Self {
            host_vtable: &HOST_APP_VTBL,
            pis_vtable: &PIS_VTBL,
            handler_vtable: &CH_VTBL,
            handler2_vtable: &CH2_VTBL,
            uh_vtable: &UH_VTBL,
            handler3_vtable: &CH3_VTBL,
            info_listener_vtable: &IL_VTBL,
            #[cfg(target_os = "linux")]
            rl_vtable: &RL_VTBL,
            // Pre-allocate for typical max params edited per process block.
            pending_params: Mutex::new(Vec::with_capacity(32)),
            pending_restart_flags: AtomicI32::new(0),
            #[cfg(target_os = "linux")]
            run_loop_state: Mutex::new(RunLoopState {
                event_handlers: Vec::new(),
                timers: Vec::new(),
            }),
        }
    }

    /// Release registrations outside the mutex, before the module is unloaded.
    #[cfg(target_os = "linux")]
    pub(crate) fn clear_run_loop(&self) {
        let old = {
            let mut state = self
                .run_loop_state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *state)
        };
        drop(old);
    }

    /// Drain parameter changes queued by `performEdit` callbacks.
    /// Returns the queued changes and leaves the queue empty.
    #[must_use]
    pub(crate) fn drain_pending_params(&self) -> Vec<(u32, f64)> {
        match self.pending_params.lock() {
            Ok(mut queue) => std::mem::take(&mut *queue),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }

    /// Drain accumulated restartComponent flags. Returns 0 if no restart was requested.
    /// The returned value is the OR of all flags set since the last drain.
    pub(crate) fn drain_restart_flags(&self) -> i32 {
        self.pending_restart_flags.swap(0, Ordering::AcqRel)
    }

    /// IHostApplication interface pointer (offset 0).
    /// Use `handler_ptr()` for `IComponent::initialize` and
    /// `IEditController::initialize`.
    pub fn as_ptr(&mut self) -> *mut c_void {
        self as *mut Self as *mut c_void
    }

    /// IComponentHandler interface pointer (for IEditController::setComponentHandler).
    pub fn handler_ptr(&mut self) -> *mut c_void {
        std::ptr::addr_of_mut!(self.handler_vtable) as *mut c_void
    }

    /// Service the Linux IRunLoop: poll registered fds and fire due timers.
    ///
    /// Call this periodically from the editor event loop. Typically every
    /// ~10ms to keep plugin animations smooth and I/O responsive.
    ///
    /// The mutex is released before calling back into plugin code to prevent
    /// deadlocks if the plugin re-enters registration methods from within
    /// `on_fd_is_set` or `on_timer` callbacks.
    ///
    /// # Safety
    /// The plugin module and host context must remain alive during this call.
    /// Callbacks must run on the plugin's UI thread, with no concurrent service
    /// or termination. Owned snapshots keep handlers alive across unregistration.
    #[cfg(target_os = "linux")]
    pub unsafe fn service_run_loop(&self) {
        unsafe {
            // Snapshot event handlers while holding lock, then release before callbacks.
            let handlers = {
                let state = match self.run_loop_state.lock() {
                    Ok(s) => s,
                    Err(poisoned) => poisoned.into_inner(),
                };
                state.event_handlers.clone()
            };

            // Poll registered file descriptors (lock not held).
            if !handlers.is_empty() {
                let mut pollfds: Vec<libc::pollfd> = handlers
                    .iter()
                    .map(|(fd, _)| libc::pollfd {
                        fd: *fd,
                        events: libc::POLLIN | libc::POLLPRI,
                        revents: 0,
                    })
                    .collect();

                // Non-blocking poll (timeout = 0).
                let ready = libc::poll(pollfds.as_mut_ptr(), pollfds.len() as libc::nfds_t, 0);
                if ready > 0 {
                    for (i, pfd) in pollfds.iter().enumerate() {
                        if pfd.revents != 0 {
                            if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                                tracing::debug!(
                                    fd = pfd.fd,
                                    revents = pfd.revents,
                                    "IRunLoop: fd error condition"
                                );
                            }
                            let handler = handlers[i].1.ptr;
                            let vtbl = *(handler as *const *const IEventHandlerVtbl);
                            ((*vtbl).on_fd_is_set)(handler, pfd.fd);
                        }
                    }
                }
            }

            // Snapshot timers while holding lock.
            let timer_snapshot = {
                let state = match self.run_loop_state.lock() {
                    Ok(s) => s,
                    Err(poisoned) => poisoned.into_inner(),
                };
                state.timers.clone()
            };

            // Fire due timers (lock not held — plugin may re-enter register/unregister).
            let now = std::time::Instant::now();
            let mut fired: Vec<(usize, std::time::Instant)> = Vec::new();
            for (i, (interval_ms, owned, last_fired)) in timer_snapshot.iter().enumerate() {
                let handler = owned.ptr;
                if now.duration_since(*last_fired) >= std::time::Duration::from_millis(*interval_ms)
                {
                    fired.push((i, now));
                    let vtbl = *(handler as *const *const ITimerHandlerVtbl);
                    ((*vtbl).on_timer)(handler);
                }
            }

            // Write back updated last_fired timestamps. Match by handler pointer
            // (not index) because callbacks may unregister timers, shifting indices.
            if !fired.is_empty()
                && let Ok(mut state) = self.run_loop_state.lock()
            {
                for (snapshot_idx, fired_at) in &fired {
                    let fired_handler = &timer_snapshot[*snapshot_idx].1;
                    if let Some(entry) = state
                        .timers
                        .iter_mut()
                        .find(|e| std::sync::Arc::ptr_eq(&e.1, fired_handler))
                    {
                        entry.2 = *fired_at;
                    }
                }
            }
        }
    }
}

// ── IConnectionPoint proxy ──────────────────────────────────────────
//
// Instead of connecting component ↔ controller IConnectionPoints directly,
// the host inserts proxies that forward notify() calls. This provides:
// 1. Clean lifetime management — disconnecting the proxy on terminate
// 2. Logging/tracing of cross-component messages
// 3. A future extension point for thread marshaling if needed
//
// Host-owned connection proxies forward messages between peers.

/// IConnectionPoint IID: {70A4156F-6E6E-4026-9891-48BFAA60D8D1}
/// Source: pluginterfaces/vst/ivstmessage.h
pub(crate) const IID_ICONNECTION_POINT: TUID = [
    0x70, 0xA4, 0x15, 0x6F, 0x6E, 0x6E, 0x40, 0x26, 0x98, 0x91, 0x48, 0xBF, 0xAA, 0x60, 0xD8, 0xD1,
];

/// IConnectionPoint vtable: 3 FUnknown + 3 methods.
#[repr(C)]
struct IConnectionPointProxyVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    connect: unsafe extern "C" fn(*mut c_void, other: *mut c_void) -> TResult,
    disconnect: unsafe extern "C" fn(*mut c_void, other: *mut c_void) -> TResult,
    notify: unsafe extern "C" fn(*mut c_void, message: *mut c_void) -> TResult,
}

/// Host-side IConnectionPoint proxy.
///
/// Wraps a real IConnectionPoint (from the plugin's component or controller)
/// and forwards notify() calls. The proxy is heap-allocated and ref-counted.
#[repr(C)]
pub(crate) struct ConnectionProxyObj {
    vtable: *const IConnectionPointProxyVtbl,
    ref_count: std::sync::atomic::AtomicU32,
    /// The real IConnectionPoint target that receives forwarded messages.
    /// Set via connect(), cleared via disconnect(). Not synchronized —
    /// all IConnectionPoint methods must be called from the same thread
    /// (guaranteed by VST3 spec: main thread only).
    target: *mut c_void,
    /// The parent COM object (component or controller) for QI forwarding.
    /// Some plugins query their
    /// IConnectionPoint peer for custom interfaces to obtain engine
    /// pointers or shared state. Since the IConnectionPoint may be a
    /// tearoff object that doesn't forward QIs to the parent, we store
    /// the parent explicitly and forward QIs there.
    qi_parent: *mut c_void,
    /// Label for tracing (e.g., "comp→ctrl" or "ctrl→comp").
    label: &'static str,
}

static CONNECTION_PROXY_VTBL: IConnectionPointProxyVtbl = IConnectionPointProxyVtbl {
    query_interface: cp_proxy_qi,
    add_ref: cp_proxy_add_ref,
    release: cp_proxy_release,
    connect: cp_proxy_connect,
    disconnect: cp_proxy_disconnect,
    notify: cp_proxy_notify,
};

impl ConnectionProxyObj {
    /// Create a new heap-allocated proxy and return as raw COM pointer.
    ///
    /// `qi_parent` is the component or controller COM pointer used to
    /// forward unknown queryInterface calls. This enables plugins that
    /// query custom interfaces from their IConnectionPoint peer.
    pub(crate) fn new_boxed(label: &'static str, qi_parent: *mut c_void) -> *mut c_void {
        // AddRef the parent so the proxy holds a strong reference.
        if !qi_parent.is_null() {
            unsafe {
                let vtbl = *(qi_parent as *const *const FUnknownVtbl);
                ((*vtbl).add_ref)(qi_parent);
            }
        }
        let obj = Box::new(Self {
            vtable: &CONNECTION_PROXY_VTBL,
            ref_count: std::sync::atomic::AtomicU32::new(1),
            target: std::ptr::null_mut(),
            qi_parent,
            label,
        });
        Box::into_raw(obj) as *mut c_void
    }

    /// Clear the forwarding target to prevent dangling notify() calls.
    /// Releases the old target if non-null. Used during terminate cleanup.
    pub(crate) unsafe fn clear_target(ptr: *mut c_void) {
        unsafe {
            let obj = &mut *(ptr as *mut Self);
            let old = obj.target;
            obj.target = std::ptr::null_mut();
            if !old.is_null() {
                let vtbl = *(old as *const *const FUnknownVtbl);
                ((*vtbl).release)(old);
            }
        }
    }
}

unsafe extern "C" fn cp_proxy_qi(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        let proxy = &*(this as *const ConnectionProxyObj);
        if *iid == IID_ICONNECTION_POINT || *iid == IID_FUNKNOWN {
            cp_proxy_add_ref(this);
            *obj = this;
            return K_RESULT_OK;
        }
        // Forward unknown QIs to the parent COM object (component or controller).
        // Some plugins query their
        // IConnectionPoint peer for custom interfaces to obtain engine
        // pointers or shared state. The IConnectionPoint returned by QI
        // may be a tearoff that doesn't forward QIs to the parent, so
        // we forward directly to the parent object.
        if !proxy.qi_parent.is_null() {
            let parent_vtbl = *(proxy.qi_parent as *const *const FUnknownVtbl);
            let result = ((*parent_vtbl).query_interface)(proxy.qi_parent, iid, obj);
            if result == K_RESULT_OK {
                tracing::debug!(
                    label = proxy.label,
                    iid = ?&iid.cast::<[u8; 16]>().read(),
                    "IConnectionPoint QI forwarded to parent"
                );
                return result;
            }
        }
        tracing::debug!(
            label = proxy.label,
            iid = ?&iid.cast::<[u8; 16]>().read(),
            "IConnectionPoint QI → kNoInterface"
        );
        *obj = std::ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "C" fn cp_proxy_add_ref(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const ConnectionProxyObj);
        obj.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }
}

unsafe extern "C" fn cp_proxy_release(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const ConnectionProxyObj);
        let prev = obj.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(Ordering::Acquire);
            // Release the qi_parent reference before dropping.
            let obj_mut = &mut *(this as *mut ConnectionProxyObj);
            if !obj_mut.qi_parent.is_null() {
                let vtbl = *(obj_mut.qi_parent as *const *const FUnknownVtbl);
                ((*vtbl).release)(obj_mut.qi_parent);
                obj_mut.qi_parent = std::ptr::null_mut();
            }
            drop(Box::from_raw(this as *mut ConnectionProxyObj));
            return 0;
        }
        prev - 1
    }
}

unsafe extern "C" fn cp_proxy_connect(this: *mut c_void, other: *mut c_void) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut ConnectionProxyObj);
        tracing::debug!(label = obj.label, peer = ?other, "IConnectionPoint connect");
        // addRef the new target (COM ownership: we retain the pointer).
        if !other.is_null() {
            let vtbl = *(other as *const *const FUnknownVtbl);
            ((*vtbl).add_ref)(other);
        }
        obj.target = other;
        tracing::trace!(label = obj.label, "ConnectionProxy::connect");
        K_RESULT_OK
    }
}

unsafe extern "C" fn cp_proxy_disconnect(this: *mut c_void, _other: *mut c_void) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut ConnectionProxyObj);
        tracing::debug!(label = obj.label, "IConnectionPoint disconnect");
        // Release the old target and clear. Proxy has at most one connection.
        let old = obj.target;
        obj.target = std::ptr::null_mut();
        if !old.is_null() {
            let vtbl = *(old as *const *const FUnknownVtbl);
            ((*vtbl).release)(old);
        }
        tracing::trace!(label = obj.label, "ConnectionProxy::disconnect");
        K_RESULT_OK
    }
}

unsafe extern "C" fn cp_proxy_notify(this: *mut c_void, message: *mut c_void) -> TResult {
    unsafe {
        let obj = &*(this as *const ConnectionProxyObj);
        tracing::debug!(label = obj.label, message = ?message, "IConnectionPoint notify");
        if obj.target.is_null() {
            tracing::debug!(
                label = obj.label,
                "ConnectionProxy::notify — target is null, dropping message"
            );
            return K_RESULT_FALSE;
        }
        // Forward the message to the real target's notify().
        // The target is an IConnectionPoint — its vtable has notify at slot 5.
        let target_vtbl = *(obj.target as *const *const IConnectionPointProxyVtbl);
        let result = ((*target_vtbl).notify)(obj.target, message);
        tracing::debug!(
            label = obj.label,
            result,
            "ConnectionProxy::notify forwarded"
        );
        result
    }
}

// ── IMessage minimal implementation ─────────────────────────────────

/// IMessage vtable: 3 FUnknown + 3 IMessage methods.
#[repr(C)]
struct IMessageVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    get_message_id: unsafe extern "C" fn(*mut c_void) -> *const u8,
    set_message_id: unsafe extern "C" fn(*mut c_void, id: *const u8),
    get_attributes: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
}

/// Heap-allocated IMessage implementation.
#[repr(C)]
struct MessageObj {
    vtable: *const IMessageVtbl,
    ref_count: std::sync::atomic::AtomicU32,
    message_id: Vec<u8>,
    attributes: *mut c_void, // AttributeListObj pointer
}

static MESSAGE_VTBL: IMessageVtbl = IMessageVtbl {
    query_interface: msg_query_interface,
    add_ref: msg_add_ref,
    release: msg_release,
    get_message_id: msg_get_message_id,
    set_message_id: msg_set_message_id,
    get_attributes: msg_get_attributes,
};

impl MessageObj {
    /// Create a new heap-allocated message and return as raw pointer.
    fn new_boxed() -> *mut c_void {
        let attr = AttributeListObj::new_boxed();
        let msg = Box::new(Self {
            vtable: &MESSAGE_VTBL,
            ref_count: std::sync::atomic::AtomicU32::new(1),
            message_id: vec![0],
            attributes: attr,
        });
        Box::into_raw(msg) as *mut c_void
    }
}

unsafe extern "C" fn msg_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        if *iid == IID_IMESSAGE || *iid == IID_FUNKNOWN {
            msg_add_ref(this);
            *obj = this;
            return K_RESULT_OK;
        }
        *obj = std::ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "C" fn msg_add_ref(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const MessageObj);
        obj.ref_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }
}

unsafe extern "C" fn msg_release(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const MessageObj);
        let prev = obj
            .ref_count
            .fetch_sub(1, std::sync::atomic::Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
            let boxed = Box::from_raw(this as *mut MessageObj);
            if !boxed.attributes.is_null() {
                attr_release(boxed.attributes);
            }
            // boxed drops here
            return 0;
        }
        prev - 1
    }
}

unsafe extern "C" fn msg_get_message_id(this: *mut c_void) -> *const u8 {
    unsafe { (*(this as *const MessageObj)).message_id.as_ptr() }
}

unsafe extern "C" fn msg_set_message_id(this: *mut c_void, id: *const u8) {
    unsafe {
        let obj = &mut *(this as *mut MessageObj);
        let mut key = read_cstr_key(id);
        key.push(0); // IMessage stores the null terminator
        obj.message_id = key;
    }
}

/// Returns a borrowed (non-retained) pointer to the message's attribute list.
/// Per VST3 convention, IMessage::getAttributes() returns a raw pointer that
/// the caller may use within the same scope but must NOT retain (no addRef).
/// The attribute list's lifetime is tied to the MessageObj — it is freed when
/// the message's ref count hits zero (msg_release).
unsafe extern "C" fn msg_get_attributes(this: *mut c_void) -> *mut c_void {
    unsafe { (*(this as *const MessageObj)).attributes }
}

// ── IAttributeList minimal implementation ────────────────────────────

/// IAttributeList IID: {1E5F0AEB-CC7F-4533-A254-401138AD5EE4}
const IID_IATTRIBUTE_LIST: TUID = [
    0x1E, 0x5F, 0x0A, 0xEB, 0xCC, 0x7F, 0x45, 0x33, 0xA2, 0x54, 0x40, 0x11, 0x38, 0xAD, 0x5E, 0xE4,
];

/// IAttributeList vtable: 3 FUnknown + 8 IAttributeList methods.
#[repr(C)]
struct IAttributeListVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    set_int: unsafe extern "C" fn(*mut c_void, id: *const u8, value: i64) -> TResult,
    get_int: unsafe extern "C" fn(*mut c_void, id: *const u8, value: *mut i64) -> TResult,
    set_float: unsafe extern "C" fn(*mut c_void, id: *const u8, value: f64) -> TResult,
    get_float: unsafe extern "C" fn(*mut c_void, id: *const u8, value: *mut f64) -> TResult,
    set_string: unsafe extern "C" fn(*mut c_void, id: *const u8, string: *const u16) -> TResult,
    get_string:
        unsafe extern "C" fn(*mut c_void, id: *const u8, string: *mut u16, size: u32) -> TResult,
    set_binary:
        unsafe extern "C" fn(*mut c_void, id: *const u8, data: *const c_void, size: u32) -> TResult,
    get_binary: unsafe extern "C" fn(
        *mut c_void,
        id: *const u8,
        data: *mut *const c_void,
        size: *mut u32,
    ) -> TResult,
}

/// Attribute value types for IAttributeList storage.
enum AttributeValue {
    Int(i64),
    Float(f64),
    String(Vec<u16>),
    Binary(Vec<u8>),
}

#[repr(C)]
struct AttributeListObj {
    vtable: *const IAttributeListVtbl,
    ref_count: std::sync::atomic::AtomicU32,
    data: HashMap<Vec<u8>, AttributeValue>,
}

static ATTRIBUTE_LIST_VTBL: IAttributeListVtbl = IAttributeListVtbl {
    query_interface: attr_query_interface,
    add_ref: attr_add_ref,
    release: attr_release,
    set_int: attr_set_int,
    get_int: attr_get_int,
    set_float: attr_set_float,
    get_float: attr_get_float,
    set_string: attr_set_string,
    get_string: attr_get_string,
    set_binary: attr_set_binary,
    get_binary: attr_get_binary,
};

impl AttributeListObj {
    fn new_boxed() -> *mut c_void {
        let obj = Box::new(Self {
            vtable: &ATTRIBUTE_LIST_VTBL,
            ref_count: std::sync::atomic::AtomicU32::new(1),
            data: HashMap::new(),
        });
        Box::into_raw(obj) as *mut c_void
    }
}

unsafe extern "C" fn attr_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        if *iid == IID_IATTRIBUTE_LIST || *iid == IID_FUNKNOWN {
            attr_add_ref(this);
            *obj = this;
            return K_RESULT_OK;
        }
        *obj = std::ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "C" fn attr_add_ref(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        obj.ref_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }
}

unsafe extern "C" fn attr_release(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        let prev = obj
            .ref_count
            .fetch_sub(1, std::sync::atomic::Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
            drop(Box::from_raw(this as *mut AttributeListObj));
            return 0;
        }
        prev - 1
    }
}

/// Read a null-terminated C string into a Vec<u8> key (without the null terminator).
unsafe fn read_cstr_key(id: *const u8) -> Vec<u8> {
    unsafe {
        let mut len = 0;
        // SAFETY: VST3 contract guarantees null-terminated C strings. The 4096-byte
        // cap is a safety net to prevent runaway reads from buggy plugins.
        while len < 4096 && *id.add(len) != 0 {
            len += 1;
        }
        std::slice::from_raw_parts(id, len).to_vec()
    }
}

unsafe extern "C" fn attr_set_int(this: *mut c_void, id: *const u8, value: i64) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut AttributeListObj);
        let key = read_cstr_key(id);
        obj.data.insert(key, AttributeValue::Int(value));
        K_RESULT_OK
    }
}
unsafe extern "C" fn attr_get_int(this: *mut c_void, id: *const u8, value: *mut i64) -> TResult {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        let key = read_cstr_key(id);
        match obj.data.get(&key) {
            Some(AttributeValue::Int(v)) => {
                *value = *v;
                K_RESULT_OK
            }
            _ => K_RESULT_FALSE,
        }
    }
}
unsafe extern "C" fn attr_set_float(this: *mut c_void, id: *const u8, value: f64) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut AttributeListObj);
        let key = read_cstr_key(id);
        obj.data.insert(key, AttributeValue::Float(value));
        K_RESULT_OK
    }
}
unsafe extern "C" fn attr_get_float(this: *mut c_void, id: *const u8, value: *mut f64) -> TResult {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        let key = read_cstr_key(id);
        match obj.data.get(&key) {
            Some(AttributeValue::Float(v)) => {
                *value = *v;
                K_RESULT_OK
            }
            _ => K_RESULT_FALSE,
        }
    }
}
unsafe extern "C" fn attr_set_string(
    this: *mut c_void,
    id: *const u8,
    string: *const u16,
) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut AttributeListObj);
        let key = read_cstr_key(id);
        let mut len = 0;
        // SAFETY: VST3 contract guarantees null-terminated UTF-16 strings. The
        // 65536-char cap is a safety net to prevent runaway reads from buggy plugins.
        while len < 65536 && *string.add(len) != 0 {
            len += 1;
        }
        let chars = std::slice::from_raw_parts(string, len + 1).to_vec(); // include null
        obj.data.insert(key, AttributeValue::String(chars));
        K_RESULT_OK
    }
}
unsafe extern "C" fn attr_get_string(
    this: *mut c_void,
    id: *const u8,
    string: *mut u16,
    size: u32,
) -> TResult {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        let key = read_cstr_key(id);
        match obj.data.get(&key) {
            Some(AttributeValue::String(chars)) => {
                let capacity_chars = size as usize / std::mem::size_of::<u16>();
                if capacity_chars == 0 {
                    return K_RESULT_FALSE;
                }
                let copy_len = capacity_chars.min(chars.len());
                std::ptr::copy_nonoverlapping(chars.as_ptr(), string, copy_len);
                // Always null-terminate within buffer bounds
                if capacity_chars > 0 && copy_len < capacity_chars {
                    *string.add(copy_len) = 0;
                } else if capacity_chars > 0 {
                    *string.add(capacity_chars - 1) = 0;
                }
                K_RESULT_OK
            }
            _ => K_RESULT_FALSE,
        }
    }
}
unsafe extern "C" fn attr_set_binary(
    this: *mut c_void,
    id: *const u8,
    data: *const c_void,
    size: u32,
) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut AttributeListObj);
        let key = read_cstr_key(id);
        let bytes = std::slice::from_raw_parts(data as *const u8, size as usize).to_vec();
        obj.data.insert(key, AttributeValue::Binary(bytes));
        K_RESULT_OK
    }
}
unsafe extern "C" fn attr_get_binary(
    this: *mut c_void,
    id: *const u8,
    data: *mut *const c_void,
    size: *mut u32,
) -> TResult {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        let key = read_cstr_key(id);
        match obj.data.get(&key) {
            Some(AttributeValue::Binary(bytes)) => {
                *data = bytes.as_ptr() as *const c_void;
                *size = bytes.len() as u32;
                K_RESULT_OK
            }
            _ => K_RESULT_FALSE,
        }
    }
}

// ── IBStream / MemoryStream ─────────────────────────────────────────

/// IBStream vtable: FUnknown (3) + IBStream (4) = 7 function pointers.
#[repr(C)]
pub(crate) struct IBStreamVtbl {
    // FUnknown (3)
    pub query_interface:
        unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    // IBStream (4)
    pub read: unsafe extern "C" fn(
        this: *mut c_void,
        buffer: *mut c_void,
        num_bytes: i32,
        num_bytes_read: *mut i32,
    ) -> TResult,
    pub write: unsafe extern "C" fn(
        this: *mut c_void,
        buffer: *const c_void,
        num_bytes: i32,
        num_bytes_written: *mut i32,
    ) -> TResult,
    pub seek:
        unsafe extern "C" fn(this: *mut c_void, pos: i64, mode: i32, result: *mut i64) -> TResult,
    pub tell: unsafe extern "C" fn(this: *mut c_void, pos: *mut i64) -> TResult,
}

/// In-memory IBStream for state transfer between component and controller.
#[repr(C)]
pub(crate) struct MemoryStream {
    vtable: *const IBStreamVtbl,
    data: Vec<u8>,
    position: usize,
}

static MEMORY_STREAM_VTBL: IBStreamVtbl = IBStreamVtbl {
    query_interface: mem_stream_query_interface,
    add_ref: mem_stream_add_ref,
    release: mem_stream_release,
    read: mem_stream_read,
    write: mem_stream_write,
    seek: mem_stream_seek,
    tell: mem_stream_tell,
};

/// IBStream IID: {C3BF6EA2-3099-4752-9B6B-F9901EE33E9B}
const IID_IBSTREAM: TUID = [
    0xC3, 0xBF, 0x6E, 0xA2, 0x30, 0x99, 0x47, 0x52, 0x9B, 0x6B, 0xF9, 0x90, 0x1E, 0xE3, 0x3E, 0x9B,
];

unsafe extern "C" fn mem_stream_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        if *iid == IID_FUNKNOWN || *iid == IID_IBSTREAM {
            mem_stream_add_ref(this);
            *obj = this;
            return K_RESULT_OK;
        }
        *obj = std::ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "C" fn mem_stream_add_ref(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "C" fn mem_stream_release(_this: *mut c_void) -> u32 {
    1
}

unsafe fn stream_from_ptr(this: *mut c_void) -> *mut MemoryStream {
    this as *mut MemoryStream
}

unsafe extern "C" fn mem_stream_read(
    this: *mut c_void,
    buffer: *mut c_void,
    num_bytes: i32,
    num_bytes_read: *mut i32,
) -> TResult {
    unsafe {
        let s = &mut *stream_from_ptr(this);
        if num_bytes <= 0 {
            if !num_bytes_read.is_null() {
                *num_bytes_read = 0;
            }
            return K_RESULT_OK;
        }
        let available = s.data.len().saturating_sub(s.position);
        let to_read = (num_bytes as usize).min(available);
        if to_read > 0 {
            std::ptr::copy_nonoverlapping(
                s.data.as_ptr().add(s.position),
                buffer as *mut u8,
                to_read,
            );
            s.position += to_read;
        }
        if !num_bytes_read.is_null() {
            *num_bytes_read = to_read as i32;
        }
        // Steinberg's reference MemoryStream always returns kResultTrue from
        // read(), even when fewer bytes are available than requested. The caller
        // checks *num_bytes_read for the actual count. Returning kResultFalse
        // here caused some plugins  to treat partial reads as
        // errors, leaving internal state uninitialized.
        K_RESULT_OK
    }
}

unsafe extern "C" fn mem_stream_write(
    this: *mut c_void,
    buffer: *const c_void,
    num_bytes: i32,
    num_bytes_written: *mut i32,
) -> TResult {
    unsafe {
        let s = &mut *stream_from_ptr(this);
        if num_bytes <= 0 {
            if !num_bytes_written.is_null() {
                *num_bytes_written = 0;
            }
            return K_RESULT_OK;
        }
        let count = num_bytes as usize;
        // Extend or overwrite
        let end = s.position + count;
        if end > s.data.len() {
            s.data.resize(end, 0);
        }
        std::ptr::copy_nonoverlapping(
            buffer as *const u8,
            s.data.as_mut_ptr().add(s.position),
            count,
        );
        s.position += count;
        if !num_bytes_written.is_null() {
            *num_bytes_written = count as i32;
        }
        K_RESULT_OK
    }
}

unsafe extern "C" fn mem_stream_seek(
    this: *mut c_void,
    pos: i64,
    mode: i32,
    result: *mut i64,
) -> TResult {
    unsafe {
        let s = &mut *stream_from_ptr(this);
        let new_pos: i64 = match mode {
            K_IB_SEEK_SET => pos,
            K_IB_SEEK_CUR => s.position as i64 + pos,
            K_IB_SEEK_END => s.data.len() as i64 + pos,
            _ => return K_RESULT_FALSE,
        };
        s.position = new_pos.max(0) as usize;
        if !result.is_null() {
            *result = s.position as i64;
        }
        K_RESULT_OK
    }
}

unsafe extern "C" fn mem_stream_tell(this: *mut c_void, pos: *mut i64) -> TResult {
    unsafe {
        let s = &mut *stream_from_ptr(this);
        if !pos.is_null() {
            *pos = s.position as i64;
        }
        K_RESULT_OK
    }
}

impl MemoryStream {
    pub fn new() -> Self {
        Self {
            vtable: &MEMORY_STREAM_VTBL,
            data: Vec::new(),
            position: 0,
        }
    }

    /// Create a memory stream pre-loaded with the given bytes.
    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self {
            vtable: &MEMORY_STREAM_VTBL,
            data,
            position: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn reset_position(&mut self) {
        self.position = 0;
    }

    /// Get the COM interface pointer. The object must not be moved
    /// while this pointer is in use (it points to `self`).
    pub fn as_ptr(&mut self) -> *mut c_void {
        self as *mut Self as *mut c_void
    }

    /// Consume the stream and return its data bytes.
    pub fn into_vec(self) -> Vec<u8> {
        self.data
    }
}

/// Seek modes matching the VST3 IBStream spec.
const K_IB_SEEK_SET: i32 = 0;
const K_IB_SEEK_CUR: i32 = 1;
const K_IB_SEEK_END: i32 = 2;

// ── Helpers ─────────────────────────────────────────────────────────

/// Extract a UTF-8 string from a null-terminated byte buffer.
pub(crate) fn cstr_from_buf(buf: &[u8]) -> String {
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).to_string()
}

// ── ParameterInfo ─────────────────────────────────────────────────

/// VST3 `Vst::ParameterInfo` struct (from `ivsteditcontroller.h`).
///
/// Layout verified against SDK: id(4) + title(256) + shortTitle(256) +
/// units(256) + stepCount(4) + defaultNormalizedValue(8) + unitId(4) + flags(4) = 792 bytes.
#[repr(C)]
pub(crate) struct ParameterInfo {
    pub id: u32,
    pub title: [u16; 128],
    pub short_title: [u16; 128],
    pub units: [u16; 128],
    pub step_count: i32,
    pub default_normalized_value: f64,
    pub unit_id: i32,
    pub flags: i32,
}

impl Default for ParameterInfo {
    fn default() -> Self {
        Self {
            id: 0,
            title: [0u16; 128],
            short_title: [0u16; 128],
            units: [0u16; 128],
            step_count: 0,
            default_normalized_value: 0.0,
            unit_id: 0,
            flags: 0,
        }
    }
}

// ── Layout assertions ───────────────────────────────────────────────

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(std::mem::size_of::<ProcessSetup>() == 24);
    assert!(std::mem::size_of::<AudioBusBuffers>() == 24);
    assert!(std::mem::size_of::<ProcessData>() == 80);
    assert!(std::mem::size_of::<Event>() == 48);
    assert!(std::mem::size_of::<NoteOnEvent>() == 20);
    assert!(std::mem::size_of::<NoteOffEvent>() == 16);
    assert!(std::mem::size_of::<ParameterInfo>() == 792);

    let ptr = std::mem::size_of::<usize>();
    assert!(std::mem::size_of::<IComponentVtbl>() == 14 * ptr);
    assert!(std::mem::size_of::<IAudioProcessorVtbl>() == 11 * ptr);
    assert!(std::mem::size_of::<IBStreamVtbl>() == 7 * ptr);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn timer_snapshots_own_handlers_through_reentrant_unregistration() {
        use std::sync::atomic::AtomicUsize;
        #[repr(C)]
        struct Timer {
            vtable: *const ITimerHandlerVtbl,
            refs: AtomicUsize,
            loop_ptr: *mut c_void,
            target: *mut c_void,
            calls: *const AtomicUsize,
            freed: *const AtomicUsize,
        }
        unsafe extern "C" fn qi(_: *mut c_void, _: *const TUID, _: *mut *mut c_void) -> TResult {
            K_NO_INTERFACE
        }
        unsafe extern "C" fn retain(this: *mut c_void) -> u32 {
            unsafe { (*(this as *mut Timer)).refs.fetch_add(1, Ordering::Relaxed) as u32 + 1 }
        }
        unsafe extern "C" fn release(this: *mut c_void) -> u32 {
            unsafe {
                let timer = &*(this as *mut Timer);
                let remaining = timer.refs.fetch_sub(1, Ordering::AcqRel) - 1;
                if remaining == 0 {
                    (*timer.freed).fetch_add(1, Ordering::Relaxed);
                    drop(Box::from_raw(this as *mut Timer));
                }
                remaining as u32
            }
        }
        unsafe extern "C" fn fire(this: *mut c_void) {
            unsafe {
                let timer = &*(this as *mut Timer);
                (*timer.calls).fetch_add(1, Ordering::Relaxed);
                if !timer.target.is_null() {
                    rl_unregister_timer(timer.loop_ptr, timer.target);
                    release(timer.target); // plugin's own reference to B
                    rl_unregister_timer(timer.loop_ptr, this);
                    release(this); // plugin's own reference to A
                }
            }
        }
        static VTABLE: ITimerHandlerVtbl = ITimerHandlerVtbl {
            _query_interface: qi,
            _add_ref: retain,
            _release: release,
            on_timer: fire,
        };
        let calls = AtomicUsize::new(0);
        let freed = AtomicUsize::new(0);
        let mut context = HostContextObj::new();
        let loop_ptr = std::ptr::addr_of_mut!(context.rl_vtable).cast();
        let timer = |target| {
            Box::into_raw(Box::new(Timer {
                vtable: &VTABLE,
                refs: AtomicUsize::new(1),
                loop_ptr,
                target,
                calls: &calls,
                freed: &freed,
            }))
            .cast()
        };
        let b = timer(std::ptr::null_mut());
        let a = timer(b);
        unsafe {
            assert_eq!(rl_register_timer(loop_ptr, a, 0), K_RESULT_OK);
            assert_eq!(rl_register_timer(loop_ptr, b, 0), K_RESULT_OK);
            context.service_run_loop();
        }
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert_eq!(freed.load(Ordering::Relaxed), 2);
        assert!(context.run_loop_state.lock().unwrap().timers.is_empty());
        context.clear_run_loop();
    }

    #[test]
    fn callback_ring_supports_concurrent_wraparound_and_nonblocking_reads() {
        std::thread::scope(|scope| {
            for producer in 0..4 {
                scope.spawn(move || {
                    for n in 0..2048 {
                        ring_push(HostCbKind::PerformEdit, producer, n);
                    }
                });
            }
            scope.spawn(|| {
                for _ in 0..128 {
                    let _ = dump_callback_ring();
                }
            });
        });
        assert!(dump_callback_ring().contains("PerformEdit"));
        let _held = RING[0].data.lock().unwrap();
        // A dump must skip a busy slot rather than wait for its writer.
        let _ = dump_callback_ring();
    }

    #[test]
    fn event_list_basic_operations() {
        let mut list = EventListObj::new();
        assert_eq!(unsafe { event_list_get_event_count(list.as_ptr()) }, 0);

        let mut list = EventListObj::with_events(vec![
            Event::note_on(0, 0, 60, 1.0, 60),
            Event::note_off(44100, 0, 60, 0.0, 60),
        ]);
        let ptr = list.as_ptr();

        unsafe {
            assert_eq!(event_list_get_event_count(ptr), 2);

            let mut event = std::mem::zeroed::<Event>();
            assert_eq!(event_list_get_event(ptr, 0, &mut event), K_RESULT_OK);
            assert_eq!(event.event_type, K_NOTE_ON_EVENT);
            assert_eq!(event.sample_offset, 0);
            assert_eq!(event.data.note_on.pitch, 60);

            assert_eq!(event_list_get_event(ptr, 1, &mut event), K_RESULT_OK);
            assert_eq!(event.event_type, K_NOTE_OFF_EVENT);
            assert_eq!(event.sample_offset, 44100);

            // Out of bounds
            assert_eq!(event_list_get_event(ptr, 2, &mut event), K_RESULT_FALSE);
        }
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn struct_layout_sizes() {
        assert_eq!(std::mem::size_of::<ProcessSetup>(), 24);
        assert_eq!(std::mem::size_of::<AudioBusBuffers>(), 24);
        assert_eq!(std::mem::size_of::<ProcessData>(), 80);
        assert_eq!(std::mem::size_of::<Event>(), 48);
    }

    #[test]
    fn host_context_qi_returns_correct_sub_interfaces() {
        let mut ctx = HostContextObj::new();
        let base = ctx.as_ptr() as *const HostContextObj;
        let mut out: *mut c_void = std::ptr::null_mut();

        unsafe {
            // IHostApplication (offset 0, also the FUnknown identity)
            assert_eq!(
                unified_host_qi(base, &IID_IHOST_APPLICATION, &mut out),
                K_RESULT_OK
            );
            assert_eq!(out, base as *mut c_void);

            // FUnknown resolves to the same base pointer
            assert_eq!(unified_host_qi(base, &IID_FUNKNOWN, &mut out), K_RESULT_OK);
            assert_eq!(out, base as *mut c_void);

            // IPlugInterfaceSupport (offset 8)
            assert_eq!(
                unified_host_qi(base, &IID_IPLUG_INTERFACE_SUPPORT, &mut out),
                K_RESULT_OK
            );
            assert_eq!(out, std::ptr::addr_of!((*base).pis_vtable) as *mut c_void);

            // IComponentHandler (offset 16)
            assert_eq!(
                unified_host_qi(base, &IID_ICOMPONENT_HANDLER, &mut out),
                K_RESULT_OK
            );
            assert_eq!(
                out,
                std::ptr::addr_of!((*base).handler_vtable) as *mut c_void
            );

            // IComponentHandler2 (offset 24)
            assert_eq!(
                unified_host_qi(base, &IID_ICOMPONENT_HANDLER2, &mut out),
                K_RESULT_OK
            );
            assert_eq!(
                out,
                std::ptr::addr_of!((*base).handler2_vtable) as *mut c_void
            );

            // IUnitHandler (offset 32)
            assert_eq!(
                unified_host_qi(base, &IID_IUNIT_HANDLER, &mut out),
                K_RESULT_OK
            );
            assert_eq!(out, std::ptr::addr_of!((*base).uh_vtable) as *mut c_void);

            // Unsupported IID returns kNoInterface and nulls the output
            let bogus: TUID = [0xFF; 16];
            assert_eq!(unified_host_qi(base, &bogus, &mut out), K_NO_INTERFACE);
            assert!(out.is_null());
        }
    }

    #[test]
    fn memory_stream_read_full() {
        let data = vec![10u8, 20, 30, 40, 50];
        let mut stream = MemoryStream::from_bytes(data.clone());
        let ptr = stream.as_ptr();

        let mut buf = [0u8; 5];
        let mut bytes_read: i32 = 0;
        unsafe {
            let result = mem_stream_read(ptr, buf.as_mut_ptr() as *mut c_void, 5, &mut bytes_read);
            assert_eq!(result, K_RESULT_OK);
            assert_eq!(bytes_read, 5);
            assert_eq!(buf, [10, 20, 30, 40, 50]);
        }
    }

    #[test]
    fn memory_stream_read_partial() {
        let data = vec![1u8, 2, 3];
        let mut stream = MemoryStream::from_bytes(data);
        let ptr = stream.as_ptr();

        let mut buf = [0u8; 2];
        let mut bytes_read: i32 = 0;
        unsafe {
            mem_stream_read(ptr, buf.as_mut_ptr() as *mut c_void, 2, &mut bytes_read);
            assert_eq!(bytes_read, 2);
            assert_eq!(buf, [1, 2]);

            // Read remaining
            mem_stream_read(ptr, buf.as_mut_ptr() as *mut c_void, 2, &mut bytes_read);
            assert_eq!(bytes_read, 1);
            assert_eq!(buf[0], 3);

            // Read past end
            mem_stream_read(ptr, buf.as_mut_ptr() as *mut c_void, 2, &mut bytes_read);
            assert_eq!(bytes_read, 0);
        }
    }

    #[test]
    fn memory_stream_seek_and_tell() {
        let data = vec![0u8; 100];
        let mut stream = MemoryStream::from_bytes(data);
        let ptr = stream.as_ptr();

        unsafe {
            let mut pos: i64 = 0;

            // Seek from start
            assert_eq!(
                mem_stream_seek(ptr, 50, K_IB_SEEK_SET, &mut pos),
                K_RESULT_OK
            );
            assert_eq!(pos, 50);

            // Tell
            mem_stream_tell(ptr, &mut pos);
            assert_eq!(pos, 50);

            // Seek from current
            assert_eq!(
                mem_stream_seek(ptr, 10, K_IB_SEEK_CUR, &mut pos),
                K_RESULT_OK
            );
            assert_eq!(pos, 60);

            // Seek from end
            assert_eq!(
                mem_stream_seek(ptr, -20, K_IB_SEEK_END, &mut pos),
                K_RESULT_OK
            );
            assert_eq!(pos, 80);

            // Seek past end — clamps to end
            assert_eq!(
                mem_stream_seek(ptr, 1, K_IB_SEEK_END, &mut pos),
                K_RESULT_OK
            );
            assert_eq!(pos, 101);

            // Seek before start — clamps to 0
            assert_eq!(
                mem_stream_seek(ptr, -1, K_IB_SEEK_SET, &mut pos),
                K_RESULT_OK
            );
            assert_eq!(pos, 0);
        }
    }

    #[test]
    fn memory_stream_write() {
        let mut stream = MemoryStream::new();
        let ptr = stream.as_ptr();

        let data = [1u8; 5];
        let mut bytes_written: i32 = 0;
        unsafe {
            let result =
                mem_stream_write(ptr, data.as_ptr() as *const c_void, 5, &mut bytes_written);
            assert_eq!(result, K_RESULT_OK);
            assert_eq!(bytes_written, 5);
        }
    }
}
