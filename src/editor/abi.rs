//! Editor interface layouts verified against the pinned SDK.

use super::*;

// ── IEditController vtable ──────────────────────────────────────────

/// IEditController extends IPluginBase (extends FUnknown).
/// Layout: 3 FUnknown + 2 IPluginBase + 13 IEditController = 18 function pointers.
#[repr(C)]
pub(super) struct IEditControllerVtbl {
    // FUnknown (3)
    pub(super) query_interface:
        unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    pub(super) add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub(super) release: unsafe extern "C" fn(*mut c_void) -> u32,
    // IPluginBase (2)
    pub(super) initialize: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    pub(super) terminate: unsafe extern "C" fn(*mut c_void) -> TResult,
    // IEditController (13)
    pub(super) set_component_state: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    pub(super) set_state: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    pub(super) get_state: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    pub(super) get_parameter_count: unsafe extern "C" fn(*mut c_void) -> i32,
    pub(super) get_parameter_info: unsafe extern "C" fn(*mut c_void, i32, *mut c_void) -> TResult,
    pub(super) get_param_string_by_value:
        unsafe extern "C" fn(*mut c_void, u32, f64, *mut u16) -> TResult,
    pub(super) get_param_value_by_string:
        unsafe extern "C" fn(*mut c_void, u32, *const u16, *mut f64) -> TResult,
    pub(super) normalized_param_to_plain: unsafe extern "C" fn(*mut c_void, u32, f64) -> f64,
    pub(super) plain_param_to_normalized: unsafe extern "C" fn(*mut c_void, u32, f64) -> f64,
    pub(super) get_param_normalized: unsafe extern "C" fn(*mut c_void, u32) -> f64,
    pub(super) set_param_normalized: unsafe extern "C" fn(*mut c_void, u32, f64) -> TResult,
    pub(super) set_component_handler: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    pub(super) create_view: unsafe extern "C" fn(*mut c_void, *const u8) -> *mut c_void,
}

// ── IPlugView vtable ────────────────────────────────────────────────

/// IPlugView extends FUnknown.
/// Layout: 3 FUnknown + 12 IPlugView = 15 function pointers.
#[repr(C)]
pub(super) struct IPlugViewVtbl {
    // FUnknown (3)
    pub(super) query_interface:
        unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    pub(super) add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub(super) release: unsafe extern "C" fn(*mut c_void) -> u32,
    // IPlugView (12)
    pub(super) is_platform_type_supported: unsafe extern "C" fn(*mut c_void, *const u8) -> TResult,
    pub(super) attached: unsafe extern "C" fn(*mut c_void, *mut c_void, *const u8) -> TResult,
    pub(super) removed: unsafe extern "C" fn(*mut c_void) -> TResult,
    pub(super) on_wheel: unsafe extern "C" fn(*mut c_void, f32) -> TResult,
    pub(super) on_key_down: unsafe extern "C" fn(*mut c_void, u16, i16, i16) -> TResult,
    pub(super) on_key_up: unsafe extern "C" fn(*mut c_void, u16, i16, i16) -> TResult,
    pub(super) get_size: unsafe extern "C" fn(*mut c_void, *mut ViewRect) -> TResult,
    pub(super) on_size: unsafe extern "C" fn(*mut c_void, *mut ViewRect) -> TResult,
    pub(super) on_focus: unsafe extern "C" fn(*mut c_void, u8) -> TResult,
    pub(super) set_frame: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    pub(super) can_resize: unsafe extern "C" fn(*mut c_void) -> TResult,
    pub(super) check_size_constraint: unsafe extern "C" fn(*mut c_void, *mut ViewRect) -> TResult,
}
