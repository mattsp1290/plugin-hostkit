use super::*;

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
    #[cfg(target_pointer_width = "64")]
    fn struct_layout_sizes() {
        assert_eq!(std::mem::size_of::<ProcessSetup>(), 24);
        assert_eq!(std::mem::size_of::<AudioBusBuffers>(), 24);
        assert_eq!(std::mem::size_of::<ProcessData>(), 80);
        assert_eq!(std::mem::size_of::<Event>(), 48);
    }
}
