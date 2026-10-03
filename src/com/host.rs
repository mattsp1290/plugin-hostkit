use super::*;

mod run_loop;
#[cfg(target_os = "linux")]
use run_loop::*;

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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
