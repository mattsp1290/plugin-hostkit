use super::*;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
